//! The agent chat (Ctrl I): a full-height panel on the right where the user talks to Claude
//! Code, their own install, signed in on its own. Each message runs `claude -p` in the
//! background with Ravune's MCP bridge as its only tools: it reads and changes the open
//! project through the same commands as any agent (Preferences → Agents), and what one reply
//! changed undoes in one step.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui::{self, Align, CornerRadius, Frame, Key, KeyboardShortcut, Layout, Modifiers, RichText, Ui};
use serde_json::{Value, json};

use crate::theme::Theme;
use crate::{App, icons, kit};

/// What the agent is told besides the user's words.
const BRIEF: &str = "You are the assistant inside Ravune, the music app the user is looking at. Use the ravune tools to read and change the open project: the user hears every change at once and can undo each of your replies in one step. Answer briefly, in the user's language, in plain text without Markdown. Never write em dashes.";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Who {
    User,
    Agent,
    /// A command the agent ran.
    Tool,
    Problem,
}

pub struct Said {
    pub who: Who,
    pub text: String,
}

#[derive(Default)]
pub struct Chat {
    pub said: Vec<Said>,
    pub draft: String,
    /// Put the cursor in the box next frame (it was opened from the keyboard).
    pub focus: bool,
    /// Claude Code's session: the next message carries on with it.
    session: Option<String>,
    run: Option<Run>,
}

/// One reply on its way: the process and what its output said so far.
struct Run {
    child: Child,
    events: Receiver<Event>,
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = self.child.kill(); // stopped, or the chat closed; done already is fine too
        let _ = self.child.wait();
    }
}

#[derive(Debug, PartialEq)]
enum Event {
    Session(String),
    Said(Who, String),
}

/// One line of `claude -p --output-format stream-json`.
fn parse(line: &str) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    let mut out = Vec::new();
    match v["type"].as_str() {
        Some("system") if v["subtype"] == "init" => out.extend(v["session_id"].as_str().map(|id| Event::Session(id.into()))),
        Some("assistant") => {
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => out.extend(block["text"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(|s| Event::Said(Who::Agent, s.into()))),
                    Some("tool_use") => out.push(Event::Said(Who::Tool, tool(block))),
                    _ => {}
                }
            }
        }
        Some("result") if v["is_error"] == true => out.push(Event::Said(Who::Problem, v["result"].as_str().unwrap_or("Claude Code stopped").trim().into())),
        _ => {}
    }
    out
}

/// A command the agent ran, short: `mixer.set {"low":-3.0,"track":1}`, `query project`.
fn tool(block: &Value) -> String {
    let name = block["name"].as_str().unwrap_or("?");
    let name = name.strip_prefix("mcp__ravune__").unwrap_or(name);
    let input = &block["input"];
    match (name, input["id"].as_str(), &input["props"]) {
        ("run_operator", Some(id), Value::Null) => id.into(),
        ("run_operator", Some(id), props) => format!("{id} {props}"),
        _ => name.replace('_', " "),
    }
}

/// Claude Code, wherever the PATH has it (npm installs a .cmd on Windows), with no console.
fn spawn(args: &[&str]) -> std::io::Result<Child> {
    let names: &[&str] = if cfg!(windows) { &["claude", "claude.cmd"] } else { &["claude"] };
    let dir = crate::files::data_dir().filter(|d| d.is_dir()).unwrap_or_else(std::env::temp_dir);
    let mut last = std::io::Error::from(std::io::ErrorKind::NotFound);
    for name in names {
        let mut command = Command::new(name);
        command.args(args).current_dir(&dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        match command.spawn() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => last = e,
            other => return other,
        }
    }
    Err(last)
}

fn start(prompt: &str, session: Option<&str>, ctx: egui::Context) -> Result<Run, String> {
    let mcp = json!({ "mcpServers": { "ravune": { "command": crate::agent::bridge().to_string_lossy(), "args": [] } } }).to_string();
    // nothing but Ravune's commands; none of the user's own settings, hooks or MCP servers
    let mut args = vec!["-p", "--output-format", "stream-json", "--verbose", "--tools", "", "--setting-sources", "", "--strict-mcp-config", "--mcp-config", &mcp];
    args.extend(["--allowedTools", "mcp__ravune", "--append-system-prompt", BRIEF]);
    if let Some(id) = session {
        args.extend(["--resume", id]);
    }
    let mut child = spawn(&args).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => tr!("The chat runs through Claude Code, and it isn't installed here: get it at claude.com/claude-code, then sign in once by running claude in a terminal.").to_string(),
        _ => tr!("Claude Code didn't start: {}", e).to_string(),
    })?;
    let (stdin, stdout) = (child.stdin.take(), child.stdout.take().ok_or("no output")?);
    let prompt = prompt.to_string();
    let (tx, events) = channel();
    std::thread::Builder::new()
        .name("chat".into())
        .spawn(move || {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(prompt.as_bytes()); // closed when it drops: the end of the message
            }
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                for event in parse(&line) {
                    if tx.send(event).is_err() {
                        return;
                    }
                }
                ctx.request_repaint();
            }
            drop(tx); // done: the panel sees the channel close
            ctx.request_repaint();
        })
        .map_err(|e| e.to_string())?;
    Ok(Run { child, events })
}

impl App {
    /// Sends the draft. The agent server comes on if it was off: the chat goes through it.
    fn chat_send(&mut self, ctx: &egui::Context) {
        let text = self.chat.draft.trim().to_string();
        if text.is_empty() || self.chat.run.is_some() {
            return;
        }
        self.settings.agent.enabled = true;
        self.chat.draft.clear();
        self.chat.said.push(Said { who: Who::User, text: text.clone() });
        match start(&text, self.chat.session.as_deref(), ctx.clone()) {
            Ok(run) => self.chat.run = Some(run),
            Err(e) => self.chat.said.push(Said { who: Who::Problem, text: e }),
        }
    }

    /// Takes in what the reply on its way said (every frame, open or not).
    pub fn chat_poll(&mut self) {
        let Some(run) = &self.chat.run else { return };
        let (mut events, mut done) = (Vec::new(), false);
        loop {
            match run.events.try_recv() {
                Ok(e) => events.push(e),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    done = true;
                    break;
                }
            }
        }
        for event in events {
            match event {
                Event::Session(id) => self.chat.session = Some(id),
                Event::Said(who, text) => match self.chat.said.last_mut() {
                    // a failed reply says it once as text and once as the result
                    Some(last) if who == Who::Problem && last.who == Who::Agent && last.text == text => last.who = Who::Problem,
                    _ => self.chat.said.push(Said { who, text }),
                },
            }
        }
        if done {
            self.chat.run = None;
            if self.chat.said.last().is_some_and(|s| s.who == Who::User) {
                self.chat.said.push(Said { who: Who::Problem, text: tr!("Claude Code stopped without an answer. Try running claude in a terminal to see why.").to_string() });
            }
        }
    }

    /// The chat panel: the conversation, and a box to write in at the bottom.
    pub fn chat_panel(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        kit::bar(ui, &t, |ui| {
            ui.add_space(t.pad);
            ui.label(RichText::new(format!("{}  {}", icons::ROBOT, tr!("Agent"))).color(t.fg_text));
            kit::right(ui, |ui| {
                let close = self.tip(tr!("Close the chat"), "app.chat");
                if kit::icon_button(ui, &t, icons::X, &close, false).clicked() {
                    self.settings.show_chat = false;
                }
                if !self.chat.said.is_empty() && kit::icon_button(ui, &t, icons::PLUS, tr!("New chat"), false).clicked() {
                    self.chat = Chat::default(); // stops a reply on its way, too
                }
            });
        });
        egui::Panel::bottom("chat-input").frame(Frame::new().fill(t.bg_panel).inner_margin(t.pad)).show(ui, |ui| self.chat_input(ui, &t));
        egui::ScrollArea::vertical().id_salt("chat").auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
            Frame::new().inner_margin(t.pad).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = t.gap * 2.0;
                if self.chat.said.is_empty() {
                    ui.label(RichText::new(tr!("Ask for a change and Claude makes it in your project: \"make the kick punchier\", \"add a reverb bus for the snare\", \"write a bassline in A minor\".")).color(t.fg_muted));
                    ui.label(RichText::new(tr!("It runs your own Claude Code with nothing but Ravune's commands. Each reply undoes in one step (Ctrl Z).")).small().color(t.fg_muted));
                }
                let width = ui.available_width();
                for said in &self.chat.said {
                    match said.who {
                        Who::User => {
                            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                                Frame::new().fill(t.bg_device).corner_radius(CornerRadius::same(6)).inner_margin(8).show(ui, |ui| {
                                    ui.set_max_width(width * 0.85);
                                    ui.add(egui::Label::new(RichText::new(&said.text).color(t.fg_text)).wrap().selectable(true));
                                });
                            });
                        }
                        Who::Agent => {
                            ui.add(egui::Label::new(RichText::new(&said.text).color(t.fg_text)).wrap().selectable(true));
                        }
                        Who::Tool => {
                            ui.add(egui::Label::new(RichText::new(format!("{}  {}", icons::ARROW_BEND_DOWN_RIGHT, said.text)).font(Theme::mono(10.0)).color(t.fg_muted)).truncate());
                        }
                        Who::Problem => {
                            ui.add(egui::Label::new(RichText::new(format!("{}  {}", icons::WARNING, said.text)).color(t.meter_hot)).wrap().selectable(true));
                        }
                    }
                }
                if self.chat.run.is_some() {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0).color(t.accent));
                        ui.label(RichText::new(tr!("Working…")).small().color(t.fg_muted));
                    });
                }
            });
        });
    }

    /// The box at the bottom: Enter sends, Shift+Enter starts a new line.
    fn chat_input(&mut self, ui: &mut Ui, t: &Theme) {
        let running = self.chat.run.is_some();
        let edit = egui::TextEdit::multiline(&mut self.chat.draft)
            .hint_text(tr!("Ask Claude to change the song…"))
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter));
        let r = ui.add(edit);
        if std::mem::take(&mut self.chat.focus) {
            r.request_focus();
        }
        let enter = r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr!("Claude Code · Enter sends")).small().color(t.fg_muted));
            kit::right(ui, |ui| {
                if running {
                    if kit::icon_button(ui, t, icons::STOP, tr!("Stop"), false).clicked() {
                        self.chat.run = None;
                        self.chat.said.push(Said { who: Who::Problem, text: tr!("Stopped").to_string() });
                    }
                } else if kit::icon_button(ui, t, icons::ARROW_CIRCLE_UP, tr!("Send (Enter)"), false).clicked() || enter {
                    self.chat_send(ui.ctx());
                    r.request_focus();
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_json_becomes_the_conversation() {
        let lines = [
            r#"{"type":"system","subtype":"hook_started","session_id":"s1"}"#,
            r#"{"type":"system","subtype":"init","session_id":"s1","tools":[]}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":" Sure. "},{"type":"tool_use","name":"mcp__ravune__run_operator","input":{"id":"mixer.set","props":{"low":-3.0,"track":1}}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"mcp__ravune__query_project","input":{}},{"type":"text","text":"  "}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Sure.","session_id":"s1"}"#,
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate","session_id":"s1"}"#,
            "not json",
        ];
        let events: Vec<Event> = lines.iter().flat_map(|l| parse(l)).collect();
        assert_eq!(
            events,
            vec![
                Event::Session("s1".into()),
                Event::Said(Who::Agent, "Sure.".into()),
                Event::Said(Who::Tool, r#"mixer.set {"low":-3.0,"track":1}"#.into()),
                Event::Said(Who::Tool, "query project".into()),
                Event::Said(Who::Problem, "Failed to authenticate".into()),
            ]
        );
    }
}
