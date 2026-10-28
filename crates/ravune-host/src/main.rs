//! ravune-host: runs one CLAP or VST3 plugin for Ravune, in its own process, so a crash
//! takes only the plugin down.
//!
//!     ravune-host --scan <plugin file>   prints what's in the file and exits
//!     ravune-host                        serves one plugin: commands on stdin (JSON lines,
//!                                        see ravune-bridge), audio over shared memory
//!
//! Threads: the main thread owns the plugin (its main-thread calls, timers and editor
//! window); an audio thread answers the engine's blocks; a reader thread turns stdin
//! into commands. If Ravune goes away, stdin closes and the host quits.

mod clap;
mod vst3;
mod window;

use std::io::{BufReader, stdout};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use ravune_bridge::control::{receive, send};
use ravune_bridge::{Block, Command, Format, Found, ParamInfo, Reply, Server, decode, encode};

/// A plugin loaded on the main thread.
enum Loaded {
    Clap(clap::Clap),
    Vst3(Box<vst3::Vst3>),
}

/// Its audio half, moved to the audio thread.
enum Audio {
    Clap(Box<clap::Audio>),
    Vst3(Box<vst3::Audio>),
}

impl Audio {
    fn process(&mut self, block: Block<'_>) {
        match self {
            Audio::Clap(a) => a.process(block),
            Audio::Vst3(a) => a.process(block),
        }
    }

    /// CLAP wants processing stopped on the audio thread (VST3 stops in `finish`).
    fn stop(&mut self) {
        if let Audio::Clap(a) = self {
            a.stop();
        }
    }
}

impl Loaded {
    fn name(&self) -> String {
        match self {
            Loaded::Clap(p) => p.name.clone(),
            Loaded::Vst3(p) => p.name.clone(),
        }
    }

    fn instrument(&self) -> bool {
        match self {
            Loaded::Clap(p) => p.instrument,
            Loaded::Vst3(p) => p.instrument,
        }
    }

    fn audio(&mut self) -> Result<Audio, String> {
        match self {
            Loaded::Clap(p) => p.audio().map(|a| Audio::Clap(Box::new(a))),
            Loaded::Vst3(p) => p.audio().map(|a| Audio::Vst3(Box::new(a))),
        }
    }

    fn finish(&mut self, audio: Audio) {
        match (self, audio) {
            (Loaded::Clap(p), Audio::Clap(a)) => p.finish(*a),
            (Loaded::Vst3(p), Audio::Vst3(a)) => p.finish(*a),
            _ => {}
        }
    }

    fn params(&mut self) -> Vec<ParamInfo> {
        match self {
            Loaded::Clap(p) => p.params(),
            Loaded::Vst3(p) => p.params(),
        }
    }

    fn set_param(&mut self, id: u32, value: f64) {
        match self {
            Loaded::Clap(p) => p.set_param(id, value),
            Loaded::Vst3(p) => p.set_param(id, value),
        }
    }

    fn save(&mut self) -> Result<Vec<u8>, String> {
        match self {
            Loaded::Clap(p) => p.save_state(),
            Loaded::Vst3(p) => p.save_state(),
        }
    }

    fn load(&mut self, data: &[u8]) -> Result<(), String> {
        match self {
            Loaded::Clap(p) => p.load_state(data),
            Loaded::Vst3(p) => p.load_state(data),
        }
    }

    fn open_editor(&mut self, title: &str) -> Result<(), String> {
        match self {
            Loaded::Clap(p) => p.open_editor(title),
            Loaded::Vst3(p) => p.open_editor(title),
        }
    }

    fn close_editor(&mut self) {
        match self {
            Loaded::Clap(p) => p.close_editor(),
            Loaded::Vst3(p) => p.close_editor(),
        }
    }

    /// Main-thread housekeeping; returns parameter moves and whether the editor was closed.
    fn idle(&mut self) -> (Vec<(u32, f64)>, bool) {
        match self {
            Loaded::Clap(p) => p.idle(),
            Loaded::Vst3(p) => p.idle(),
        }
    }
}

fn load(format: Format, path: &str, id: &str, rate: f64, max_frames: u32) -> Result<Loaded, String> {
    match format {
        Format::Clap => clap::Clap::load(Path::new(path), id, rate, max_frames).map(Loaded::Clap),
        Format::Vst3 => vst3::Vst3::load(Path::new(path), id, rate, max_frames).map(|p| Loaded::Vst3(Box::new(p))),
    }
}

fn scan(path: &str) -> Result<Vec<Found>, String> {
    let p = Path::new(path);
    match p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("clap") => clap::scan(p),
        Some("vst3") => vst3::scan(p),
        _ => Err(format!("{path}: not a .clap or .vst3 plugin")),
    }
}

fn reply(message: &Reply) {
    let _ = send(&mut stdout().lock(), message);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, path] = args.as_slice()
        && flag == "--scan"
    {
        let found = scan(path).unwrap_or_else(|e| {
            eprintln!("ravune-host: {e}");
            Vec::new()
        });
        let _ = send(&mut stdout().lock(), &found);
        return;
    }
    serve();
}

fn serve() {
    window::init();
    let (tx, commands) = channel();
    std::thread::spawn(move || {
        let mut input = BufReader::new(std::io::stdin());
        while let Some(Ok(command)) = receive::<Command>(&mut input) {
            if tx.send(command).is_err() {
                return;
            }
        }
        let _ = tx.send(Command::Quit); // Ravune is gone
    });
    let Ok(Command::Load { format, path, id, rate, max_frames, shm, state }) = commands.recv() else { return };
    let server = match Server::open(Path::new(&shm)) {
        Ok(s) => s,
        Err(e) => return reply(&Reply::Error { message: format!("shared memory: {e}") }),
    };
    let mut plugin = match load(format, &path, &id, rate, max_frames) {
        Ok(p) => p,
        Err(message) => return reply(&Reply::Error { message }),
    };
    if let Some(data) = state
        && let Err(e) = decode(&data).and_then(|d| plugin.load(&d))
    {
        eprintln!("ravune-host: couldn't restore the state: {e}");
    }
    let audio = match plugin.audio() {
        Ok(a) => a,
        Err(message) => return reply(&Reply::Error { message }),
    };
    let quit = Arc::new(AtomicBool::new(false));
    let audio_thread = {
        let quit = quit.clone();
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                let (mut server, mut audio) = (server, audio);
                while !quit.load(Relaxed) {
                    if let Some(block) = server.wait(Duration::from_millis(50)) {
                        audio.process(block);
                        server.finish();
                    }
                }
                audio.stop();
                audio
            })
            .expect("spawn the audio thread")
    };
    reply(&Reply::Loaded { name: plugin.name(), instrument: plugin.instrument() });

    loop {
        window::pump();
        let (moved, closed) = plugin.idle();
        for (id, value) in moved {
            reply(&Reply::Param { id, value });
        }
        if closed {
            plugin.close_editor();
            reply(&Reply::GuiClosed);
        }
        let command = match commands.recv_timeout(Duration::from_millis(10)) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match command {
            Command::Quit => break,
            Command::Load { .. } => reply(&Reply::Error { message: "already loaded".into() }),
            Command::Gui { show: true, title } => match plugin.open_editor(&title) {
                Ok(()) => reply(&Reply::Ok),
                Err(message) => reply(&Reply::Error { message }),
            },
            Command::Gui { show: false, .. } => {
                plugin.close_editor();
                reply(&Reply::Ok);
            }
            Command::State => match plugin.save() {
                Ok(data) => reply(&Reply::State { data: encode(&data) }),
                Err(message) => reply(&Reply::Error { message }),
            },
            Command::SetState { data } => match decode(&data).and_then(|d| plugin.load(&d)) {
                Ok(()) => reply(&Reply::Ok),
                Err(message) => reply(&Reply::Error { message }),
            },
            Command::Params => reply(&Reply::Params { params: plugin.params() }),
            Command::SetParam { id, value } => {
                plugin.set_param(id, value);
                reply(&Reply::Ok);
            }
        }
    }
    plugin.close_editor();
    quit.store(true, Relaxed);
    if let Ok(audio) = audio_thread.join() {
        plugin.finish(audio);
    }
}
