//! MIDI input: connects to every MIDI keyboard, re-scanning each second for hot-plug.
//! Channel messages go straight to the audio thread (no UI-frame latency), with their
//! channel for MPE; controller moves (CCs) are also kept for MIDI learn and mapped knobs.

use std::sync::{Arc, Mutex};

use eframe::egui;
use ravune_engine::{MidiSink, Msg};
use midir::{MidiInput, MidiInputConnection};

/// (channel 0-15, controller, value) as they arrive, for the UI.
pub type Ccs = Arc<Mutex<Vec<(u8, u8, u8)>>>;

#[derive(Default)]
pub struct Midi {
    connections: Vec<(String, MidiInputConnection<()>)>,
    last_scan: f64,
    pub ccs: Ccs,
}

impl Midi {
    pub fn devices(&self) -> impl Iterator<Item = &str> {
        self.connections.iter().map(|(name, _)| name.as_str())
    }

    pub fn poll(&mut self, now: f64, sink: &MidiSink, ctx: &egui::Context) {
        if now - self.last_scan < 1.0 && self.last_scan > 0.0 {
            return;
        }
        self.last_scan = now;
        let Ok(scanner) = MidiInput::new("Ravune") else { return };
        let ports: Vec<_> = scanner.ports().into_iter().filter_map(|p| Some((scanner.port_name(&p).ok()?, p))).collect();
        // unplugged devices: dropping the connection closes it
        self.connections.retain(|(name, _)| ports.iter().any(|(n, _)| n == name));
        for (name, port) in ports {
            if self.connections.iter().any(|(n, _)| *n == name) {
                continue;
            }
            let Ok(input) = MidiInput::new("Ravune") else { continue };
            let (sink, ctx, ccs) = (sink.clone(), ctx.clone(), self.ccs.clone());
            let on_message = move |_: u64, bytes: &[u8], _: &mut ()| {
                if let Some(msg) = parse(bytes) {
                    if let Some(tx) = sink.lock().unwrap().as_mut() {
                        let _ = tx.push(msg);
                    }
                    if let [status, cc, value] = *bytes
                        && status & 0xF0 == 0xB0
                        && cc < 120
                        && let Ok(mut q) = ccs.lock()
                    {
                        q.push((status & 15, cc, value));
                    }
                    ctx.request_repaint(); // light up the on-screen keys, move mapped knobs
                }
            };
            // A port another app holds open fails here; we simply retry on the next scan.
            if let Ok(conn) = input.connect(&port, "ravune-in", on_message, ()) {
                self.connections.push((name, conn));
            }
        }
    }
}

/// Notes, poly / channel pressure, CCs and pitch bend, with their channel.
fn parse(bytes: &[u8]) -> Option<Msg> {
    match *bytes {
        [status, a, b] if matches!(status & 0xF0, 0x80 | 0x90 | 0xA0 | 0xB0 | 0xE0) => Some(Msg::Midi([status, a & 127, b & 127])),
        [status, a] if status & 0xF0 == 0xD0 => Some(Msg::Midi([status, a & 127, 0])),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_channel_messages_with_their_channel() {
        assert!(matches!(parse(&[0x93, 60, 127]), Some(Msg::Midi([0x93, 60, 127]))));
        assert!(matches!(parse(&[0xE1, 0, 64]), Some(Msg::Midi([0xE1, 0, 64]))), "pitch bend");
        assert!(matches!(parse(&[0xD2, 90]), Some(Msg::Midi([0xD2, 90, 0]))), "channel pressure is two bytes");
        assert!(parse(&[0xC0, 5]).is_none(), "program change ignored");
        assert!(parse(&[0xF8]).is_none(), "clock ignored");
    }
}
