//! Mixer tracks, effect devices and modulation (macros, LFOs, mappings).

use serde::{Deserialize, Serialize};

use crate::{Curve, Param, ParamInfo, PluginRef};

pub const MAX_TRACKS: usize = 32;
pub const MAX_DEVICE_PARAMS: usize = 6;
pub const MACROS: usize = 8;
pub const MAX_LFOS: usize = 4;

const fn spec(name: &'static str, (min, max, default): (f32, f32, f32), curve: Curve, unit: &'static str, help: &'static str) -> ParamInfo {
    ParamInfo { name, min, max, default, curve, unit, help }
}

use Curve::*;

const MIX: ParamInfo = spec("Mix", (0.0, 1.0, 0.3), Linear, "%", "Dry/wet balance.");

/// Built-in effects. Each has at most `MAX_DEVICE_PARAMS` parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Eq,
    Filter,
    Compressor,
    Distortion,
    Chorus,
    Delay,
    Reverb,
    Limiter,
    /// A CLAP or VST3 effect (its plugin is on the device). Not in [`DeviceKind::ALL`].
    Plugin,
}

impl DeviceKind {
    pub const ALL: [DeviceKind; 8] = [
        DeviceKind::Eq,
        DeviceKind::Filter,
        DeviceKind::Compressor,
        DeviceKind::Distortion,
        DeviceKind::Chorus,
        DeviceKind::Delay,
        DeviceKind::Reverb,
        DeviceKind::Limiter,
    ];

    pub fn name(self) -> &'static str {
        match self {
            DeviceKind::Eq => "EQ Three",
            DeviceKind::Filter => "Filter",
            DeviceKind::Compressor => "Compressor",
            DeviceKind::Distortion => "Saturator",
            DeviceKind::Chorus => "Chorus",
            DeviceKind::Delay => "Delay",
            DeviceKind::Reverb => "Reverb",
            DeviceKind::Limiter => "Limiter",
            DeviceKind::Plugin => "Plugin",
        }
    }

    pub fn params(self) -> &'static [ParamInfo] {
        const EQ: [ParamInfo; 4] = [
            spec("Low", (-15.0, 15.0, 0.0), Linear, "dB", "Low shelf at 120 Hz."),
            spec("Mid Freq", (200.0, 8000.0, 1000.0), Log, "Hz", "Centre of the mid band."),
            spec("Mid", (-15.0, 15.0, 0.0), Linear, "dB", "Boost or cut around the mid frequency."),
            spec("High", (-15.0, 15.0, 0.0), Linear, "dB", "High shelf at 6 kHz."),
        ];
        const FILTER: [ParamInfo; 3] = [
            spec("Type", (0.0, 2.0, 0.0), Steps, "ftype", "Low-pass keeps the lows, high-pass keeps the highs, band-pass keeps the middle."),
            spec("Cutoff", (30.0, 18000.0, 2000.0), Log, "Hz", "Where the filter starts to act."),
            spec("Reso", (0.0, 1.0, 0.2), Linear, "%", "Emphasis at the cutoff."),
        ];
        const COMP: [ParamInfo; 5] = [
            spec("Threshold", (-48.0, 0.0, -18.0), Linear, "dB", "Level above which the signal is turned down."),
            spec("Ratio", (1.0, 20.0, 4.0), Log, ":1", "How hard: 4:1 lets 1 dB through for every 4 dB over the threshold."),
            spec("Attack", (0.0001, 0.1, 0.005), Log, "s", "How fast it reacts. Slower keeps the punch of drums."),
            spec("Release", (0.01, 1.0, 0.12), Log, "s", "How fast it lets go."),
            spec("Makeup", (0.0, 24.0, 3.0), Linear, "dB", "Gain added after compression."),
        ];
        const DIST: [ParamInfo; 3] = [
            spec("Drive", (0.0, 36.0, 12.0), Linear, "dB", "How hard the signal hits the saturation curve."),
            spec("Tone", (500.0, 18000.0, 8000.0), Log, "Hz", "Darkens the distortion."),
            spec("Mix", (0.0, 1.0, 1.0), Linear, "%", "Dry/wet balance."),
        ];
        const CHORUS: [ParamInfo; 3] = [
            spec("Rate", (0.05, 5.0, 0.6), Log, "Hz", "Speed of the movement."),
            spec("Depth", (0.0, 1.0, 0.5), Linear, "%", "Width of the movement."),
            spec("Mix", (0.0, 1.0, 0.5), Linear, "%", "Dry/wet balance."),
        ];
        const DELAY: [ParamInfo; 4] = [
            spec("Time", (1.0, 16.0, 3.0), Steps, "16th", "Echo time in 16th notes, synced to the tempo."),
            spec("Feedback", (0.0, 0.95, 0.4), Linear, "%", "How many echoes."),
            spec("Ping-Pong", (0.0, 1.0, 1.0), Steps, "onoff", "Echoes bounce between left and right."),
            MIX,
        ];
        const REVERB: [ParamInfo; 4] = [
            spec("Size", (0.0, 1.0, 0.6), Linear, "%", "Room size: small booth to large hall."),
            spec("Damping", (0.0, 1.0, 0.4), Linear, "%", "How quickly the highs die out."),
            spec("Width", (0.0, 1.0, 1.0), Linear, "%", "Stereo spread of the tail."),
            MIX,
        ];
        const LIMIT: [ParamInfo; 2] = [
            spec("Ceiling", (-12.0, 0.0, -0.3), Linear, "dB", "Peaks never go above this."),
            spec("Release", (0.01, 1.0, 0.1), Log, "s", "How fast the gain recovers."),
        ];
        match self {
            DeviceKind::Eq => &EQ,
            DeviceKind::Filter => &FILTER,
            DeviceKind::Compressor => &COMP,
            DeviceKind::Distortion => &DIST,
            DeviceKind::Chorus => &CHORUS,
            DeviceKind::Delay => &DELAY,
            DeviceKind::Reverb => &REVERB,
            DeviceKind::Limiter => &LIMIT,
            DeviceKind::Plugin => &[],
        }
    }

    pub fn defaults(self) -> Vec<f32> {
        self.params().iter().map(|p| p.default).collect()
    }
}

/// Compressor settings for "Duck to…": about 16 dB down on a full-scale hit, back up
/// within an eighth note, and quiet while the trigger's tail fades.
pub const DUCK: [f32; 5] = [-20.0, 6.0, 0.0005, 0.12, 0.0];

/// An effect in a mixer track's chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Device {
    /// Stable id: the engine keeps the effect's state (delay lines, reverb tail) by it.
    pub id: u32,
    pub kind: DeviceKind,
    pub params: Vec<f32>,
    #[serde(default)]
    pub bypass: bool,
    /// Compressors only: the track whose sound drives it (sidechain), heard after its
    /// effects and before its fader, so a muted "ghost" kick still pumps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidechain: Option<usize>,
    /// [`DeviceKind::Plugin`] only: which plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginRef>,
    /// What the user called it (empty: its kind's name), its color and its icon (an icon
    /// name the app knows; none: its kind's).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Send {
    pub to: usize,
    /// 0..1, post-fader.
    pub amount: f32,
}

/// FL-style mixer insert: effects, fader, pan, where it goes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MixerTrack {
    pub name: String,
    /// Linear, 0..1.25 (1 = 0 dB).
    pub gain: f32,
    /// -1 (left) .. 1 (right).
    pub pan: f32,
    /// The strip's EQ, in dB (-15..15): low shelf at 120 Hz, mid around 1 kHz, high shelf at
    /// 6 kHz. After the effects, before the fader.
    #[serde(skip_serializing_if = "flat")]
    pub eq: [f32; 3],
    pub mute: bool,
    /// Soloed: while any track is, only the soloed ones play (with what they feed and what
    /// feeds them).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub solo: bool,
    /// Track this one feeds (0 = master). Ignored on the master itself.
    pub route: usize,
    pub sends: Vec<Send>,
    pub devices: Vec<Device>,
    /// Its color in the mixer (none: the palette's, by its place) and its icon (an icon
    /// name the app knows; none: what plays into it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// A ghost: a sound's own track, made by its panel, kept out of the mixer unless its
    /// ghosts are shown (it plays and routes like any other).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

fn flat(eq: &[f32; 3]) -> bool {
    *eq == [0.0; 3]
}

impl Default for MixerTrack {
    fn default() -> Self {
        Self::new("Track")
    }
}

impl MixerTrack {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), gain: 1.0, pan: 0.0, eq: [0.0; 3], mute: false, solo: false, route: 0, sends: Vec::new(), devices: Vec::new(), color: None, icon: None, hidden: false }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Macro {
    pub name: String,
    /// 0..1
    pub value: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lfo {
    /// 0 sine, 1 triangle, 2 saw, 3 square (same as the synth waves), 4 random.
    pub shape: u8,
    /// Free rate in Hz.
    pub hz: f32,
    /// When synced: one cycle every `beats`.
    pub sync: bool,
    pub beats: f32,
}

impl Default for Lfo {
    fn default() -> Self {
        Self { shape: 0, hz: 1.0, sync: true, beats: 1.0 }
    }
}

/// Where modulation comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "index", rename_all = "snake_case")]
pub enum Source {
    /// 0..1
    Macro(u8),
    /// -1..1
    Lfo(u8),
}

/// What a modulation source can move: any knob in the project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Target {
    Synth { slot: u8, param: Param },
    Device { id: u32, param: usize },
    TrackGain { track: usize },
    TrackPan { track: usize },
    /// A channel's volume (its knob in the rack).
    ChannelGain { slot: u8 },
    /// A macro knob (automation and MIDI only: macros don't modulate macros).
    Macro { index: u8 },
}

/// A controller knob or fader (MIDI CC on a channel, 0-based) moving a parameter;
/// learned with right-click → MIDI learn.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CcMap {
    pub channel: u8,
    pub cc: u8,
    pub target: Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mapping {
    pub source: Source,
    pub target: Target,
    /// -1..1: how far the source moves the target, as a fraction of the knob's travel.
    pub amount: f32,
}
