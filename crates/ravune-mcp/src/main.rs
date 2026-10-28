//! `ravune-mcp`: an MCP server on stdio that drives a running Ravune. MCP clients (Claude
//! Code: `claude mcp add ravune -- <path>/ravune-mcp`) start it; it reads the port and the
//! secret Ravune wrote into its data folder (`agent.json`, there while Preferences → Agents
//! is on) and forwards each tool call to Ravune as one JSON line.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;

use serde_json::{Value, json};

const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Ravune is a DAW. Every edit is an operator: find them with search_operators and run them with run_operator. \
Read the project with query_project (ticks: 96 per beat, 24 per 16th step, 384 per bar), find sounds with browse_sounds and listen with \
render_and_analyze. Everything you change until checkpoint is one undo step for the user.";

/// Ravune's data folder, as the app finds it (eframe's storage dir, or `RAVUNE_DATA`).
fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("RAVUNE_DATA") {
        return Some(dir.into());
    }
    let home = || std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Ravune").join("data"))
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library").join("Application Support").join("Ravune"))
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| home().map(|h| h.join(".local").join("share"))).map(|d| d.join("ravune"))
    }
}

fn tools() -> Value {
    let object = |props: Value, required: &[&str]| json!({ "type": "object", "properties": props, "required": required });
    json!([
        { "name": "search_operators", "description": "Find Ravune operators (every edit is one) by words in their id, label or description. Returns ids, descriptions and example props.",
          "inputSchema": object(json!({ "query": { "type": "string", "description": "e.g. \"note\", \"mixer send\", \"tempo\"; empty lists all" } }), &[]) },
        { "name": "run_operator", "description": "Run one operator with JSON props (see search_operators for examples). Edits until the next checkpoint undo together with one Ctrl+Z.",
          "inputSchema": object(json!({ "id": { "type": "string" }, "props": { "type": "object" } }), &["id"]) },
        { "name": "query_project", "description": "Read the project: a summary (tempo, key, channels, patterns, mixer), one pattern's notes, the playlist clips, or everything.",
          "inputSchema": object(json!({ "what": { "type": "string", "enum": ["summary", "pattern", "clips", "full"] }, "index": { "type": "integer", "description": "pattern index for what=pattern (default: the current one)" } }), &[]) },
        { "name": "browse_sounds", "description": "Search the sound library (built-in kit, the user's folders) by name. Paths go into channel.add's `sample`.",
          "inputSchema": object(json!({ "query": { "type": "string" }, "limit": { "type": "integer" } }), &["query"]) },
        { "name": "render_and_analyze", "description": "Render bars offline and describe them: peak and RMS level, energy per band, stereo width, onsets (in beats), key, tempo.",
          "inputSchema": object(json!({ "bars": { "type": "number", "description": "default 4, at most 32" }, "from_bar": { "type": "number" }, "mode": { "type": "string", "enum": ["song", "pattern"] } }), &[]) },
        { "name": "checkpoint", "description": "Close the current undo step: later edits undo separately.", "inputSchema": object(json!({}), &[]) },
        { "name": "undo", "description": "Take back the last undo step (everything since the last checkpoint, if you edited).", "inputSchema": object(json!({}), &[]) },
        { "name": "save_project", "description": "Save the project to its file (only if the user allowed files in Preferences → Agents).", "inputSchema": object(json!({}), &[]) },
    ])
}

/// The line to Ravune, opened on the first tool call.
struct Ravune {
    lines: BufReader<TcpStream>,
    out: TcpStream,
    next: u64,
}

impl Ravune {
    fn connect() -> Result<Self, String> {
        let off = "Ravune isn't listening: open Ravune, then Preferences → Agents → \"Let agents use Ravune\"";
        let file = data_dir().ok_or(off)?.join("agent.json");
        let info: Value = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or(off)?;
        let port = info["port"].as_u64().ok_or(off)? as u16;
        let out = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("{off} ({e})"))?;
        let lines = BufReader::new(out.try_clone().map_err(|e| e.to_string())?);
        let mut ravune = Self { lines, out, next: 1 };
        ravune.call("hello", json!({ "token": info["token"] }))?;
        Ok(ravune)
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        writeln!(self.out, "{}", json!({ "id": id, "method": method, "params": params })).map_err(|e| e.to_string())?;
        let mut line = String::new();
        if self.lines.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("Ravune closed the connection".into());
        }
        let reply: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        match reply.get("error") {
            Some(e) => Err(e.as_str().map_or_else(|| e.to_string(), String::from)),
            None => Ok(reply["result"].clone()),
        }
    }
}

#[derive(Default)]
struct Bridge {
    ravune: Option<Ravune>,
}

impl Bridge {
    /// Forwards a tool call, reconnecting once if Ravune restarted.
    fn tool(&mut self, name: &str, args: Value) -> Result<Value, String> {
        for attempt in 0..2 {
            if self.ravune.is_none() {
                self.ravune = Some(Ravune::connect()?);
            }
            match self.ravune.as_mut().unwrap().call(name, args.clone()) {
                Err(e) if attempt == 0 && (e.contains("closed") || e.contains("os error")) => self.ravune = None,
                result => return result,
            }
        }
        Err("Ravune isn't answering".into())
    }

    /// One JSON-RPC message in, the reply out (none for notifications).
    fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id")?.clone();
        let result = match msg["method"].as_str().unwrap_or("") {
            "initialize" => {
                let asked = msg["params"]["protocolVersion"].as_str().unwrap_or("");
                let version = PROTOCOLS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOLS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "ravune", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("");
                let args = msg["params"].get("arguments").cloned().unwrap_or(json!({}));
                let (text, error) = match self.tool(name, args) {
                    Ok(v) => (serde_json::to_string_pretty(&v).unwrap_or_default(), false),
                    Err(e) => (e, true),
                };
                Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": error }))
            }
            other => Err(json!({ "code": -32601, "message": format!("no method '{other}'") })),
        };
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e }),
        })
    }
}

fn main() {
    let mut bridge = Bridge::default();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => bridge.handle(&msg),
            Err(e) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } })),
        };
        if let Some(reply) = reply
            && writeln!(stdout, "{reply}").and_then(|_| stdout.flush()).is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A fake Ravune: checks the token, then echoes each method and its params back.
    #[test]
    fn speaks_mcp_and_forwards_tool_calls() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!("ravune-mcp-test-{port}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agent.json"), json!({ "port": port, "token": "s3cret" }).to_string()).unwrap();
        // SAFETY: this test is the only one touching the environment
        unsafe { std::env::set_var("RAVUNE_DATA", &dir) };
        std::thread::spawn(move || {
            let (stream, _) = server.accept().unwrap();
            let mut out = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines() {
                let msg: Value = serde_json::from_str(&line.unwrap()).unwrap();
                let reply = if msg["method"] == "hello" {
                    assert_eq!(msg["params"]["token"], "s3cret");
                    json!({ "id": msg["id"], "result": {} })
                } else if msg["method"] == "undo" {
                    json!({ "id": msg["id"], "error": "nothing to undo" })
                } else {
                    json!({ "id": msg["id"], "result": { "method": msg["method"], "params": msg["params"] } })
                };
                writeln!(out, "{reply}").unwrap();
            }
        });
        let mut b = Bridge::default();
        let init = b.handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } })).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(b.handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none(), "no reply to a notification");
        let list = b.handle(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 8);
        let call = b.handle(&json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "run_operator", "arguments": { "id": "pattern.add" } } })).unwrap();
        assert_eq!(call["result"]["isError"], false);
        let text: Value = serde_json::from_str(call["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!((text["method"].as_str(), text["params"]["id"].as_str()), (Some("run_operator"), Some("pattern.add")));
        let failed = b.handle(&json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "undo" } })).unwrap();
        assert_eq!((failed["result"]["isError"].as_bool(), failed["result"]["content"][0]["text"].as_str()), (Some(true), Some("nothing to undo")));
        let unknown = b.handle(&json!({ "jsonrpc": "2.0", "id": 5, "method": "resources/list" })).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }
}
