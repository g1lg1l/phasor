//! VA-1: 16 voices × (2 polyBLEP oscillators → resonant low-pass → ADSR).
//!
//! MPE: every voice remembers its MIDI channel. Pitch bend, pressure and slide (CC 74) on
//! a member channel (2-16) move only the notes on it (bend ±48 semitones); on channel 1
//! they move every note (bend ±2), which is also how a plain keyboard's wheel works.
//! Pressure and slide open the filter.

use ravune_core::{PARAM_COUNT, PARAMS, Param};
use std::f32::consts::{PI, TAU};

const VOICES: usize = 16;

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Copy)]
struct Voice {
    note: u8,
    channel: u8,
    velocity: f32,
    stage: Stage,
    env: f32,
    phase1: f32,
    phase2: f32,
    // state-variable filter memory
    ic1: f32,
    ic2: f32,
    age: u64,
}

const IDLE: Voice = Voice {
    note: 0,
    channel: 0,
    velocity: 0.0,
    stage: Stage::Idle,
    env: 0.0,
    phase1: 0.0,
    phase2: 0.0,
    ic1: 0.0,
    ic2: 0.0,
    age: 0,
};

pub struct Synth {
    sr: f32,
    params: [f32; PARAM_COUNT],
    voices: [Voice; VOICES],
    clock: u64,
    gain: f32,
    /// Per MIDI channel: pitch bend -1..1, pressure 0..1, slide 0..1 (0.5 = neutral).
    bend: [f32; 16],
    pressure: [f32; 16],
    slide: [f32; 16],
}

impl Synth {
    pub fn new(sample_rate: f32) -> Self {
        Self { sr: sample_rate, params: PARAMS.map(|p| p.default), voices: [IDLE; VOICES], clock: 0, gain: 0.0, bend: [0.0; 16], pressure: [0.0; 16], slide: [0.5; 16] }
    }

    pub fn set_params(&mut self, params: [f32; PARAM_COUNT]) {
        self.params = params;
    }

    pub fn note_on_ch(&mut self, note: u8, velocity: f32, channel: u8) {
        self.clock += 1;
        let channel = channel & 15;
        let v = &self.voices;
        let i = v
            .iter()
            .position(|v| v.note == note && v.channel == channel && v.stage != Stage::Idle)
            .or_else(|| v.iter().position(|v| v.stage == Stage::Idle))
            // steal: prefer releasing voices, then the oldest
            .unwrap_or_else(|| (0..VOICES).min_by_key(|&i| (v[i].stage != Stage::Release, v[i].age)).unwrap());
        let v = &mut self.voices[i];
        if v.stage == Stage::Idle {
            *v = IDLE;
        }
        v.note = note;
        v.channel = channel;
        v.velocity = velocity.clamp(0.0, 1.0);
        v.stage = Stage::Attack;
        v.age = self.clock;
    }

    pub fn note_off_ch(&mut self, note: u8, channel: u8) {
        for v in &mut self.voices {
            if v.note == note && v.channel == channel & 15 && matches!(v.stage, Stage::Attack | Stage::Decay | Stage::Sustain) {
                v.stage = Stage::Release;
            }
        }
    }

    /// Pitch bend on a channel, -1..1.
    pub fn bend(&mut self, channel: u8, value: f32) {
        self.bend[channel as usize & 15] = value.clamp(-1.0, 1.0);
    }

    /// Channel pressure (aftertouch), 0..1.
    pub fn pressure(&mut self, channel: u8, value: f32) {
        self.pressure[channel as usize & 15] = value.clamp(0.0, 1.0);
    }

    /// MPE slide (CC 74), 0..1.
    pub fn slide(&mut self, channel: u8, value: f32) {
        self.slide[channel as usize & 15] = value.clamp(0.0, 1.0);
    }

    pub fn all_notes_off(&mut self) {
        for v in &mut self.voices {
            if v.stage != Stage::Idle {
                v.stage = Stage::Release;
            }
        }
    }

    /// Adds the synth's output to `out` (mono).
    pub fn render(&mut self, out: &mut [f32]) {
        let p = |x: Param| self.params[x as usize];
        let sr = self.sr;
        let (wave1, wave2) = (p(Param::Osc1Wave) as u8, p(Param::Osc2Wave) as u8);
        let ratio2 = ((p(Param::Osc2Pitch) + p(Param::Osc2Detune) / 100.0) / 12.0).exp2();
        let mix = p(Param::OscMix);
        let cutoff = p(Param::Cutoff);
        let k = 2.0 - 1.9 * p(Param::Resonance);
        let env_octaves = p(Param::EnvAmount) * 5.0;
        let attack_step = 1.0 / (p(Param::Attack) * sr);
        let decay = falloff(p(Param::Decay), sr);
        let release = falloff(p(Param::Release), sr);
        let sustain = p(Param::Sustain);
        let max_fc = 0.45 * sr;
        let (bend, pressure, slide) = (self.bend, self.pressure, self.slide);

        let mut tmp = [0.0f32; 512];
        for chunk in out.chunks_mut(tmp.len()) {
            let tmp = &mut tmp[..chunk.len()];
            tmp.fill(0.0);
            for v in self.voices.iter_mut().filter(|v| v.stage != Stage::Idle) {
                let c = v.channel as usize;
                let semitones = bend[0] * 2.0 + if c > 0 { bend[c] * 48.0 } else { 0.0 };
                let dt1 = (midi_hz(v.note) * (semitones / 12.0).exp2() / sr).min(0.49);
                // pressure (up to +2 octaves) and slide (±1.5) open the filter
                let bright = (pressure[0].max(pressure[c]) * 2.0 + (slide[c] - 0.5) * 3.0).exp2();
                let dt2 = (dt1 * ratio2).min(0.49);
                for s in tmp.iter_mut() {
                    match v.stage {
                        Stage::Attack => {
                            v.env += attack_step;
                            if v.env >= 1.0 {
                                v.env = 1.0;
                                v.stage = Stage::Decay;
                            }
                        }
                        Stage::Decay => {
                            v.env = sustain + (v.env - sustain) * decay;
                            if (v.env - sustain).abs() < 1e-4 {
                                v.stage = Stage::Sustain;
                            }
                        }
                        Stage::Sustain => v.env = sustain,
                        Stage::Release => {
                            v.env *= release;
                            if v.env < 1e-4 {
                                *v = IDLE;
                                break;
                            }
                        }
                        Stage::Idle => break,
                    }
                    let x = osc(wave1, v.phase1, dt1) * (1.0 - mix) + osc(wave2, v.phase2, dt2) * mix;
                    v.phase1 = wrap(v.phase1 + dt1);
                    v.phase2 = wrap(v.phase2 + dt2);

                    // Zero-delay-feedback state-variable low-pass (Simper/Cytomic).
                    let fc = (cutoff * bright * (env_octaves * v.env).exp2()).min(max_fc);
                    let g = (PI * fc / sr).tan();
                    let a1 = 1.0 / (1.0 + g * (g + k));
                    let v3 = x - v.ic2;
                    let v1 = a1 * v.ic1 + g * a1 * v3;
                    let v2 = v.ic2 + g * v1;
                    v.ic1 = 2.0 * v1 - v.ic1;
                    v.ic2 = 2.0 * v2 - v.ic2;

                    *s += v2 * v.env * v.velocity;
                }
            }
            let target = p(Param::Gain) * p(Param::Gain) * 0.35;
            for (o, s) in chunk.iter_mut().zip(tmp.iter()) {
                self.gain += (target - self.gain) * 0.002; // ~10 ms smoothing, no zipper
                *o += s * self.gain;
            }
        }
    }
}

/// Per-sample multiplier that falls 60 dB in `seconds`.
fn falloff(seconds: f32, sr: f32) -> f32 {
    (-6.9 / (seconds * sr)).exp()
}

fn midi_hz(note: u8) -> f32 {
    440.0 * ((note as f32 - 69.0) / 12.0).exp2()
}

fn wrap(phase: f32) -> f32 {
    if phase >= 1.0 { phase - 1.0 } else { phase }
}

/// Band-limited step correction (polyBLEP).
fn blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

fn osc(wave: u8, phase: f32, dt: f32) -> f32 {
    match wave {
        0 => (phase * TAU).sin(),
        1 => 1.0 - 4.0 * (phase - 0.5).abs(),
        2 => 2.0 * phase - 1.0 - blep(phase, dt),
        _ => {
            let square = if phase < 0.5 { 1.0 } else { -1.0 };
            square + blep(phase, dt) - blep(wrap(phase + 0.5), dt)
        }
    }
}
