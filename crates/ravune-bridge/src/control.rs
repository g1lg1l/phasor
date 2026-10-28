//! The control channel: JSON lines over the host's stdin / stdout, each prefixed with
//! [`MARK`] so whatever a plugin prints to stdout can't be mistaken for a reply.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command as Process, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::{Bridge, MAX_FRAMES};

/// Starts every protocol line.
pub const MARK: &str = "\u{1}ravune ";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Clap,
    Vst3,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Clap => "CLAP",
            Format::Vst3 => "VST3",
        }
    }
}

/// A plugin a scan found in a file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub format: Format,
    pub path: String,
    /// CLAP id or VST3 class id (hex).
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub instrument: bool,
    /// Categories as the plugin gives them ("synthesizer", "Fx|Delay", …).
    pub features: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    /// Group path, e.g. "Oscillator 1".
    pub module: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub value: f64,
    pub stepped: bool,
}

/// Ravune → host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    Load { format: Format, path: String, id: String, rate: f64, max_frames: u32, shm: String, state: Option<String> },
    /// Opens or closes the plugin's own editor window.
    Gui { show: bool, title: String },
    State,
    SetState { data: String },
    Params,
    SetParam { id: u32, value: f64 },
    Quit,
}

/// Host → Ravune.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    Loaded { name: String, instrument: bool },
    State { data: String },
    Params { params: Vec<ParamInfo> },
    /// The plugin (or its editor) moved a parameter.
    Param { id: u32, value: f64 },
    GuiClosed,
    Ok,
    Error { message: String },
}

impl Reply {
    /// Sent on its own, not as an answer to a command.
    fn is_event(&self) -> bool {
        matches!(self, Reply::Param { .. } | Reply::GuiClosed)
    }
}

pub fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode(text: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD.decode(text).map_err(|e| e.to_string())
}

/// Writes one protocol line (the host's side).
pub fn send<T: Serialize>(out: &mut impl Write, message: &T) -> std::io::Result<()> {
    let line = serde_json::to_string(message).map_err(std::io::Error::other)?;
    writeln!(out, "{MARK}{line}")?;
    out.flush()
}

/// Reads one protocol line, skipping anything else on the stream; `None` at its end.
pub fn receive<T: for<'de> Deserialize<'de>>(input: &mut impl BufRead) -> Option<Result<T, String>> {
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if let Some(json) = line.trim_end().strip_prefix(MARK) {
            return Some(serde_json::from_str(json).map_err(|e| e.to_string()));
        }
    }
}

fn spawn(host: &Path, args: &[&str]) -> std::io::Result<Child> {
    let mut command = Process::new(host);
    command.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command.spawn()
}

/// What's in a plugin file, asked of a throwaway host process: a plugin that crashes or
/// hangs while being scanned can't take Ravune with it.
pub fn scan(host: &Path, file: &Path, timeout: Duration) -> Result<Vec<Found>, String> {
    let mut child = spawn(host, &["--scan", &file.to_string_lossy()]).map_err(|e| format!("can't start {}: {e}", host.display()))?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(receive::<Vec<Found>>(&mut BufReader::new(stdout)));
    });
    let result = match rx.recv_timeout(timeout) {
        Ok(Some(found)) => found,
        Ok(None) => Err("the plugin crashed while being scanned".into()),
        Err(_) => Err(format!("no answer in {} s", timeout.as_secs())),
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// A running host process with one plugin loaded.
pub struct Hosted {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<Reply>,
    /// Events that arrived while waiting for an answer.
    events: Vec<Reply>,
    /// Answers still owed for commands sent without waiting ([`Self::send`]).
    owed: usize,
    pub bridge: Arc<Bridge>,
    shm: PathBuf,
    pub name: String,
    pub instrument: bool,
}

impl Hosted {
    /// Starts `host`, loads the plugin into it and waits until it's ready.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(host: &Path, format: Format, path: &str, id: &str, rate: f64, state: Option<String>, timeout: Duration) -> Result<Self, String> {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let shm = std::env::temp_dir().join(format!("ravune-{}-{n}.shm", std::process::id()));
        let bridge = Arc::new(Bridge::create(&shm).map_err(|e| format!("shared memory: {e}"))?);
        let mut child = spawn(host, &[]).map_err(|e| format!("can't start {}: {e}", host.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, replies) = channel();
        std::thread::Builder::new()
            .name("plugin-replies".into())
            .spawn(move || {
                let mut out = BufReader::new(stdout);
                while let Some(Ok(reply)) = receive::<Reply>(&mut out) {
                    if tx.send(reply).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let mut hosted = Self { child, stdin, replies, events: Vec::new(), owed: 0, bridge, shm, name: String::new(), instrument: false };
        let load = Command::Load { format, path: path.into(), id: id.into(), rate, max_frames: MAX_FRAMES as u32, shm: hosted.shm.to_string_lossy().into_owned(), state };
        match hosted.ask(&load, timeout)? {
            Reply::Loaded { name, instrument } => {
                (hosted.name, hosted.instrument) = (name, instrument);
                Ok(hosted)
            }
            Reply::Error { message } => Err(message),
            other => Err(format!("unexpected answer {other:?}")),
        }
    }

    /// Sends without waiting; its answer is skipped when it comes.
    pub fn send(&mut self, command: &Command) -> Result<(), String> {
        send(&mut self.stdin, command).map_err(|_| "the plugin process is gone".to_string())?;
        if *command != Command::Quit {
            self.owed += 1;
        }
        Ok(())
    }

    /// Sends `command` and waits for its answer (events arriving meanwhile are kept).
    pub fn ask(&mut self, command: &Command, timeout: Duration) -> Result<Reply, String> {
        self.send(command)?;
        let deadline = Instant::now() + timeout;
        loop {
            match self.replies.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(reply) if reply.is_event() => self.events.push(reply),
                Ok(reply) => {
                    self.owed = self.owed.saturating_sub(1);
                    if self.owed == 0 {
                        return Ok(reply); // the earlier ones answered fire-and-forget sends
                    }
                }
                Err(RecvTimeoutError::Timeout) => return Err(format!("no answer in {} s", timeout.as_secs())),
                Err(RecvTimeoutError::Disconnected) => {
                    self.bridge.kill();
                    return Err("the plugin process crashed".into());
                }
            }
        }
    }

    /// Events since the last call (parameter moves, the editor closing).
    pub fn poll(&mut self) -> Vec<Reply> {
        let mut out = std::mem::take(&mut self.events);
        for reply in self.replies.try_iter() {
            if reply.is_event() {
                out.push(reply);
            } else {
                self.owed = self.owed.saturating_sub(1);
            }
        }
        out
    }

    /// False once the process has exited (then the bridge stops waiting for it).
    pub fn running(&mut self) -> bool {
        let alive = matches!(self.child.try_wait(), Ok(None));
        if !alive {
            self.bridge.kill();
        }
        alive
    }

    /// For tests and "the plugin is stuck": end the process now.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.bridge.kill();
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        self.bridge.kill();
        let _ = send(&mut self.stdin, &Command::Quit);
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline && matches!(self.child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.shm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_lines_skip_plugin_noise() {
        let mut out = Vec::new();
        send(&mut out, &Reply::Param { id: 7, value: 0.5 }).unwrap();
        let mut stream = b"plugin says hi\n".to_vec();
        stream.extend(out);
        let mut reader = BufReader::new(&stream[..]);
        assert_eq!(receive::<Reply>(&mut reader), Some(Ok(Reply::Param { id: 7, value: 0.5 })));
        assert_eq!(receive::<Reply>(&mut reader), None);
        let load = Command::Load { format: Format::Clap, path: "a.clap".into(), id: "x".into(), rate: 48000.0, max_frames: 256, shm: "s".into(), state: None };
        let text = serde_json::to_string(&load).unwrap();
        assert!(text.contains(r#""op":"load""#) && text.contains(r#""format":"clap""#), "{text}");
        assert_eq!(decode(&encode(b"\x00state\xff")).unwrap(), b"\x00state\xff");
    }
}
