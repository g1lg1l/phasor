//! Renderer: plays a [`Song`]. Device-independent, so tests and offline export drive it too.
//!
//! Each block: collect every event that falls inside it (steps, notes, clicks, clip
//! starts/ends, loop wraps) into a preallocated list, sort it, then render up to each
//! event's exact sample before applying it. Instruments play into their mixer track's
//! bus; the buses then run their device chains in routing order, feeding each other and
//! finally the master. Nothing here allocates.

use std::f32::consts::{FRAC_PI_2, PI, SQRT_2, TAU};
use std::sync::Arc;
use std::time::Duration;

use ravune_bridge::{Bridge, Event as PlugEvent, kind as plug};
use ravune_core::{DeviceKind, EnvTarget, MACROS, MAX_CHANNELS, MAX_LFOS, MAX_TRACKS, PARAMS, PPQ, Source, curve_at, filter_cutoff};

use crate::effects::{Effect, Eq3, Params, svf};
use crate::song::{Content, Event, MAX_EFFECTS, ModTarget, Mode, Song, SongMod, SongPattern, Sound};
use crate::stretch::Stretcher;
use crate::synth::Synth;
use crate::{Garbage, Msg, MultiMap, PluginTarget, Sampler, VoiceMode};

/// Largest block rendered in one go; longer requests are split. Modulation updates once per block.
pub const BLOCK: usize = 256;

#[derive(Clone, Copy)]
struct Bus {
    l: [f32; BLOCK],
    r: [f32; BLOCK],
}

/// Audio clips that can sound at once.
const CLIP_PLAYERS: usize = 16;

/// One playlist audio clip sounding: its voice (or its stretcher, in stretch mode), which
/// clip, its filter's state.
#[derive(Default)]
struct ClipPlayer {
    voice: Sampler,
    stretch: Stretcher,
    clip: Option<u32>,
    /// The DJ filter's two passes, for each side.
    filter: [[[f32; 2]; 2]; 2],
}

// Slots live in a Vec allocated once; boxing the synth would allocate on the audio thread
// whenever a channel switches instrument.
#[allow(clippy::large_enum_variant)]
enum Slot {
    Idle,
    Synth(Synth),
    Sampler(Sampler),
    /// Its bridge is in [`Renderer::plugins`].
    Plugin,
}

/// Starts every region of `map` that answers `key` at `velocity`. The channel's pitch
/// transposes which regions play, not only how they sound.
#[allow(clippy::too_many_arguments)]
fn play_regions(s: &mut Sampler, map: &MultiMap, key: u8, velocity: u8, gain: f32, pitch: f32, turn: u32, sr: f32) {
    let played = (key as f32 + pitch).clamp(0.0, 127.0) as u8;
    for r in map.regions(played, velocity, turn.wrapping_sub(1)) {
        let mode = VoiceMode { key, release: r.release, one_shot: r.one_shot, looped: r.looped, offset: r.offset };
        s.play(r.sample.clone(), gain * r.gain, played as f32 - r.root, sr, mode);
    }
}

#[derive(Clone, Copy)]
enum Fire {
    Note(Event),
    Click(bool),
    /// A pattern clip ended: release notes its channels are holding.
    ClipEnd(usize),
    /// The loop jumped back to its start.
    Wrap,
    /// Playlist clip index, ticks into the sample.
    AudioStart(u32, f64),
    AudioStop(u32),
}

impl Fire {
    /// At equal times, endings go before beginnings so retriggers work.
    fn order(&self) -> u8 {
        match self {
            Fire::Note(e) if e.velocity > 0 => 2,
            Fire::AudioStart(..) | Fire::Click(_) => 2,
            _ => 1,
        }
    }
}

#[derive(Clone, Copy)]
struct Pending {
    /// Ticks from the start of the block.
    at: f64,
    fire: Fire,
}

const MAX_PENDING: usize = 2048;

fn push(out: &mut Vec<Pending>, at: f64, fire: Fire) {
    if out.len() < out.capacity() {
        out.push(Pending { at, fire }); // never grows: capacity is reserved up front
    }
}

pub struct Renderer {
    sr: f32,
    song: Arc<Song>,
    slots: Vec<Slot>,
    /// Keys the sequencer is holding on each slot (released at clip end, loop wrap, stop).
    seq_held: [u128; MAX_CHANNELS],
    /// Round-robin counters for multi-sample instruments, per slot.
    turns: [u32; MAX_CHANNELS],
    /// The VA-1 channel each sequenced key plays on (notes with their own pitch or
    /// brightness get a channel of their own, MPE-style), and the next one to hand out.
    seq_ch: [[u8; 128]; MAX_CHANNELS],
    next_ch: [u8; MAX_CHANNELS],
    target: Option<u8>,
    held: u128,
    preview: Sampler,
    /// Playlist audio clips that are sounding, each with its own gain, pan and filter.
    players: Vec<ClipPlayer>,
    /// The app's output volume, and where the last block ended its ramp to it.
    output: f32,
    output_now: f32,
    /// Recording: where the master goes while the song plays.
    tap: Option<rtrb::Producer<f32>>,
    playing: bool,
    /// Position in the timeline (pattern or song), ticks.
    pos: f64,
    /// Just started: audio clips already under the playhead must start mid-way.
    seek: bool,
    click: Click,
    pending: Vec<Pending>,
    effects: Vec<(u32, Box<Effect>)>,
    bus: Vec<Bus>,
    /// Preview and metronome: added after the master, untouched by the mixer.
    direct: [f32; BLOCK],
    /// This block's device params and track (gain, pan), after modulation.
    dev_params: Vec<Params>,
    levels: [(f32, f32); MAX_TRACKS],
    /// This block's channel volumes (by slot) and macros, after automation and modulation.
    chan_gain: [f32; MAX_CHANNELS],
    macros: [f32; MACROS],
    /// (left, right) gain each track ended the last block on; gains ramp from there.
    applied: [(f32, f32); MAX_TRACKS],
    meters: [f32; MAX_TRACKS],
    /// Samples each track's effects have put out nothing (below -120 dB) in a row: past a
    /// second, with nothing coming in, they sleep (silence through 15 EQs and a reverb is
    /// most of an idle project's CPU).
    quiet: [u32; MAX_TRACKS],
    /// Each track's strip EQ.
    strip_eq: [Eq3; MAX_TRACKS],
    lfo_phase: [f64; MAX_LFOS],
    /// Cycles a free-running LFO has done (what its random shape draws from).
    lfo_cycle: [u64; MAX_LFOS],
    /// Plugin instruments' bridges, by slot, and plugin effects', by device id.
    plugins: Vec<Option<Arc<Bridge>>>,
    device_plugins: Vec<(u32, Arc<Bridge>)>,
    /// Frame in the block of the event being fired (plugins get it with the event).
    at: usize,
}

/// A note for a plugin.
fn plug_note(kind: u32, at: usize, key: u8, velocity: f32) -> PlugEvent {
    PlugEvent { frame: at as u32, kind, key: key as u32, channel: 0, value: velocity as f64 }
}

impl Renderer {
    pub fn new(sample_rate: f32) -> Self {
        crate::sample::warm_up();
        Self {
            sr: sample_rate,
            song: Arc::new(Song::empty()),
            slots: (0..MAX_CHANNELS).map(|_| Slot::Idle).collect(),
            seq_held: [0; MAX_CHANNELS],
            turns: [0; MAX_CHANNELS],
            seq_ch: [[0; 128]; MAX_CHANNELS],
            next_ch: [0; MAX_CHANNELS],
            target: None,
            held: 0,
            preview: Sampler::default(),
            players: (0..CLIP_PLAYERS).map(|_| ClipPlayer::default()).collect(),
            output: 1.0,
            output_now: 1.0,
            tap: None,
            playing: false,
            pos: 0.0,
            seek: false,
            click: Click::default(),
            pending: Vec::with_capacity(MAX_PENDING),
            effects: Vec::with_capacity(MAX_EFFECTS),
            bus: vec![Bus { l: [0.0; BLOCK], r: [0.0; BLOCK] }; MAX_TRACKS],
            direct: [0.0; BLOCK],
            dev_params: Vec::with_capacity(MAX_EFFECTS),
            levels: [(1.0, 0.0); MAX_TRACKS],
            chan_gain: [1.0; MAX_CHANNELS],
            macros: [0.0; MACROS],
            applied: [(f32::NAN, f32::NAN); MAX_TRACKS],
            meters: [0.0; MAX_TRACKS],
            quiet: [0; MAX_TRACKS],
            strip_eq: std::array::from_fn(|_| Eq3::new(sample_rate)),
            lfo_phase: [0.0; MAX_LFOS],
            lfo_cycle: [0; MAX_LFOS],
            plugins: (0..MAX_CHANNELS).map(|_| None).collect(),
            device_plugins: Vec::with_capacity(MAX_EFFECTS),
            at: 0,
        }
    }

    /// The plugin bridge on a channel slot, if it plays one.
    fn plugin(&self, slot: usize) -> Option<&Bridge> {
        match self.slots.get(slot) {
            Some(Slot::Plugin) => self.plugins[slot].as_deref(),
            _ => None,
        }
    }

    /// Applies a message. Returns what it replaced (a song, an effect), which the caller
    /// must drop off the audio thread.
    pub fn handle(&mut self, msg: Msg) -> Option<Garbage> {
        match msg {
            Msg::Output(gain) => self.output = gain.clamp(0.0, 2.0),
            Msg::Record(tap) => return std::mem::replace(&mut self.tap, tap).map(Garbage::Tap),
            Msg::NoteOn { note, velocity } => {
                self.held |= 1 << (note & 127);
                self.play_key(note, velocity, 0);
            }
            Msg::Midi(m) => self.midi(m),
            Msg::NoteOff { note } => {
                self.held &= !(1 << (note & 127));
                self.release_key(note, 0);
            }
            Msg::AllNotesOff => {
                self.held = 0;
                self.silence();
            }
            Msg::Target(slot) => {
                if let Some(Slot::Synth(s)) = self.target.map(|t| &mut self.slots[t as usize]) {
                    s.all_notes_off();
                }
                if let Some(b) = self.target.and_then(|t| self.plugin(t as usize)) {
                    b.event(PlugEvent { kind: plug::ALL_OFF, ..Default::default() });
                }
                self.held = 0;
                self.target = slot.filter(|s| (*s as usize) < MAX_CHANNELS);
            }
            Msg::Song(song) => return Some(Garbage::Song(self.set_song(song))),
            Msg::AddEffect(id, fx) => {
                if let Some(slot) = self.effects.iter_mut().find(|(e, _)| *e == id) {
                    return Some(Garbage::Effect(std::mem::replace(&mut slot.1, fx)));
                }
                if self.effects.len() == self.effects.capacity() {
                    return Some(Garbage::Effect(fx)); // full: that device stays dry
                }
                self.effects.push((id, fx));
            }
            Msg::RemoveEffect(id) => {
                let i = self.effects.iter().position(|(e, _)| *e == id)?;
                return Some(Garbage::Effect(self.effects.swap_remove(i).1));
            }
            Msg::Preview(Some(sample)) => {
                self.preview.choke();
                self.preview.trigger(sample, 0.8, 0.0, self.sr);
            }
            Msg::Preview(None) => self.preview.choke(),
            Msg::Plugin { target: PluginTarget::Channel(slot), bridge } => {
                let old = std::mem::replace(self.plugins.get_mut(slot as usize)?, bridge);
                return old.map(Garbage::Plugin);
            }
            Msg::Plugin { target: PluginTarget::Device(id), bridge } => {
                let old = self.device_plugins.iter().position(|(d, _)| *d == id).map(|i| self.device_plugins.swap_remove(i).1);
                if let Some(b) = bridge {
                    if self.device_plugins.len() == self.device_plugins.capacity() {
                        return Some(Garbage::Plugin(b)); // full: that device stays dry
                    }
                    self.device_plugins.push((id, b));
                }
                return old.map(Garbage::Plugin);
            }
        }
        None
    }

    fn set_song(&mut self, song: Arc<Song>) -> Arc<Song> {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let Some(ch) = song.channels.iter().find(|c| c.slot as usize == i) else {
                *slot = Slot::Idle; // channel removed
                continue;
            };
            match (&ch.sound, &mut *slot) {
                (Sound::Synth(p), Slot::Synth(s)) => s.set_params(*p),
                (Sound::Synth(p), _) => {
                    let mut s = Synth::new(self.sr);
                    s.set_params(*p);
                    *slot = Slot::Synth(s);
                }
                (Sound::Sample(_) | Sound::Multi(_), Slot::Sampler(_)) => {}
                (Sound::Sample(_) | Sound::Multi(_), _) => *slot = Slot::Sampler(Sampler::default()),
                (Sound::Plugin, Slot::Plugin) => {}
                (Sound::Plugin, _) => *slot = Slot::Plugin,
            }
        }
        let restart = song.playing && (!self.playing || song.mode != self.song.mode);
        // a jump while the song plays: the playhead moves to the new start right away
        let jump = song.playing && self.playing && song.mode == Mode::Song && song.jump != self.song.jump;
        if restart || jump || !song.playing {
            self.release_sequenced();
            self.choke_clips();
        }
        if restart || jump {
            self.pos = song.start;
            self.seek = true;
        }
        if restart {
            (self.lfo_phase, self.lfo_cycle) = ([0.0; MAX_LFOS], [0; MAX_LFOS]);
        }
        self.playing = song.playing;
        let old = std::mem::replace(&mut self.song, song);
        let (from, to) = self.song.span();
        if self.pos >= to {
            self.pos = from; // pattern got shorter, clips were removed, the loop moved
        }
        old
    }

    /// A controller's channel message: notes keep their MIDI channel (MPE), expressions go
    /// to the keys' target, the synth or a plugin.
    fn midi(&mut self, [status, a, b]: [u8; 3]) {
        let (ch, a) = (status & 15, a & 127);
        match status & 0xF0 {
            0x90 if b > 0 => {
                self.held |= 1 << a;
                self.play_key(a, b as f32 / 127.0, ch);
            }
            0x80 | 0x90 => {
                self.held &= !(1 << a);
                self.release_key(a, ch);
            }
            0xE0 => self.expression(plug::BEND, ch, 0, (((b as u16) << 7 | a as u16) as f32 - 8192.0) / 8192.0),
            0xD0 => self.expression(plug::PRESSURE, ch, 0, a as f32 / 127.0),
            0xB0 if a == 120 || a == 123 => {
                self.held = 0;
                self.silence();
            }
            0xB0 => self.expression(plug::CC, ch, a, b as f32 / 127.0),
            _ => {}
        }
    }

    fn expression(&mut self, kind: u32, channel: u8, cc: u8, value: f32) {
        let Some(t) = self.target else { return };
        match &mut self.slots[t as usize] {
            Slot::Synth(s) => match kind {
                plug::BEND => s.bend(channel, value),
                plug::PRESSURE => s.pressure(channel, value),
                plug::CC if cc == 74 => s.slide(channel, value),
                _ => {}
            },
            Slot::Plugin => {
                if let Some(b) = self.plugin(t as usize) {
                    b.event(PlugEvent { frame: 0, kind, key: cc as u32, channel: channel as u32, value: value as f64 });
                }
            }
            _ => {}
        }
    }

    fn release_key(&mut self, note: u8, channel: u8) {
        match self.target.map(|t| &mut self.slots[t as usize]) {
            Some(Slot::Synth(s)) => s.note_off_ch(note, channel),
            Some(Slot::Sampler(s)) => s.release(note),
            Some(Slot::Plugin) => {
                if let Some(b) = self.target.and_then(|t| self.plugin(t as usize)) {
                    b.event(PlugEvent { channel: channel as u32, ..plug_note(plug::NOTE_OFF, 0, note, 0.0) });
                }
            }
            _ => {}
        }
    }

    fn play_key(&mut self, note: u8, velocity: f32, channel: u8) {
        let Some(t) = self.target else { return };
        let Some(ch) = self.song.channels.iter().find(|c| c.slot == t) else { return };
        match (&mut self.slots[t as usize], &ch.sound) {
            (Slot::Synth(s), _) => s.note_on_ch(note, velocity, channel),
            (Slot::Sampler(s), Sound::Sample(Some(sample))) => {
                // chromatic sampler: C4 plays the sample at its own pitch, cut, looped and
                // shaped as its steps play it
                s.hit(sample.clone(), velocity * ch.gain, ch.pitch + note as f32 - 60.0, self.sr, note, (ch.attack, ch.release), (ch.start, ch.end, ch.looped));
            }
            (Slot::Sampler(s), Sound::Multi(Some(map))) => {
                self.turns[t as usize] += 1;
                let vel = (velocity * 127.0).round().clamp(1.0, 127.0) as u8;
                play_regions(s, map, note, vel, velocity * ch.gain, ch.pitch, self.turns[t as usize], self.sr);
            }
            (Slot::Plugin, _) => {
                if let Some(b) = self.plugin(t as usize) {
                    b.event(PlugEvent { channel: channel as u32, ..plug_note(plug::NOTE_ON, 0, note, velocity) });
                }
            }
            _ => {}
        }
    }

    /// Releases sequencer notes on the slots in `mask` (bit per slot).
    fn release(&mut self, mask: u64) {
        for i in 0..MAX_CHANNELS {
            let held = self.seq_held[i];
            if mask >> i & 1 == 0 || held == 0 {
                continue;
            }
            for key in (0..128u8).filter(|k| held >> k & 1 == 1) {
                match &mut self.slots[i] {
                    Slot::Synth(s) => s.note_off_ch(key, self.seq_ch[i][key as usize]),
                    Slot::Sampler(s) => s.release(key),
                    Slot::Plugin => {
                        if let Some(b) = self.plugins[i].as_deref() {
                            b.event(plug_note(plug::NOTE_OFF, self.at, key, 0.0));
                        }
                    }
                    Slot::Idle => {}
                }
            }
            self.seq_held[i] = 0;
        }
    }

    fn release_sequenced(&mut self) {
        self.release(u64::MAX);
    }

    fn silence(&mut self) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            match slot {
                Slot::Synth(s) => s.all_notes_off(),
                Slot::Sampler(s) => s.choke(),
                Slot::Plugin => {
                    if let Some(b) = self.plugins[i].as_deref() {
                        b.event(PlugEvent { kind: plug::ALL_OFF, ..Default::default() });
                    }
                }
                Slot::Idle => {}
            }
        }
        self.preview.choke();
        self.choke_clips();
        self.seq_held = [0; MAX_CHANNELS];
    }

    fn fire(&mut self, fire: Fire) {
        match fire {
            Fire::Note(e) => self.fire_note(e),
            Fire::Click(downbeat) => {
                if self.song.metronome {
                    self.click.trigger(downbeat, self.sr);
                }
            }
            Fire::ClipEnd(p) => {
                let mask = self.song.patterns.get(p).map_or(0, |p| p.slots);
                self.release(mask);
            }
            Fire::Wrap => {
                self.release_sequenced();
                self.choke_clips();
            }
            Fire::AudioStart(clip, ticks) => {
                if let Some(c) = self.song.clips.get(clip as usize)
                    && let Content::Audio(sample) = &c.content
                {
                    // pitched up or following a faster song, the sound runs through faster: a
                    // start mid-clip lands further in
                    let speed = c.fx.speed(self.song.tempo);
                    let frame = ticks * self.song.tick_seconds() * sample.rate as f64 * speed;
                    // a free player, else the first one gives way
                    let i = self.players.iter().position(|p| p.clip.is_none()).unwrap_or(0);
                    let p = &mut self.players[i];
                    p.voice.choke();
                    p.stretch.choke();
                    p.filter = [[[0.0; 2]; 2]; 2];
                    if c.fx.stretch == ravune_core::Stretch::Stretch {
                        p.stretch.trigger(sample.clone(), 0.8, self.sr, frame, speed, c.fx.pitch, c.fx.looped);
                    } else {
                        // off and resample: played faster is played higher
                        p.voice.trigger_at(sample.clone(), 0.8, 12.0 * speed.log2() as f32, self.sr, frame, clip, c.fx.looped);
                    }
                    p.clip = Some(clip);
                }
            }
            Fire::AudioStop(clip) => {
                for p in self.players.iter_mut().filter(|p| p.clip == Some(clip)) {
                    p.voice.stop_tag(clip);
                    p.stretch.choke();
                }
            }
        }
    }

    fn fire_note(&mut self, e: Event) {
        let song = &self.song;
        let Some(ch) = song.channels.get(e.channel as usize) else { return };
        let slot = ch.slot as usize;
        if e.velocity == 0 {
            if self.seq_held[slot] >> e.key & 1 == 1 {
                match &mut self.slots[slot] {
                    Slot::Synth(s) => s.note_off_ch(e.key, self.seq_ch[slot][e.key as usize]),
                    Slot::Sampler(s) => s.release(e.key),
                    Slot::Plugin => {
                        if let Some(b) = self.plugins[slot].as_deref() {
                            b.event(plug_note(plug::NOTE_OFF, self.at, e.key, 0.0));
                        }
                    }
                    Slot::Idle => {}
                }
            }
            self.seq_held[slot] &= !(1 << e.key);
            return;
        }
        if ch.mute {
            return;
        }
        if ch.choke > 0 {
            for other in song.channels.iter().filter(|o| o.choke == ch.choke && o.slot != ch.slot) {
                if let Slot::Sampler(s) = &mut self.slots[other.slot as usize] {
                    s.choke();
                }
            }
        }
        let gain = e.velocity as f32 / 127.0;
        match (&mut self.slots[slot], &ch.sound) {
            (Slot::Synth(s), _) => {
                // a note with its own pitch or brightness gets a channel of its own
                let channel = if e.fine != 0.0 || e.bright != 0.5 {
                    let next = &mut self.next_ch[slot];
                    *next = *next % 15 + 1;
                    s.bend(*next, e.fine / 48.0);
                    s.slide(*next, e.bright);
                    *next
                } else {
                    0
                };
                self.seq_ch[slot][e.key as usize] = channel;
                s.note_on_ch(e.key, gain, channel);
                self.seq_held[slot] |= 1 << e.key;
            }
            (Slot::Sampler(s), Sound::Sample(Some(sample))) => {
                s.hit(sample.clone(), gain, ch.pitch + e.key as f32 - 60.0 + e.fine, self.sr, e.key, (ch.attack, ch.release), (ch.start, ch.end, ch.looped));
                // held until its note-off: to fade over the release, or to stop looping
                if ch.release > 0.0 || ch.looped {
                    self.seq_held[slot] |= 1 << e.key;
                }
            }
            (Slot::Sampler(s), Sound::Multi(Some(map))) => {
                self.turns[slot] += 1;
                play_regions(s, map, e.key, e.velocity, gain, ch.pitch + e.fine, self.turns[slot], self.sr);
                self.seq_held[slot] |= 1 << e.key;
            }
            (Slot::Plugin, _) => {
                if let Some(b) = self.plugins[slot].as_deref() {
                    b.event(plug_note(plug::NOTE_ON, self.at, e.key, gain));
                }
                self.seq_held[slot] |= 1 << e.key;
            }
            _ => {}
        }
    }

    /// Fills `left` and `right` (same length) with the next stretch of audio.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.chunks_mut(BLOCK).zip(right.chunks_mut(BLOCK)) {
            self.block(l, r);
        }
    }

    fn block(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        self.modulate(n);
        for b in &mut self.bus[..self.song.tracks.len()] {
            b.l[..n].fill(0.0);
            b.r[..n].fill(0.0);
        }
        self.direct[..n].fill(0.0);
        if self.playing {
            let ticks_per_sample = 1.0 / (self.song.tick_seconds() * self.sr as f64);
            let span = n as f64 * ticks_per_sample;
            self.pending.clear();
            let (next, chase) = collect(&self.song, self.pos, span, self.seek, &mut self.pending);
            self.seek = chase;
            self.pending.sort_unstable_by(|a, b| a.at.total_cmp(&b.at).then(a.fire.order().cmp(&b.fire.order())));
            let mut cursor = 0;
            for i in 0..self.pending.len() {
                let p = self.pending[i];
                let at = ((p.at / ticks_per_sample) - 1e-6).ceil().clamp(0.0, n as f64) as usize;
                if at > cursor {
                    self.render_sources(cursor, at);
                    cursor = at;
                }
                self.at = at.min(n.saturating_sub(1));
                self.fire(p.fire);
            }
            self.at = 0;
            self.render_sources(cursor, n);
            self.render_plugins(n);
            self.pos = next;
        } else {
            self.render_sources(0, n);
            self.render_plugins(n);
        }
        self.mix(l, r);
        // recording: the master as mixed, whole blocks only (one the UI had no room for is lost)
        if self.playing
            && let Some(tap) = &mut self.tap
            && tap.slots() >= 2 * n
        {
            for (a, b) in l.iter().zip(r.iter()) {
                let _ = tap.push(*a);
                let _ = tap.push(*b);
            }
        }
        // the app's output volume (a short ramp, no zipper), then the device never sees
        // more than full scale
        let (from, to) = (self.output_now, self.output);
        for (k, ((a, b), d)) in l.iter_mut().zip(r.iter_mut()).zip(&self.direct).enumerate() {
            let g = from + (to - from) * k as f32 / n as f32;
            *a = safe((*a + d) * g);
            *b = safe((*b + d) * g);
        }
        self.output_now = to;
    }

    /// Instruments into their tracks' buses, samples `from..to` of the block.
    fn render_sources(&mut self, from: usize, to: usize) {
        let (mut l, mut r) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        let (l, r) = (&mut l[..to - from], &mut r[..to - from]);
        for ch in &self.song.channels {
            l.fill(0.0);
            r.fill(0.0);
            match &mut self.slots[ch.slot as usize] {
                Slot::Synth(s) => {
                    s.render(l);
                    r.copy_from_slice(l);
                }
                Slot::Sampler(s) => s.render(l, r),
                Slot::Idle | Slot::Plugin => continue, // plugins render once per block
            }
            if !ch.mute {
                let g = self.chan_gain[ch.slot as usize];
                add(&mut self.bus[ch.track], from, l, r, g * g);
            }
        }
        // audio clips, each into its mixer track
        self.render_clips(from, l, r);
        // the browser's preview and the metronome: straight out, in mono
        l.fill(0.0);
        r.fill(0.0);
        self.preview.render(l, r);
        for (d, (a, b)) in self.direct[from..to].iter_mut().zip(l.iter().zip(r.iter())) {
            *d += (a + b) / 2.0 + self.click.tick();
        }
    }

    /// Audio clips into the master from `from`: each player's voice, then its clip's gain,
    /// fades and envelopes (read at both ends of the stretch, straight lines in between).
    fn render_clips(&mut self, from: usize, tl: &mut [f32], tr: &mut [f32]) {
        let n = tl.len();
        let ticks_per_sample = if self.playing { 1.0 / (self.song.tick_seconds() * self.sr as f64) } else { 0.0 };
        let t0 = self.pos + from as f64 * ticks_per_sample;
        let (song, buses, sr) = (&*self.song, &mut self.bus, self.sr);
        for p in &mut self.players {
            let Some(clip) = p.clip else { continue };
            let Some(c) = song.clips.get(clip as usize).filter(|_| p.voice.is_playing() || p.stretch.is_playing()) else {
                p.voice.choke();
                p.stretch.choke();
                p.clip = None;
                continue;
            };
            tl.fill(0.0);
            tr.fill(0.0);
            p.voice.render(tl, tr);
            // the tempo may have changed since it started
            p.stretch.set_speed(c.fx.speed(song.tempo));
            p.stretch.render(tl, tr);
            let bus = &mut buses[c.track];
            let length = c.end - c.start;
            let env = |target: EnvTarget, at: f64, default: f32| c.fx.envelope(target).map_or(default, |e| e.at(at - c.start + c.offset));
            let gain = |at: f64| c.gain * c.fx.fade(at - c.start, length) * env(EnvTarget::Volume, at, 1.0);
            let t1 = t0 + n as f64 * ticks_per_sample;
            let (g0, g1) = (gain(t0), gain(t1));
            let (p0, p1) = (env(EnvTarget::Pan, t0, 0.5), env(EnvTarget::Pan, t1, 0.5));
            let filter = filter_cutoff(env(EnvTarget::Filter, t0, 0.5)).map(|(high, hz)| (high as u8, (PI * hz.min(sr * 0.45) / sr).tan()));
            for (k, (xl, xr)) in tl.iter().zip(tr.iter()).enumerate() {
                let f = k as f32 / n as f32;
                // two passes make the DJ filter steeper (24 dB / octave)
                let mut dj = |side: usize, x: f32| {
                    filter.map_or(x, |(kind, g)| {
                        let y = svf(&mut p.filter[side][0], x, g, 1.0, kind);
                        svf(&mut p.filter[side][1], y, g, 1.0, kind)
                    })
                };
                let (yl, yr) = (dj(0, *xl), dj(1, *xr));
                let gain = g0 + (g1 - g0) * f;
                // equal power, unity in the middle (a stereo sound: its balance)
                let angle = (p0 + (p1 - p0) * f) * FRAC_PI_2;
                bus.l[from + k] += yl * gain * angle.cos() * SQRT_2;
                bus.r[from + k] += yr * gain * angle.sin() * SQRT_2;
            }
        }
    }

    fn choke_clips(&mut self) {
        for p in &mut self.players {
            p.voice.choke();
            p.stretch.choke();
            p.clip = None;
        }
    }

    /// Plugin instruments: one round trip each for the whole block, into their tracks.
    /// ponytail: each waits up to a block's length; share one deadline across plugins if
    /// a project with many slow plugins starts to crackle.
    fn render_plugins(&mut self, n: usize) {
        let song = &*self.song;
        let timeout = Duration::from_secs_f32(n as f32 / self.sr);
        let beats = self.pos / PPQ as f64;
        let (mut l, mut r) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        for ch in &song.channels {
            let (Some(Slot::Plugin), Some(Some(b))) = (self.slots.get(ch.slot as usize), self.plugins.get(ch.slot as usize)) else { continue };
            let ok = b.process(None, (&mut l[..n], &mut r[..n]), song.tempo as f64, beats, self.playing, timeout);
            if !ok || ch.mute {
                continue;
            }
            let g = self.chan_gain[ch.slot as usize].powi(2);
            let bus = &mut self.bus[ch.track];
            for i in 0..n {
                bus.l[i] += l[i] * g;
                bus.r[i] += r[i] * g;
            }
        }
    }

    /// Automation clips, then LFOs and macros → this block's device params, track levels and
    /// synth params.
    fn modulate(&mut self, frames: usize) {
        let song = &*self.song;
        let mut lfo = [0.0f32; MAX_LFOS];
        for (i, o) in song.lfos.iter().enumerate() {
            let beats = o.beats.max(1.0 / 16.0) as f64;
            // synced LFOs follow the playhead, so they line up with the bar every time
            let (phase, cycle) = if o.sync && self.playing {
                let at = self.pos / PPQ as f64 / beats;
                (at.fract(), at.floor() as u64)
            } else {
                (self.lfo_phase[i], self.lfo_cycle[i])
            };
            let hz = if o.sync { song.tempo as f64 / 60.0 / beats } else { o.hz as f64 };
            let next = phase + hz * frames as f64 / self.sr as f64;
            (self.lfo_phase[i], self.lfo_cycle[i]) = (next.fract(), cycle + next as u64);
            lfo[i] = if o.shape == 4 { held(i, cycle) } else { wave(o.shape, phase as f32) };
        }
        // automation clips under the playhead set their knob, then the lines of the patterns
        // sounding (wherever they are); modulation moves it from there
        let now = (self.playing && song.mode == Mode::Song).then_some(self.pos);
        let mut sounding = [(0usize, 0.0f64); 16];
        let mut count = 0;
        if self.playing {
            let mut add = |p: usize, at: f64| {
                if count < sounding.len() && song.patterns.get(p).is_some_and(|p| !p.autos.is_empty() && p.length > 0.0) {
                    sounding[count] = (p, at.rem_euclid(song.patterns[p].length));
                    count += 1;
                }
            };
            match song.mode {
                Mode::Pattern(p) => add(p, self.pos),
                Mode::Song => {
                    for c in song.clips.iter().take_while(|c| c.start <= self.pos) {
                        if let Content::Pattern(p) = c.content
                            && self.pos < c.end
                        {
                            add(p, self.pos - c.start + c.offset);
                        }
                    }
                }
            }
        }
        let sounding = &sounding[..count];
        let automated = |target: ModTarget| {
            song.autos
                .iter()
                .rev()
                .find_map(|a| {
                    let t = now.filter(|t| a.target == target && *t >= a.start && *t < a.end)?;
                    curve_at(&a.points, &a.curves, t - a.start + a.offset)
                })
                .or_else(|| sounding.iter().rev().find_map(|(p, at)| song.patterns[*p].autos.iter().find(|(t, _, _)| *t == target).and_then(|(_, points, curves)| curve_at(points, curves, *at))))
        };
        self.macros = song.macros;
        for (i, m) in self.macros.iter_mut().enumerate() {
            *m = automated(ModTarget::Macro(i)).unwrap_or(*m);
        }
        for ch in &song.channels {
            self.chan_gain[ch.slot as usize] = automated(ModTarget::ChannelGain(ch.slot)).unwrap_or(ch.gain);
        }
        let macros = self.macros;
        let value = |m: &SongMod| {
            m.amount
                * match m.source {
                    Source::Macro(i) => macros[i as usize],
                    Source::Lfo(i) => lfo[i as usize],
                }
        };
        self.dev_params.clear();
        self.dev_params.extend(song.devices.iter().map(|d| d.params)); // within capacity
        for (level, t) in self.levels.iter_mut().zip(&song.tracks) {
            *level = (t.gain, t.pan);
        }
        let lines = song.autos.iter().map(|a| a.target).chain(sounding.iter().flat_map(|(p, _)| song.patterns[*p].autos.iter().map(|(t, _, _)| *t)));
        for target in lines {
            let Some(v) = automated(target) else { continue };
            match target {
                ModTarget::Device { index, param } => {
                    if let Some(info) = song.devices[index].kind.params().get(param) {
                        self.dev_params[index][param] = info.from_norm(v);
                    }
                }
                ModTarget::Gain(t) => self.levels[t].0 = v * 1.25,
                ModTarget::Pan(t) => self.levels[t].1 = v * 2.0 - 1.0,
                ModTarget::Synth { .. } | ModTarget::ChannelGain(_) | ModTarget::Macro(_) => {}
            }
        }
        for m in &song.mods {
            match m.target {
                ModTarget::Device { index, param } => {
                    if let Some(info) = song.devices[index].kind.params().get(param) {
                        let p = &mut self.dev_params[index][param];
                        *p = info.from_norm(info.to_norm(*p) + value(m));
                    }
                }
                ModTarget::Gain(t) => self.levels[t].0 = (self.levels[t].0 + value(m) * 1.25).clamp(0.0, 1.25),
                ModTarget::Pan(t) => self.levels[t].1 = (self.levels[t].1 + value(m) * 2.0).clamp(-1.0, 1.0),
                ModTarget::ChannelGain(slot) => {
                    let g = &mut self.chan_gain[slot as usize];
                    *g = (*g + value(m)).clamp(0.0, 1.0);
                }
                ModTarget::Synth { .. } | ModTarget::Macro(_) => {}
            }
        }
        for ch in song.channels.iter().filter(|c| c.modulated) {
            let (Sound::Synth(base), Slot::Synth(s)) = (&ch.sound, &mut self.slots[ch.slot as usize]) else { continue };
            let mut p = *base;
            for (param, value) in p.iter_mut().enumerate() {
                if let Some(v) = automated(ModTarget::Synth { slot: ch.slot, param }) {
                    *value = PARAMS[param].from_norm(v);
                }
            }
            for m in &song.mods {
                if let ModTarget::Synth { slot, param } = m.target
                    && slot == ch.slot
                {
                    let info = &PARAMS[param];
                    p[param] = info.from_norm(info.to_norm(p[param]) + value(m));
                }
            }
            s.set_params(p);
        }
    }

    /// Runs the tracks in routing order: chain, EQ, fader, pan, then into the tracks they feed.
    /// The master lands in `l` / `r`.
    fn mix(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let song = &*self.song;
        for &t in &song.order {
            let track = &song.tracks[t];
            let Bus { l: mut bl, r: mut br } = self.bus[t];
            let (bl, br) = (&mut bl[..n], &mut br[..n]);
            // plugin effects never sleep: they may make sound from nothing, and they keep time
            let silent_in = bl.iter().chain(br.iter()).all(|x| *x == 0.0);
            let asleep = silent_in && self.quiet[t] as f32 >= self.sr && !track.devices.clone().any(|d| song.devices[d].kind == DeviceKind::Plugin);
            for d in track.devices.clone().filter(|_| !asleep) {
                let dev = &song.devices[d];
                if dev.kind == DeviceKind::Plugin {
                    if !dev.bypass
                        && let Some((_, b)) = self.device_plugins.iter().find(|(id, _)| *id == dev.id)
                    {
                        let (mut dl, mut dr) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
                        dl[..n].copy_from_slice(bl);
                        dr[..n].copy_from_slice(br);
                        let timeout = Duration::from_secs_f32(n as f32 / self.sr);
                        if !b.process(Some((&dl[..n], &dr[..n])), (&mut *bl, &mut *br), song.tempo as f64, self.pos / PPQ as f64, self.playing, timeout) {
                            bl.copy_from_slice(&dl[..n]); // late or gone: the sound passes through dry
                            br.copy_from_slice(&dr[..n]);
                        }
                    }
                    continue;
                }
                if let Some((_, fx)) = self.effects.iter_mut().find(|(id, _)| *id == dev.id)
                    && !dev.bypass
                {
                    let key = dev.key.map(|k| (&self.bus[k].l[..n], &self.bus[k].r[..n]));
                    fx.process(&self.dev_params[d], bl, br, song.tempo, key);
                }
            }
            if !asleep && track.eq != [0.0; 3] {
                self.strip_eq[t].strip(track.eq, bl, br);
            }
            if !asleep {
                let silent_out = bl.iter().chain(br.iter()).all(|x| x.abs() < 1e-6);
                self.quiet[t] = if silent_out { self.quiet[t].saturating_add(n as u32) } else { 0 };
            }
            // keep the processed, pre-fader sound: compressors keyed by this track read it
            self.bus[t].l[..n].copy_from_slice(bl);
            self.bus[t].r[..n].copy_from_slice(br);
            let (gain, pan) = self.levels[t];
            let g = if track.mute { 0.0 } else { gain };
            let target = (g * (1.0 - pan).min(1.0), g * (1.0 + pan).min(1.0));
            let (gl, gr) = match std::mem::replace(&mut self.applied[t], target) {
                (l, _) if l.is_nan() => target, // first block: no ramp in from nowhere
                last => last,
            };
            let mut peak = 0.0f32;
            for (i, (a, b)) in bl.iter_mut().zip(br.iter_mut()).enumerate() {
                let k = (i + 1) as f32 / n as f32; // ramp: fader moves never click
                *a *= gl + (target.0 - gl) * k;
                *b *= gr + (target.1 - gr) * k;
                peak = peak.max(a.abs()).max(b.abs());
            }
            self.meters[t] = self.meters[t].max(peak);
            if t == 0 {
                l.copy_from_slice(bl);
                r.copy_from_slice(br);
            }
            for &(to, level) in &track.outputs {
                let bus = &mut self.bus[to];
                for i in 0..n {
                    bus.l[i] += bl[i] * level;
                    bus.r[i] += br[i] * level;
                }
            }
        }
    }

    /// Peak level per mixer track since the last call (post-fader).
    pub fn take_meters(&mut self) -> [f32; MAX_TRACKS] {
        std::mem::replace(&mut self.meters, [0.0; MAX_TRACKS])
    }

    /// Playhead position in beats within the pattern (PAT) or song (SONG).
    pub fn beat(&self) -> f64 {
        self.pos / PPQ as f64
    }

    /// Keys currently held on the computer keyboard / piano / MIDI (bitmask of notes).
    pub fn held_notes(&self) -> u128 {
        self.held
    }
}

/// The last word before the device: untouched up to the knee, then bent smoothly toward full
/// scale. A mix that runs hot rounds off instead of clipping flat (a limiter on the master
/// keeps it clean); a blown-up effect gives silence, not a blast.
fn safe(x: f32) -> f32 {
    const KNEE: f32 = 0.9;
    if !x.is_finite() {
        return 0.0;
    }
    if x.abs() <= KNEE {
        return x;
    }
    x.signum() * (KNEE + (1.0 - KNEE) * ((x.abs() - KNEE) / (1.0 - KNEE)).tanh())
}

fn add(bus: &mut Bus, from: usize, l: &[f32], r: &[f32], gain: f32) {
    for (i, (a, b)) in l.iter().zip(r).enumerate() {
        bus.l[from + i] += a * gain;
        bus.r[from + i] += b * gain;
    }
}

/// The random LFO's level for one cycle, -1..1: the same every time the song gets there.
fn held(lfo: usize, cycle: u64) -> f32 {
    // splitmix64
    let mut x = cycle.wrapping_add((lfo as u64 + 1) << 40).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 40) as f32 / (1u64 << 23) as f32 - 1.0
}

/// LFO shapes, -1..1: sine, triangle, saw, square (same order as the synth waves).
fn wave(shape: u8, phase: f32) -> f32 {
    match shape {
        0 => (phase * TAU).sin(),
        1 => 1.0 - 4.0 * (phase - 0.5).abs(),
        2 => 2.0 * phase - 1.0,
        _ if phase < 0.5 => 1.0,
        _ => -1.0,
    }
}

/// Gathers events in `[pos, pos + span)` of the looping timeline. Returns the new position,
/// and whether the next block starts right after a wrap (so it picks up sounds mid-way).
fn collect(song: &Song, pos: f64, span: f64, seek: bool, out: &mut Vec<Pending>) -> (f64, bool) {
    // the loop region (or the whole timeline) wraps back to its start
    let (from, total) = song.span();
    let (mut at, mut rel, mut left, mut first) = (pos, 0.0, span, seek);
    if song.looping && at >= total {
        push(out, 0.0, Fire::Wrap);
        (at, first) = (from, true);
    }
    loop {
        if !song.looping && at >= total {
            return (at + left, false); // export: past the end, only tails ring
        }
        let seg = left.min(total - at);
        segment(song, at, at + seg, rel, first, out);
        first = false;
        rel += seg;
        left -= seg;
        if at + seg >= total - 1e-9 {
            if !song.looping {
                at = total;
                continue;
            }
            push(out, rel, Fire::Wrap);
            at = from;
            // round again: sounds that began before the loop's start pick up where it is
            first = true;
        } else {
            at += seg;
        }
        if left <= 1e-12 {
            return (at, first);
        }
    }
}

/// Events in timeline range `[a, b)`, at block offset `rel`.
fn segment(song: &Song, a: f64, b: f64, rel: f64, seek: bool, out: &mut Vec<Pending>) {
    // beats in [a - ε, b - ε): the ε keeps a beat on the loop's end out (it clicks again at
    // its start), and blocks tile without a gap: a beat a hair before a block's edge is the
    // next block's (with ε only at the top it fell between them: a missing click)
    let beat = PPQ as f64;
    let mut t = ((a - 1e-9) / beat).ceil() * beat;
    while t < b - 1e-9 {
        push(out, rel + (t - a).max(0.0), Fire::Click(((t / beat).round() as u64).is_multiple_of(4)));
        t += beat;
    }
    match song.mode {
        Mode::Pattern(p) => {
            if let Some(pat) = song.patterns.get(p) {
                pattern_events(pat, a, b - a, rel, out);
            }
        }
        Mode::Song => {
            for (i, clip) in song.clips.iter().enumerate() {
                if clip.end <= a || clip.start >= b {
                    continue;
                }
                let (lo, hi) = (a.max(clip.start), b.min(clip.end));
                match &clip.content {
                    Content::Pattern(p) => {
                        if let Some(pat) = song.patterns.get(*p) {
                            pattern_events(pat, lo - clip.start + clip.offset, hi - lo, rel + lo - a, out);
                        }
                        if clip.end <= b {
                            push(out, rel + clip.end - a, Fire::ClipEnd(*p));
                        }
                    }
                    Content::Audio(_) => {
                        if clip.start >= a {
                            push(out, rel + clip.start - a, Fire::AudioStart(i as u32, clip.offset));
                        } else if seek {
                            push(out, rel, Fire::AudioStart(i as u32, a - clip.start + clip.offset));
                        }
                        if clip.end <= b {
                            push(out, rel + clip.end - a, Fire::AudioStop(i as u32));
                        }
                    }
                }
            }
        }
    }
}

/// A pattern's events in local range `[from, from + len)`, looping the pattern.
fn pattern_events(pat: &SongPattern, from: f64, len: f64, rel: f64, out: &mut Vec<Pending>) {
    let length = pat.length;
    if length <= 0.0 {
        return;
    }
    let (mut t, mut r, end) = (from, rel, from + len);
    // No epsilon here: float drift can leave a clip a sliver of a block (~1e-13 ticks)
    // that still holds its first event; the next block starts past it.
    while t < end {
        let local = t % length;
        let seg = (end - t).min(length - local);
        if t + seg <= t {
            break; // too small to advance: nothing left to scan
        }
        // the slice reaching the pattern end also takes note-offs sitting exactly on it
        let closes = local + seg >= length - 1e-9;
        let first = pat.events.partition_point(|e| e.time < local);
        for e in &pat.events[first..] {
            let inside = e.time < local + seg || (closes && e.time <= length + 1e-9 && e.velocity == 0);
            if !inside {
                break;
            }
            push(out, r + e.time - local, Fire::Note(*e));
        }
        t += seg;
        r += seg;
    }
}

#[derive(Default)]
struct Click {
    phase: f32,
    step: f32,
    env: f32,
    decay: f32,
}

impl Click {
    fn trigger(&mut self, downbeat: bool, sr: f32) {
        self.step = if downbeat { 1760.0 } else { 1320.0 } / sr;
        self.phase = 0.0;
        self.env = 1.0;
        self.decay = (-1.0 / (0.012 * sr)).exp();
    }

    fn tick(&mut self) -> f32 {
        if self.env < 1e-4 {
            return 0.0;
        }
        let s = (self.phase * TAU).sin() * self.env * 0.3;
        self.phase = (self.phase + self.step).fract();
        self.env *= self.decay;
        s
    }
}
