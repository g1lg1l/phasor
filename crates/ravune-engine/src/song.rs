//! Song: the project compiled for the audio thread: sorted note events per pattern,
//! resolved samples, playlist clips, the mixer graph and modulation. Built on the UI
//! thread, never mutated after.

use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use ravune_core::{
    BAR, ClipKind, DeviceKind, Instrument, Lfo, MACROS, MAX_DEVICE_PARAMS, MAX_LFOS, MAX_TRACKS, PARAM_COUNT, Project, STEP, Source,
    Target,
};

use crate::{MultiMap, Sample};

/// Effect instances the audio thread can hold.
pub const MAX_EFFECTS: usize = 256;

pub enum Sound {
    /// `None` when the file couldn't be loaded: the channel stays silent.
    Sample(Option<Arc<Sample>>),
    Synth([f32; PARAM_COUNT]),
    /// A CLAP / VST3 instrument; its bridge arrives in a [`crate::Msg::Plugin`].
    Plugin,
    /// A multi-sample instrument; `None` when its file couldn't be read.
    /// ponytail: re-read on every compile (fine for ~1000 regions); cache by path if a huge library lags.
    Multi(Option<MultiMap>),
}

pub struct SongChannel {
    pub slot: u8,
    pub sound: Sound,
    pub gain: f32,
    pub mute: bool,
    pub pitch: f32,
    pub choke: u8,
    /// Samplers: fade-in and (when not 0) fade-out after the note, in seconds.
    pub attack: f32,
    pub release: f32,
    /// Samplers: the part of the sound each hit plays (fractions of its length), and
    /// whether it repeats while the note is held.
    pub start: f32,
    pub end: f32,
    pub looped: bool,
    /// Mixer track it plays into.
    pub track: usize,
    /// A synth with modulation mapped onto it (its params are recomputed every block).
    pub modulated: bool,
}

pub struct SongDevice {
    pub id: u32,
    pub kind: DeviceKind,
    pub params: [f32; MAX_DEVICE_PARAMS],
    pub bypass: bool,
    /// Sidechain: the track whose (post-effects, pre-fader) sound drives it.
    pub key: Option<usize>,
}

pub struct SongTrack {
    pub gain: f32,
    pub pan: f32,
    /// The strip's EQ: low, mid, high in dB.
    pub eq: [f32; 3],
    pub mute: bool,
    /// (track, level): the route at full level, then the sends. Empty for the master.
    pub outputs: Vec<(usize, f32)>,
    /// Its chain in [`Song::devices`].
    pub devices: Range<usize>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ModTarget {
    Synth { slot: u8, param: usize },
    /// Index into [`Song::devices`].
    Device { index: usize, param: usize },
    Gain(usize),
    Pan(usize),
    /// By slot.
    ChannelGain(u8),
    Macro(usize),
}

#[derive(Clone, Copy)]
pub struct SongMod {
    pub source: Source,
    pub target: ModTarget,
    pub amount: f32,
}

/// An automation clip: sets its knob while the song plays over it.
pub struct SongAuto {
    pub start: f64,
    pub end: f64,
    pub offset: f64,
    pub target: ModTarget,
    pub points: Vec<(u32, f32)>,
    pub curves: Vec<f32>,
}

/// A note on (`velocity > 0`) or off, in ticks from the pattern start.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub time: f64,
    /// Index into [`Song::channels`].
    pub channel: u16,
    pub key: u8,
    pub velocity: u8,
    /// The note's own fine pitch (semitones) and brightness (0.5 = as patched).
    pub fine: f32,
    pub bright: f32,
}

/// A knob's line in a pattern: which knob, its points (ticks from the pattern's start), its bends.
pub type PatternLine = (ModTarget, Vec<(u32, f32)>, Vec<f32>);

pub struct SongPattern {
    pub length: f64,
    /// Knobs moving along with it.
    pub autos: Vec<PatternLine>,
    /// Sorted by time; at equal times note-offs come first.
    pub events: Vec<Event>,
    /// Bit per channel slot that has data here (to release its notes when a clip ends).
    pub slots: u64,
}

pub enum Content {
    Pattern(usize),
    Audio(Arc<Sample>),
}

pub struct SongClip {
    pub start: f64,
    pub end: f64,
    pub offset: f64,
    pub content: Content,
    /// Gain, pitch, fades and envelopes (audio clips).
    pub fx: ravune_core::ClipFx,
    /// `fx.gain` as a factor.
    pub gain: f32,
    /// The mixer track it plays into (an existing one; else the master).
    pub track: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Loop one pattern (FL "PAT").
    Pattern(usize),
    /// Play the playlist (FL "SONG").
    Song,
}

/// What the transport is doing; everything else comes from the project.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transport {
    pub playing: bool,
    pub mode: Mode,
    /// Song-mode start position in ticks.
    pub start: u32,
    /// Bumped to move a playing song to `start` right away.
    pub jump: u32,
    /// Song mode plays only this part (ticks), over and over.
    pub looped: Option<(u32, u32)>,
}

pub struct Song {
    pub tempo: f32,
    pub metronome: bool,
    pub playing: bool,
    /// Off only for offline export: play once, then let tails ring out.
    pub looping: bool,
    pub mode: Mode,
    pub start: f64,
    /// Changes when the transport asks to jump to `start` while playing.
    pub jump: u32,
    /// Song length in ticks (end of the last clip, whole bars).
    pub end: f64,
    /// The loop region (song mode, ticks), if any.
    pub looped: Option<(f64, f64)>,
    pub channels: Vec<SongChannel>,
    pub patterns: Vec<SongPattern>,
    /// Sorted by start.
    pub clips: Vec<SongClip>,
    /// Index 0 is the master.
    pub tracks: Vec<SongTrack>,
    /// Tracks in processing order: every track after the ones feeding it, the master last.
    pub order: Vec<usize>,
    pub devices: Vec<SongDevice>,
    pub macros: [f32; MACROS],
    pub lfos: Vec<Lfo>,
    pub mods: Vec<SongMod>,
    pub autos: Vec<SongAuto>,
}

impl Song {
    /// Compiles a project. `sample` resolves a sample path (the UI's cache keeps every
    /// `Arc<Sample>` alive, which is what makes dropping them on the audio thread free).
    pub fn compile(project: &Project, transport: Transport, mut sample: impl FnMut(&str) -> Option<Arc<Sample>>) -> Self {
        let (mut tracks, mut devices) = (Vec::new(), Vec::new());
        let n = project.mixer.len().clamp(1, MAX_TRACKS);
        let heard = project.tracks_heard();
        for (i, t) in project.mixer.iter().take(n).enumerate() {
            let start = devices.len();
            for d in t.devices.iter().take(MAX_EFFECTS - start) {
                let mut params = [0.0; MAX_DEVICE_PARAMS];
                params.iter_mut().zip(&d.params).for_each(|(p, v)| *p = *v);
                let key = d.sidechain.filter(|&k| k < n && k != i);
                devices.push(SongDevice { id: d.id, kind: d.kind, params, bypass: d.bypass, key });
            }
            let outputs = if i == 0 {
                Vec::new()
            } else {
                std::iter::once((t.route, 1.0)).chain(t.sends.iter().map(|s| (s.to, s.amount))).filter(|(to, _)| *to < n).collect()
            };
            tracks.push(SongTrack { gain: t.gain, pan: t.pan, eq: t.eq, mute: t.mute || !heard[i], outputs, devices: start..devices.len() });
        }
        if tracks.is_empty() {
            tracks.push(SongTrack { gain: 1.0, pan: 0.0, eq: [0.0; 3], mute: false, outputs: Vec::new(), devices: 0..0 });
        }

        // a knob of the project in the song's terms (None when it's gone)
        let mod_target = |target: Target| match target {
            Target::Synth { slot, param } => Some(ModTarget::Synth { slot, param: param as usize }),
            Target::Device { id, param } => {
                let index = devices.iter().position(|d: &SongDevice| d.id == id)?;
                Some(ModTarget::Device { index, param: param.min(MAX_DEVICE_PARAMS - 1) })
            }
            Target::TrackGain { track } if track < tracks.len() => Some(ModTarget::Gain(track)),
            Target::TrackPan { track } if track < tracks.len() => Some(ModTarget::Pan(track)),
            Target::ChannelGain { slot } if project.channels.iter().any(|c| c.slot == slot) => Some(ModTarget::ChannelGain(slot)),
            Target::Macro { index } if (index as usize) < MACROS => Some(ModTarget::Macro(index as usize)),
            _ => None,
        };
        let autos: Vec<SongAuto> = project
            .clips
            .iter()
            .filter(|c| !c.fx.mute && project.lane_plays(c.lane))
            .filter_map(|c| match &c.kind {
                ClipKind::Automation { target, points, curves } if !points.is_empty() => Some(SongAuto {
                    start: c.start as f64,
                    end: c.end() as f64,
                    offset: c.offset as f64,
                    target: mod_target(*target)?,
                    points: points.clone(),
                    curves: curves.clone(),
                }),
                _ => None,
            })
            .collect();
        let mods: Vec<SongMod> = project
            .mappings
            .iter()
            .filter_map(|m| {
                // a macro moved by a macro or an LFO: not a thing (it could loop)
                let target = mod_target(m.target).filter(|t| !matches!(t, ModTarget::Macro(_)))?;
                let valid = match m.source {
                    Source::Macro(i) => (i as usize) < MACROS,
                    Source::Lfo(i) => (i as usize) < project.lfos.len().min(MAX_LFOS),
                };
                valid.then_some(SongMod { source: m.source, target, amount: m.amount })
            })
            .collect();

        let channels: Vec<SongChannel> = project
            .channels
            .iter()
            .map(|c| SongChannel {
                slot: c.slot,
                sound: match &c.instrument {
                    Instrument::Sampler { sample: path } => Sound::Sample(sample(path)),
                    Instrument::Synth { params } => Sound::Synth(*params),
                    Instrument::Multi { file } => Sound::Multi(MultiMap::load(Path::new(file), &mut sample).ok()),
                    Instrument::Plugin { .. } => Sound::Plugin,
                },
                gain: c.gain,
                mute: c.mute,
                pitch: c.pitch,
                choke: c.choke,
                attack: c.attack,
                release: c.release,
                start: c.start,
                end: c.end,
                looped: c.looped,
                track: if c.track < tracks.len() { c.track } else { 0 },
                modulated: mods
                    .iter()
                    .map(|m| m.target)
                    .chain(autos.iter().map(|a| a.target))
                    .chain(project.patterns.iter().flat_map(|p| p.automation.iter().filter_map(|a| mod_target(a.target))))
                    .any(|t| matches!(t, ModTarget::Synth { slot, .. } if slot == c.slot)),
            })
            .collect();

        let patterns = project
            .patterns
            .iter()
            .map(|p| {
                let length = p.ticks() as f64;
                let (mut events, mut slots) = (Vec::new(), 0u64);
                let mut note = |time: f64, channel: usize, key: u8, velocity: u8, fine: f32, bright: f32| {
                    events.push(Event { time, channel: channel as u16, key, velocity, fine, bright });
                };
                for (ci, ch) in project.channels.iter().enumerate() {
                    // samplers apply the channel's pitch at trigger; synths get it here (whole
                    // semitones on the key, cents on the note's fine pitch), plugins rounded
                    let (shift, cents) = match ch.instrument {
                        Instrument::Synth { .. } => (ch.pitch.trunc(), ch.pitch.fract()),
                        Instrument::Plugin { .. } => (ch.pitch.round(), 0.0),
                        Instrument::Sampler { .. } | Instrument::Multi { .. } => (0.0, 0.0),
                    };
                    let swing = ch.swing.unwrap_or(project.swing) as f64 * STEP as f64 / 3.0;
                    for n in p.notes(ch.slot).iter().filter(|n| (n.start as f64) < length) {
                        let key = (n.key as f32 + shift).clamp(0.0, 127.0) as u8;
                        // swing pushes whatever starts on an off-beat 16th
                        let late = if n.start % (2 * STEP) == STEP { swing } else { 0.0 };
                        note(n.start as f64 + late, ci, key, n.velocity, n.fine + cents, n.bright);
                        note((n.end() as f64 + late).min(length), ci, key, 0, 0.0, 0.5);
                        slots |= 1 << (ch.slot % 64);
                    }
                }
                events.sort_by(|a, b| a.time.total_cmp(&b.time).then(a.velocity.cmp(&b.velocity)));
                let autos = p.automation.iter().filter(|a| !a.points.is_empty()).filter_map(|a| Some((mod_target(a.target)?, a.points.clone(), a.curves.clone()))).collect();
                SongPattern { length, autos, events, slots }
            })
            .collect();

        let mut clips: Vec<SongClip> = project
            .clips
            .iter()
            .filter_map(|c| {
                let content = match &c.kind {
                    ClipKind::Pattern { pattern } if *pattern < project.patterns.len() => Content::Pattern(*pattern),
                    ClipKind::Pattern { .. } => return None,
                    ClipKind::Audio { sample: path } => Content::Audio(sample(path)?),
                    ClipKind::Automation { .. } => return None,
                };
                if c.fx.mute || !project.lane_plays(c.lane) {
                    return None;
                }
                let gain = 10f32.powf(c.fx.gain / 20.0);
                let track = if c.fx.track < tracks.len() { c.fx.track } else { 0 };
                Some(SongClip { start: c.start as f64, end: c.end() as f64, offset: c.offset as f64, content, fx: c.fx.clone(), gain, track })
            })
            .collect();
        clips.sort_by(|a, b| a.start.total_cmp(&b.start));

        let end = project.song_end() as f64;
        let looped = transport.looped.filter(|(a, b)| b > a).map(|(a, b)| (a as f64, b as f64));
        Self {
            tempo: project.tempo,
            metronome: project.metronome,
            playing: transport.playing,
            looping: true,
            mode: transport.mode,
            // a loop plays from its start; else from the song position, if it's in the song
            start: match looped {
                Some((from, _)) => from,
                None if (transport.start as f64) < end => transport.start as f64,
                None => 0.0,
            },
            looped,
            jump: transport.jump,
            end,
            channels,
            patterns,
            clips,
            order: order(&tracks, &devices),
            tracks,
            devices,
            macros: std::array::from_fn(|i| project.macros.get(i).map_or(0.0, |m| m.value)),
            lfos: project.lfos.iter().take(MAX_LFOS).copied().collect(),
            mods,
            autos,
        }
    }

    pub(crate) fn empty() -> Self {
        Self {
            tempo: 120.0,
            metronome: false,
            playing: false,
            looping: true,
            mode: Mode::Pattern(0),
            start: 0.0,
            jump: 0,
            end: BAR as f64,
            looped: None,
            channels: Vec::new(),
            patterns: Vec::new(),
            clips: Vec::new(),
            tracks: vec![SongTrack { gain: 1.0, pan: 0.0, eq: [0.0; 3], mute: false, outputs: Vec::new(), devices: 0..0 }],
            order: vec![0],
            devices: Vec::new(),
            macros: [0.0; MACROS],
            lfos: Vec::new(),
            mods: Vec::new(),
            autos: Vec::new(),
        }
    }

    /// Length of what's looping: the pattern in PAT mode, the playlist in SONG mode.
    /// What plays over and over: the loop region (song mode), else the whole timeline.
    pub fn span(&self) -> (f64, f64) {
        match (self.mode, self.looped) {
            (Mode::Song, Some(region)) if self.looping => region,
            _ => (0.0, self.timeline()),
        }
    }

    pub fn timeline(&self) -> f64 {
        match self.mode {
            Mode::Pattern(p) => self.patterns.get(p).map_or(BAR as f64, |p| p.length),
            Mode::Song => self.end,
        }
    }

    /// Seconds per tick.
    pub fn tick_seconds(&self) -> f64 {
        60.0 / (self.tempo as f64 * ravune_core::PPQ as f64)
    }
}

/// Feeders and sidechain sources before the tracks that need them (Kahn's algorithm).
/// The master feeds nothing, so it ends up last.
fn order(tracks: &[SongTrack], devices: &[SongDevice]) -> Vec<usize> {
    // edges: route / sends, plus key track → the track whose compressor it drives
    let edges: Vec<Vec<usize>> = (0..tracks.len())
        .map(|i| {
            let keyed = tracks.iter().enumerate().filter(|(_, t)| devices[t.devices.clone()].iter().any(|d| d.key == Some(i))).map(|(t, _)| t);
            tracks[i].outputs.iter().map(|&(to, _)| to).chain(keyed).collect()
        })
        .collect();
    let mut inputs = vec![0usize; tracks.len()];
    edges.iter().flatten().for_each(|&to| inputs[to] += 1);
    let mut ready: Vec<usize> = (0..tracks.len()).filter(|&i| inputs[i] == 0).collect();
    let mut order = Vec::with_capacity(tracks.len());
    while let Some(i) = ready.pop() {
        order.push(i);
        for &to in &edges[i] {
            inputs[to] -= 1;
            if inputs[to] == 0 {
                ready.push(to);
            }
        }
    }
    // the core rejects loops; if one slips through, its tracks run last and lose what they feed
    let missing: Vec<usize> = (0..tracks.len()).filter(|i| !order.contains(i)).collect();
    order.extend(missing);
    order
}
