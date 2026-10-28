//! The project model: channels, patterns (steps + notes), playlist clips, key.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{CcMap, Device, Lfo, MACROS, Macro, Mapping, MixerTrack, PARAM_COUNT, Target, default_synth};

/// Ticks per quarter note. Every time in the project is an integer number of ticks.
pub const PPQ: u32 = 96;
/// One 16th note (a channel-rack step).
pub const STEP: u32 = PPQ / 4;
/// One 4/4 bar.
pub const BAR: u32 = PPQ * 4;
/// Longest pattern, in 16th steps (32 bars).
pub const MAX_STEPS: usize = 512;
/// The key a step plays: a step is a one-step note on this key, on the grid.
pub const STEP_KEY: u8 = 60;
/// Colors new patterns take in turn (RGB).
pub const PATTERN_COLORS: [[u8; 3]; 12] = [
    [0xe8, 0x6a, 0x3c],
    [0xe8, 0xa8, 0x3c],
    [0xc9, 0xd4, 0x46],
    [0x6c, 0xc7, 0x5a],
    [0x3c, 0xc4, 0x9b],
    [0x3c, 0xb4, 0xd8],
    [0x4f, 0x86, 0xe8],
    [0x7c, 0x6c, 0xe8],
    [0xb0, 0x62, 0xe0],
    [0xe0, 0x5c, 0xb8],
    [0xe0, 0x5c, 0x6e],
    [0x9a, 0xa4, 0xb0],
];
/// Engine voice slots; each channel owns one for its lifetime.
pub const MAX_CHANNELS: usize = 64;
/// Velocity a freshly clicked step or drawn note gets.
pub const DEFAULT_VELOCITY: u8 = 100;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Instrument {
    /// One-shot sampler. `sample` is a file path or `builtin:<name>`.
    Sampler { sample: String },
    /// VA-1 virtual analog synth.
    Synth { params: [f32; PARAM_COUNT] },
    /// Multi-sample instrument: an .sfz or Decent Sampler .dspreset file.
    Multi { file: String },
    /// A CLAP or VST3 instrument, run in its own process.
    Plugin { plugin: PluginRef },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginFormat {
    Clap,
    Vst3,
}

/// A plugin and how it was last left.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginRef {
    pub format: PluginFormat,
    /// The plugin file or bundle.
    pub path: String,
    /// CLAP id or VST3 class id.
    pub id: String,
    pub name: String,
    /// Its saved state (base64), written when the project is saved.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub state: String,
}

impl Instrument {
    /// A sampler for an audio file, a multi-sample instrument for an .sfz / .dspreset.
    pub fn from_file(path: String) -> Self {
        if is_instrument(&path) { Instrument::Multi { file: path } } else { Instrument::Sampler { sample: path } }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    /// Engine slot, stable while the channel exists; patterns key their data by it.
    pub slot: u8,
    pub name: String,
    pub instrument: Instrument,
    /// 0..1
    pub gain: f32,
    pub mute: bool,
    /// Semitones. Samplers: playback pitch. Synths: the note steps play (C4 + pitch).
    pub pitch: f32,
    /// Channels sharing a non-zero choke group cut each other off (open/closed hats).
    pub choke: u8,
    /// Mixer track it plays into (0 = master).
    #[serde(default)]
    pub track: usize,
    /// Samplers: seconds of fade-in at each hit.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attack: f32,
    /// Samplers: seconds of fade-out once the note ends; 0 plays every hit to its end.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub release: f32,
    /// Samplers: the part of the sound each hit plays, as fractions of its length (`start`
    /// cuts its beginning, `end` its tail).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub start: f32,
    #[serde(default = "whole", skip_serializing_if = "is_whole")]
    pub end: f32,
    /// Samplers: the part between start and end repeats while the note is held (FL's loop)
    /// instead of playing once.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub looped: bool,
    /// Swing of its own (0..1, like the song's); none follows the song.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swing: Option<f32>,
}

impl Channel {
    pub fn new(slot: u8, name: impl Into<String>, instrument: Instrument) -> Self {
        Self { slot, name: name.into(), instrument, gain: 0.8, mute: false, pitch: 0.0, choke: 0, track: 0, attack: 0.0, release: 0.0, start: 0.0, end: 1.0, looped: false, swing: None }
    }

    pub fn synth(&self) -> Option<&[f32; PARAM_COUNT]> {
        match &self.instrument {
            Instrument::Synth { params } => Some(params),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// Ticks from the start of the pattern.
    pub start: u32,
    pub length: u32,
    /// MIDI key, 60 = C4.
    pub key: u8,
    pub velocity: u8,
    /// Fine pitch in semitones, -2..2 (samplers and VA-1).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fine: f32,
    /// Brightness 0..1, 0.5 = as patched (VA-1: the filter, per note).
    #[serde(default = "half", skip_serializing_if = "is_half")]
    pub bright: f32,
}

fn is_zero(x: &f32) -> bool {
    *x == 0.0
}

fn half() -> f32 {
    0.5
}

fn whole() -> f32 {
    1.0
}

fn is_whole(x: &f32) -> bool {
    *x == 1.0
}

fn is_half(x: &f32) -> bool {
    *x == 0.5
}

impl Default for Note {
    fn default() -> Self {
        Self { start: 0, length: STEP, key: STEP_KEY, velocity: DEFAULT_VELOCITY, fine: 0.0, bright: 0.5 }
    }
}

impl Note {
    pub fn end(&self) -> u32 {
        self.start + self.length
    }

    /// Is this a step-sequencer step: one step long, on the grid, on the step key?
    pub fn is_step(&self) -> bool {
        self.key == STEP_KEY && self.length == STEP && self.start.is_multiple_of(STEP)
    }
}

/// FL-style pattern: notes for any channel. The step sequencer and the piano roll are two
/// views of the same notes: a step is a one-step note on [`STEP_KEY`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "PatternFile")]
pub struct Pattern {
    pub name: String,
    /// RGB; none = the palette color for its place in the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    /// Length in 16th steps, 1..=`MAX_STEPS`.
    pub length: usize,
    /// Notes by channel slot. Unordered; the engine sorts when compiling.
    pub notes: BTreeMap<u8, Vec<Note>>,
    /// Knobs moving inside the pattern: wherever it plays, they play along.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automation: Vec<PatternAuto>,
}

/// A knob's line inside a pattern: points in ticks from the pattern's start, values 0..1
/// along the knob's travel (like an automation clip's).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatternAuto {
    pub target: Target,
    pub points: Vec<(u32, f32)>,
    /// How each stretch between two points bends (see [`bend`]); empty: all straight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<f32>,
}

/// A pattern as saved; files from before steps became notes kept step velocities apart.
#[derive(Deserialize)]
#[serde(default)]
struct PatternFile {
    name: String,
    color: Option<[u8; 3]>,
    length: usize,
    steps: BTreeMap<u8, Vec<u8>>,
    notes: BTreeMap<u8, Vec<Note>>,
    automation: Vec<PatternAuto>,
}

impl Default for PatternFile {
    fn default() -> Self {
        let Pattern { name, color, length, notes, automation } = Pattern::default();
        Self { name, color, length, steps: BTreeMap::new(), notes, automation }
    }
}

impl From<PatternFile> for Pattern {
    fn from(file: PatternFile) -> Self {
        let mut p = Pattern { name: file.name, color: file.color, length: file.length.clamp(1, MAX_STEPS), notes: file.notes, automation: file.automation };
        for (slot, velocities) in file.steps {
            for (s, &v) in velocities.iter().enumerate().filter(|(_, v)| **v > 0) {
                p.set_step(slot, s, v);
            }
        }
        p
    }
}

impl Default for Pattern {
    fn default() -> Self {
        Self::new("Pattern 1")
    }
}

impl Pattern {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), color: None, length: 16, notes: BTreeMap::new(), automation: Vec::new() }
    }

    pub fn ticks(&self) -> u32 {
        self.length as u32 * STEP
    }

    /// The velocity of a step (0 = off).
    pub fn step(&self, slot: u8, step: usize) -> u8 {
        let start = step as u32 * STEP;
        self.notes(slot).iter().find(|n| n.is_step() && n.start == start).map_or(0, |n| n.velocity)
    }

    /// Turns a step on with a velocity, or off with 0.
    pub fn set_step(&mut self, slot: u8, step: usize, velocity: u8) {
        let start = step as u32 * STEP;
        let notes = self.notes_mut(slot);
        match notes.iter_mut().find(|n| n.is_step() && n.start == start) {
            Some(n) if velocity > 0 => n.velocity = velocity,
            Some(_) => notes.retain(|n| !(n.is_step() && n.start == start)),
            None if velocity > 0 => notes.push(Note { start, length: STEP, key: STEP_KEY, velocity, ..Default::default() }),
            None => {}
        }
    }

    /// Can the step sequencer show this channel's notes (are they all steps)?
    pub fn is_steps(&self, slot: u8) -> bool {
        self.notes(slot).iter().all(Note::is_step)
    }

    pub fn notes(&self, slot: u8) -> &[Note] {
        self.notes.get(&slot).map_or(&[], Vec::as_slice)
    }

    pub fn notes_mut(&mut self, slot: u8) -> &mut Vec<Note> {
        self.notes.entry(slot).or_default()
    }

    /// Drops everything a removed channel left behind (its slot may be reused).
    pub(crate) fn forget(&mut self, slot: u8) {
        self.notes.remove(&slot);
        self.automation.retain(|a| !matches!(a.target, Target::Synth { slot: s, .. } | Target::ChannelGain { slot: s } if s == slot));
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClipKind {
    /// Plays a pattern, looping it if the clip is longer.
    Pattern { pattern: usize },
    /// Plays a sample file at its own speed.
    Audio { sample: String },
    /// Moves a knob over time (FL Studio's automation clips): points like an envelope's,
    /// values 0..1 along the knob's travel.
    Automation {
        target: Target,
        points: Vec<(u32, f32)>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        curves: Vec<f32>,
    },
}

/// A block on the playlist. Any clip can go on any lane (FL-style).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub lane: u32,
    /// Ticks from the start of the song.
    pub start: u32,
    pub length: u32,
    /// Ticks skipped at the start of the content (left-trim, split).
    pub offset: u32,
    pub kind: ClipKind,
    /// The clip's own sound (gain, pitch, fades, envelopes); mute for any clip.
    #[serde(default, skip_serializing_if = "ClipFx::is_plain")]
    pub fx: ClipFx,
}

impl Clip {
    pub fn end(&self) -> u32 {
        self.start + self.length
    }
}

/// A named point on the timeline: the ruler shows it, a click there jumps to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub tick: u32,
    pub name: String,
}

/// A playlist lane's own settings; lanes without one are plain.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LaneInfo {
    pub name: String,
    pub color: Option<[u8; 3]>,
    pub mute: bool,
    pub solo: bool,
}

/// Chords the piano roll can stamp: name and intervals from the root.
pub const CHORDS: [(&str, &[u8]); 10] = [
    ("Major", &[0, 4, 7]),
    ("Minor", &[0, 3, 7]),
    ("7", &[0, 4, 7, 10]),
    ("Maj7", &[0, 4, 7, 11]),
    ("Min7", &[0, 3, 7, 10]),
    ("Sus2", &[0, 2, 7]),
    ("Sus4", &[0, 5, 7]),
    ("Dim", &[0, 3, 6]),
    ("Aug", &[0, 4, 8]),
    ("Power", &[0, 7, 12]),
];

/// What a clip envelope moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvTarget {
    Volume,
    Pan,
    /// DJ filter: low-pass below the middle, high-pass above, open in the middle.
    Filter,
}

impl EnvTarget {
    pub const ALL: [EnvTarget; 3] = [EnvTarget::Volume, EnvTarget::Pan, EnvTarget::Filter];

    pub fn name(self) -> &'static str {
        match self {
            EnvTarget::Volume => "Volume",
            EnvTarget::Pan => "Pan",
            EnvTarget::Filter => "Filter",
        }
    }

    /// Where a fresh envelope sits: full volume, centre, filter open.
    pub fn neutral(self) -> f32 {
        match self {
            EnvTarget::Volume => 1.0,
            EnvTarget::Pan | EnvTarget::Filter => 0.5,
        }
    }

    /// A value as people read it: "-6.0 dB", "L 40", "LP 2.1 kHz".
    pub fn describe(self, v: f32) -> String {
        match self {
            EnvTarget::Volume if v <= 0.0 => "-inf dB".into(),
            EnvTarget::Volume => format!("{:.1} dB", 20.0 * v.log10()),
            EnvTarget::Pan if (v - 0.5).abs() < 0.01 => "C".into(),
            EnvTarget::Pan => format!("{} {:.0}", if v < 0.5 { "L" } else { "R" }, (v - 0.5).abs() * 200.0),
            EnvTarget::Filter => match filter_cutoff(v) {
                None => "open".into(),
                Some((high, hz)) => format!("{} {}", if high { "HP" } else { "LP" }, if hz >= 1000.0 { format!("{:.1} kHz", hz / 1000.0) } else { format!("{hz:.0} Hz") }),
            },
        }
    }
}

/// The DJ filter for a 0..1 value: none in the middle, else (high-pass?, cutoff Hz), 20 Hz–20 kHz.
pub fn filter_cutoff(v: f32) -> Option<(bool, f32)> {
    let d = v - 0.5;
    if d.abs() < 0.01 {
        return None;
    }
    // low-pass closes as the value goes down, high-pass as it goes up
    let k = if d < 0.0 { v / 0.5 } else { d / 0.5 };
    Some((d > 0.0, 20.0 * 1000f32.powf(k.clamp(0.0, 1.0))))
}

/// Breakpoints over a clip: ticks from where its content starts (so trims and splits keep
/// them on the sound), values 0..1, straight lines in between.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub target: EnvTarget,
    pub points: Vec<(u32, f32)>,
    /// How each stretch between two points bends (see [`bend`]); empty: all straight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<f32>,
    /// Drawn on the clip in the playlist.
    #[serde(default = "shown")]
    pub shown: bool,
}

fn shown() -> bool {
    true
}

impl Envelope {
    pub fn at(&self, tick: f64) -> f32 {
        curve_at(&self.points, &self.curves, tick).unwrap_or(self.target.neutral())
    }
}

/// Lines between breakpoints (tick, value), each bent by its curve (`curves[i]` for the
/// stretch after point i, none = straight), flat before the first and after the last;
/// None without any.
pub fn curve_at(p: &[(u32, f32)], curves: &[f32], tick: f64) -> Option<f32> {
    match p.iter().position(|(t, _)| *t as f64 > tick) {
        None => p.last().map(|(_, v)| *v),
        Some(0) => Some(p[0].1),
        Some(i) => {
            let ((t0, v0), (t1, v1)) = (p[i - 1], p[i]);
            let k = ((tick - t0 as f64) / (t1 - t0).max(1) as f64) as f32;
            Some(v0 + (v1 - v0) * bend(k, curves.get(i - 1).copied().unwrap_or(0.0), v1 >= v0))
        }
    }
}

/// How far along a stretch (`k` 0..1) its value has gone, bent by `curve` (-1..1): 0 is a
/// straight line, toward 1 it bows up (rises early, falls late), toward -1 down. The bend is
/// exponential (a DAW's usual curve): steep at one end, but never straight up the way a
/// power curve's start was, and the same shape whichever way the stretch goes.
pub fn bend(k: f32, curve: f32, rising: bool) -> f32 {
    if curve.abs() < 1e-3 {
        return k;
    }
    let a = 6.0 * curve.clamp(-1.0, 1.0) * if rising { -1.0 } else { 1.0 };
    ((a * k).exp() - 1.0) / (a.exp() - 1.0)
}

/// Curves as the operators keep them: -1..1, one per stretch between `points` points; none
/// at all when every one is straight.
pub fn clean_curves(mut curves: Vec<f32>, points: usize) -> Vec<f32> {
    curves.iter_mut().for_each(|c| *c = if c.is_finite() { c.clamp(-1.0, 1.0) } else { 0.0 });
    curves.resize(points.saturating_sub(1), 0.0);
    if curves.iter().all(|c| c.abs() < 1e-3) {
        curves.clear();
    }
    curves
}

/// A point put in at index `k`: the stretch it splits keeps its bend on both sides.
pub fn insert_point(points: &mut Vec<(u32, f32)>, curves: &mut Vec<f32>, k: usize, point: (u32, f32)) {
    if !curves.is_empty() {
        curves.resize(points.len().saturating_sub(1), 0.0);
        let bent = curves.get(k.saturating_sub(1)).copied().unwrap_or(0.0);
        curves.insert(k.min(curves.len()), bent);
    }
    points.insert(k, point);
}

/// Point `k` taken out: the stretch before it now runs to the next one.
pub fn remove_point(points: &mut Vec<(u32, f32)>, curves: &mut Vec<f32>, k: usize) {
    points.remove(k);
    if !curves.is_empty() {
        curves.remove(k.min(curves.len() - 1));
    }
}

/// The middle of each stretch of a line, where its bend is grabbed: (stretch, tick, value).
pub fn bend_spots(points: &[(u32, f32)], curves: &[f32]) -> Vec<(usize, f64, f32)> {
    points
        .windows(2)
        .enumerate()
        .map(|(k, w)| {
            let mid = (w[0].0 as f64 + w[1].0 as f64) / 2.0;
            (k, mid, curve_at(points, curves, mid).unwrap_or(0.5))
        })
        .collect()
}

/// The line through `points` from tick `from` to `to`, as (tick, value): its points, and
/// enough in between to draw the bent stretches smoothly.
pub fn curve_samples(points: &[(u32, f32)], curves: &[f32], from: f64, to: f64) -> Vec<(f64, f32)> {
    let at = |tick: f64| curve_at(points, curves, tick).unwrap_or(0.5);
    let mut out = vec![(from, at(from))];
    for (i, w) in points.windows(2).enumerate() {
        let (a, b) = (w[0].0 as f64, w[1].0 as f64);
        if b <= from || a >= to {
            continue;
        }
        if curves.get(i).is_some_and(|c| c.abs() >= 1e-3) {
            out.extend((1..24).map(|s| a + (b - a) * s as f64 / 24.0).filter(|tick| *tick > from && *tick < to).map(|tick| (tick, at(tick))));
        }
        if b < to {
            out.push((b, w[1].1));
        }
    }
    if let Some(first) = points.first().filter(|p| (p.0 as f64) > from && (p.0 as f64) < to) {
        out.insert(1, (first.0 as f64, first.1));
    }
    out.push((to, at(to)));
    out
}

/// Breakpoints as the operators keep them: values 0..1, in time order, one per tick.
pub fn clean_points(mut points: Vec<(u32, f32)>) -> Vec<(u32, f32)> {
    points.iter_mut().for_each(|(_, v)| *v = v.clamp(0.0, 1.0));
    points.sort_by_key(|(t, _)| *t);
    points.dedup_by_key(|(t, _)| *t);
    points
}

/// A clip's own sound. Audio clips use all of it; pattern clips only `mute`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipFx {
    /// dB.
    pub gain: f32,
    /// Semitones (the clip plays faster or slower).
    pub pitch: f32,
    /// Ticks from the clip's start / before its end.
    pub fade_in: u32,
    pub fade_out: u32,
    pub mute: bool,
    pub envelopes: Vec<Envelope>,
    /// The mixer track an audio clip plays into (0 = master).
    pub track: usize,
    /// How an audio clip follows the song's tempo.
    pub stretch: Stretch,
    /// The tempo the sound was made at (0 = not known); what `stretch` follows from.
    pub bpm: f32,
    /// Audio clips: the sound repeats under the clip, end to start, instead of playing once.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub looped: bool,
}

/// How an audio clip follows the song's tempo (FL Studio's stretch modes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stretch {
    /// At its own speed, whatever the tempo.
    #[default]
    Off,
    /// Faster with a faster song, and higher too (a turntable).
    Resample,
    /// Faster with a faster song, same pitch.
    Stretch,
}

impl Stretch {
    pub const ALL: [Stretch; 3] = [Stretch::Off, Stretch::Resample, Stretch::Stretch];

    pub fn name(self) -> &'static str {
        match self {
            Stretch::Off => "Off",
            Stretch::Resample => "Resample",
            Stretch::Stretch => "Stretch",
        }
    }
}

impl ClipFx {
    pub fn is_plain(&self) -> bool {
        *self == Self::default()
    }

    pub fn envelope(&self, target: EnvTarget) -> Option<&Envelope> {
        self.envelopes.iter().find(|e| e.target == target)
    }

    /// Seconds of the sound that go by in a second of the song at `tempo`.
    pub fn speed(&self, tempo: f32) -> f64 {
        let pitch = 2f64.powf(self.pitch as f64 / 12.0);
        let tempo = if self.bpm > 0.0 { tempo as f64 / self.bpm as f64 } else { 1.0 };
        match self.stretch {
            Stretch::Off => pitch,
            Stretch::Resample => pitch * tempo,
            Stretch::Stretch => tempo,
        }
    }

    /// Fade gain at `tick` ticks into a clip `length` long.
    pub fn fade(&self, tick: f64, length: f64) -> f32 {
        let fin = if self.fade_in > 0 { (tick / self.fade_in as f64).clamp(0.0, 1.0) } else { 1.0 };
        let fout = if self.fade_out > 0 { ((length - tick) / self.fade_out as f64).clamp(0.0, 1.0) } else { 1.0 };
        (fin * fout) as f32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scale {
    #[default]
    Chromatic,
    Major,
    Minor,
    Dorian,
    Phrygian,
    MajorPentatonic,
    MinorPentatonic,
    Blues,
}

pub const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

pub fn note_name(key: u8) -> String {
    format!("{}{}", NOTE_NAMES[key as usize % 12], key as i32 / 12 - 1)
}

impl Scale {
    pub const ALL: [Scale; 8] = [
        Scale::Chromatic,
        Scale::Major,
        Scale::Minor,
        Scale::Dorian,
        Scale::Phrygian,
        Scale::MajorPentatonic,
        Scale::MinorPentatonic,
        Scale::Blues,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Scale::Chromatic => "Chromatic",
            Scale::Major => "Major",
            Scale::Minor => "Minor",
            Scale::Dorian => "Dorian",
            Scale::Phrygian => "Phrygian",
            Scale::MajorPentatonic => "Major Pentatonic",
            Scale::MinorPentatonic => "Minor Pentatonic",
            Scale::Blues => "Blues",
        }
    }

    fn intervals(self) -> &'static [u8] {
        match self {
            Scale::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            Scale::Major => &[0, 2, 4, 5, 7, 9, 11],
            Scale::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Scale::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Scale::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Scale::MajorPentatonic => &[0, 2, 4, 7, 9],
            Scale::MinorPentatonic => &[0, 3, 5, 7, 10],
            Scale::Blues => &[0, 3, 5, 6, 7, 10],
        }
    }

    pub fn contains(self, root: u8, key: u8) -> bool {
        self.intervals().contains(&((key % 12 + 12 - root % 12) % 12))
    }

    /// The nearest key that is in the scale (ties go down).
    pub fn fold(self, root: u8, key: u8) -> u8 {
        (0..12u8)
            .find_map(|d| [key.saturating_sub(d), key.saturating_add(d).min(127)].into_iter().find(|&k| self.contains(root, k)))
            .unwrap_or(key)
    }
}

/// Everything that is saved and undoable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub tempo: f32,
    pub metronome: bool,
    /// 0..1: delays every second 16th, up to a triplet feel.
    pub swing: f32,
    /// Song key: root (0 = C … 11 = B) and scale. Drives piano-roll highlighting and snapping.
    pub root: u8,
    pub scale: Scale,
    pub channels: Vec<Channel>,
    /// Always at least one.
    pub patterns: Vec<Pattern>,
    /// The playlist.
    pub clips: Vec<Clip>,
    /// Playlist lanes' names, colors, mute and solo (by lane; missing = plain).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lanes: Vec<LaneInfo>,
    /// Named points on the timeline, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Mixer tracks; index 0 is the master.
    pub mixer: Vec<MixerTrack>,
    pub macros: Vec<Macro>,
    /// Stored settings of the macros (one value each), recalled with one click.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variations: Vec<Vec<f32>>,
    pub lfos: Vec<Lfo>,
    pub mappings: Vec<Mapping>,
    /// MIDI controllers mapped onto parameters.
    pub midi_map: Vec<CcMap>,
    /// Next device id.
    pub next_id: u32,
}

impl Default for Project {
    /// The starter template: a drum kit from the built-in library plus a VA-1.
    fn default() -> Self {
        let sampler = |slot, name: &str, file: &str| {
            Channel::new(slot, name, Instrument::Sampler { sample: format!("builtin:{file}") })
        };
        let mut closed = sampler(3, "Hat", "hat-closed.wav");
        let mut open = sampler(4, "Open Hat", "hat-open.wav");
        (closed.choke, open.choke) = (1, 1);
        let mut synth = Channel::new(5, "VA-1", Instrument::Synth { params: default_synth() });
        synth.track = 2;
        let mut channels = vec![sampler(0, "Kick", "kick.wav"), sampler(1, "Snare", "snare.wav"), sampler(2, "Clap", "clap.wav"), closed, open];
        channels.iter_mut().for_each(|c| c.track = 1);
        channels.push(synth);
        Self {
            mixer: vec![MixerTrack::new("Master"), MixerTrack::new("Drums"), MixerTrack::new("Synth")],
            macros: (1..=MACROS).map(|i| Macro { name: format!("Macro {i}"), value: 0.0 }).collect(),
            lfos: Vec::new(),
            mappings: Vec::new(),
            midi_map: Vec::new(),
            next_id: 1,
            tempo: 120.0,
            metronome: false,
            swing: 0.0,
            root: 0,
            scale: Scale::Chromatic,
            channels,
            patterns: vec![Pattern::default()],
            clips: Vec::new(),
            lanes: Vec::new(),
            markers: Vec::new(),
            variations: Vec::new(),
        }
    }
}

impl Project {
    pub(crate) fn free_slot(&self) -> Option<u8> {
        (0..MAX_CHANNELS as u8).find(|s| self.channels.iter().all(|c| c.slot != *s))
    }

    pub fn lane(&self, lane: u32) -> LaneInfo {
        self.lanes.get(lane as usize).cloned().unwrap_or_default()
    }

    /// Which mixer tracks sound: all of them, or while any is soloed, the soloed ones, the
    /// tracks they feed (buses, returns, the master) and the tracks feeding them.
    pub fn tracks_heard(&self) -> Vec<bool> {
        let n = self.mixer.len();
        if !self.mixer.iter().any(|t| t.solo) {
            return vec![true; n];
        }
        let feeds = |i: usize| -> Vec<usize> {
            let t = &self.mixer[i];
            if i == 0 { Vec::new() } else { std::iter::once(t.route).chain(t.sends.iter().map(|s| s.to)).filter(|to| *to < n).collect() }
        };
        let mut heard: Vec<bool> = self.mixer.iter().map(|t| t.solo).collect();
        heard[0] = true;
        // downstream of a solo, then upstream into one (feeds can't loop, so this settles)
        let mut down: Vec<usize> = (0..n).filter(|i| self.mixer[*i].solo).collect();
        while let Some(i) = down.pop() {
            for to in feeds(i) {
                if !std::mem::replace(&mut heard[to], true) {
                    down.push(to);
                }
            }
        }
        let mut up: Vec<usize> = (0..n).filter(|i| self.mixer[*i].solo).collect();
        let mut fed = vec![false; n];
        while let Some(i) = up.pop() {
            for from in (1..n).filter(|from| feeds(*from).contains(&i)) {
                if !std::mem::replace(&mut fed[from], true) {
                    heard[from] = true;
                    up.push(from);
                }
            }
        }
        heard
    }

    /// Does a lane play: not muted, and soloed if any lane is?
    pub fn lane_plays(&self, lane: u32) -> bool {
        let l = self.lane(lane);
        !l.mute && (l.solo || !self.lanes.iter().any(|l| l.solo))
    }

    /// End of the last clip, rounded up to a whole bar (at least one bar).
    pub fn song_end(&self) -> u32 {
        self.clips.iter().map(Clip::end).max().unwrap_or(0).div_ceil(BAR).max(1) * BAR
    }

    /// (track, position, device) for a device id.
    pub fn device(&self, id: u32) -> Option<(usize, usize, &Device)> {
        self.mixer.iter().enumerate().find_map(|(t, track)| track.devices.iter().position(|d| d.id == id).map(|i| (t, i, &track.devices[i])))
    }

    /// Tracks a track sends audio to (its route, then its sends). The master feeds nothing.
    pub fn outputs(&self, track: usize) -> Vec<usize> {
        if track == 0 {
            return Vec::new();
        }
        let t = &self.mixer[track];
        std::iter::once(t.route).chain(t.sends.iter().map(|s| s.to)).filter(|&to| to < self.mixer.len()).collect()
    }

    /// Tracks that must be processed after `track`: what it feeds, and the tracks whose
    /// compressors it keys.
    pub fn after(&self, track: usize) -> Vec<usize> {
        let keyed = self.mixer.iter().enumerate().filter(|(_, t)| t.devices.iter().any(|d| d.sidechain == Some(track))).map(|(i, _)| i);
        let mut out = self.outputs(track);
        out.extend(keyed);
        out
    }

    /// Would an edge `from → to` close a loop (does `to` already reach `from`)?
    pub(crate) fn would_loop(&self, from: usize, to: usize) -> bool {
        let mut stack = vec![to];
        let mut seen = vec![false; self.mixer.len()];
        while let Some(t) = stack.pop() {
            if t == from {
                return true;
            }
            if !std::mem::replace(&mut seen[t], true) {
                stack.extend(self.after(t));
            }
        }
        false
    }
}

/// Multi-sample instrument files (SFZ, Decent Sampler), played across the keyboard.
pub const INSTRUMENT_EXTENSIONS: [&str; 2] = ["sfz", "dspreset"];

pub fn is_instrument(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| INSTRUMENT_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Display name for a sample path: file name without folders or extension.
pub fn sample_name(path: &str) -> &str {
    let file = path.rsplit(['/', '\\', ':']).next().unwrap_or(path);
    file.rsplit_once('.').map_or(file, |(stem, _)| stem)
}
