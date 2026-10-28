//! Built-in stereo effects. Buffers are allocated in [`Effect::new`] on the UI thread;
//! `process` never allocates. Parameters arrive every block (already modulated), in the
//! units of `DeviceKind::params()`.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::f32::consts::{PI, TAU};

use ravune_core::{DeviceKind, MAX_DEVICE_PARAMS};

use crate::{Msg, Song};

pub type Params = [f32; MAX_DEVICE_PARAMS];

pub enum Effect {
    Eq(Eq3),
    Filter(Filter),
    Compressor(Compressor),
    Distortion(Saturator),
    Chorus(Chorus),
    Delay(Delay),
    Reverb(Reverb),
    Limiter(Limiter),
}

impl Effect {
    /// `None` for plugin devices: their sound comes from a host process, not from here.
    pub fn new(kind: DeviceKind, sr: f32) -> Option<Self> {
        Some(match kind {
            DeviceKind::Eq => Effect::Eq(Eq3::new(sr)),
            DeviceKind::Filter => Effect::Filter(Filter { sr, ic: [[0.0; 2]; 2] }),
            DeviceKind::Compressor => Effect::Compressor(Compressor { sr, env: 0.0 }),
            DeviceKind::Distortion => Effect::Distortion(Saturator { sr, lp: [0.0; 2] }),
            DeviceKind::Chorus => Effect::Chorus(Chorus { sr, lines: [Line::new(sr * 0.05), Line::new(sr * 0.05)], phase: 0.0 }),
            DeviceKind::Delay => Effect::Delay(Delay { sr, lines: [Line::new(sr * 4.0), Line::new(sr * 4.0)] }),
            DeviceKind::Reverb => Effect::Reverb(Reverb::new(sr)),
            DeviceKind::Limiter => Effect::Limiter(Limiter::new(sr)),
            DeviceKind::Plugin => return None,
        })
    }

    /// `key`: a sidechain signal (compressor only); otherwise it listens to itself.
    pub fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32], tempo: f32, key: Option<(&[f32], &[f32])>) {
        match self {
            Effect::Eq(e) => e.process(p, l, r),
            Effect::Filter(e) => e.process(p, l, r),
            Effect::Compressor(e) => e.process(p, l, r, key),
            Effect::Distortion(e) => e.process(p, l, r),
            Effect::Chorus(e) => e.process(p, l, r),
            Effect::Delay(e) => e.process(p, l, r, tempo),
            Effect::Reverb(e) => e.process(p, l, r),
            Effect::Limiter(e) => e.process(p, l, r),
        }
    }
}

/// UI side: creates and retires effect instances so the audio thread holds exactly one
/// per device in the song. The state (delay lines, reverb tails) lives as long as the id.
#[derive(Default)]
pub struct Effects {
    live: HashMap<u32, DeviceKind>,
}

impl Effects {
    pub fn messages(&mut self, song: &Song, sr: f32) -> Vec<Msg> {
        let mut out = Vec::new();
        self.live.retain(|id, kind| {
            // a different project can reuse an id for another kind
            let keep = song.devices.iter().any(|d| d.id == *id && d.kind == *kind);
            if !keep {
                out.push(Msg::RemoveEffect(*id));
            }
            keep
        });
        for d in &song.devices {
            if let Entry::Vacant(slot) = self.live.entry(d.id)
                && let Some(fx) = Effect::new(d.kind, sr)
            {
                slot.insert(d.kind);
                out.push(Msg::AddEffect(d.id, Box::new(fx)));
            }
        }
        out
    }
}

fn mix(dry: f32, wet: f32, amount: f32) -> f32 {
    dry + (wet - dry) * amount
}

fn db(x: f32) -> f32 {
    10f32.powf(x / 20.0)
}

/// One-pole coefficient for a time constant in seconds.
fn coeff(seconds: f32, sr: f32) -> f32 {
    (-1.0 / (seconds.max(1e-5) * sr)).exp()
}

// ---------------------------------------------------------------------------
// EQ Three: low shelf, peak, high shelf (RBJ biquads)
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

enum Shape {
    LowShelf,
    Peak,
    HighShelf,
}

impl Biquad {
    /// Sets the coefficients, keeping the filter memory.
    fn set(&mut self, shape: Shape, hz: f32, gain_db: f32, sr: f32) {
        let a = 10f32.powf(gain_db / 40.0);
        let w = TAU * hz.min(sr * 0.45) / sr;
        let (cos, sin) = (w.cos(), w.sin());
        let (b, a0, a1, a2) = match shape {
            Shape::Peak => {
                let alpha = sin / (2.0 * 0.9);
                ([1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a], 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a)
            }
            Shape::LowShelf | Shape::HighShelf => {
                // shelf slope 1: 2·√A·α = √A·sin·√2
                let s = a.sqrt() * sin * 2f32.sqrt();
                let sign = if matches!(shape, Shape::LowShelf) { 1.0 } else { -1.0 };
                let (p, m) = (a + 1.0, a - 1.0);
                (
                    [a * (p - sign * m * cos + s), 2.0 * sign * a * (m - sign * p * cos), a * (p - sign * m * cos - s)],
                    p + sign * m * cos + s,
                    -2.0 * sign * (m + sign * p * cos),
                    p + sign * m * cos - s,
                )
            }
        };
        self.b = b.map(|x| x / a0);
        self.a = [a1 / a0, a2 / a0];
    }

    fn tick(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

pub struct Eq3 {
    sr: f32,
    /// [band][channel]
    bands: [[Biquad; 2]; 3],
}

impl Eq3 {
    pub(crate) fn new(sr: f32) -> Self {
        Self { sr, bands: Default::default() }
    }

    /// A mixer strip's EQ: low, mid (at 1 kHz) and high, in dB.
    pub(crate) fn strip(&mut self, [low, mid, high]: [f32; 3], l: &mut [f32], r: &mut [f32]) {
        let mut p = [0.0; MAX_DEVICE_PARAMS];
        p[..4].copy_from_slice(&[low, 1000.0, mid, high]);
        self.process(&p, l, r);
    }

    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let settings = [(Shape::LowShelf, 120.0, p[0]), (Shape::Peak, p[1], p[2]), (Shape::HighShelf, 6000.0, p[3])];
        for (band, (shape, hz, gain)) in self.bands.iter_mut().zip(settings) {
            if gain == 0.0 {
                continue; // flat band: skip it entirely
            }
            let mut q = band[0];
            q.set(shape, hz, gain, self.sr);
            for (f, buf) in band.iter_mut().zip([&mut *l, &mut *r]) {
                (f.b, f.a) = (q.b, q.a);
                buf.iter_mut().for_each(|x| *x = f.tick(*x));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Filter: state-variable (ZDF), low / high / band
// ---------------------------------------------------------------------------

pub struct Filter {
    sr: f32,
    ic: [[f32; 2]; 2],
}

impl Filter {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let g = (PI * p[1].min(self.sr * 0.45) / self.sr).tan();
        let k = 2.0 - 1.96 * p[2];
        let kind = p[0] as u8;
        for (ic, buf) in self.ic.iter_mut().zip([l, r]) {
            for x in buf.iter_mut() {
                *x = svf(ic, *x, g, k, kind);
            }
        }
    }
}

/// One sample through a state-variable filter (ZDF): `g` = tan(π·fc/sr), `k` = 1/Q,
/// `kind` 0 = low-pass, 1 = high-pass, else band-pass.
pub(crate) fn svf(ic: &mut [f32; 2], x: f32, g: f32, k: f32, kind: u8) -> f32 {
    let a1 = 1.0 / (1.0 + g * (g + k));
    let v3 = x - ic[1];
    let v1 = a1 * ic[0] + g * a1 * v3;
    let v2 = ic[1] + g * v1;
    ic[0] = 2.0 * v1 - ic[0];
    ic[1] = 2.0 * v2 - ic[1];
    match kind {
        0 => v2,
        1 => x - k * v1 - v2,
        _ => v1,
    }
}

// ---------------------------------------------------------------------------
// Compressor: stereo-linked, peak detection, gain computed in dB
// ---------------------------------------------------------------------------

pub struct Compressor {
    sr: f32,
    /// Current gain reduction, dB.
    env: f32,
}

impl Compressor {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32], key: Option<(&[f32], &[f32])>) {
        let [threshold, ratio, attack, release, makeup, ..] = *p;
        let (att, rel) = (coeff(attack, self.sr), coeff(release, self.sr));
        let slope = 1.0 - 1.0 / ratio.max(1.0);
        for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let peak = match key {
                Some((kl, kr)) => kl[i].abs().max(kr[i].abs()),
                None => a.abs().max(b.abs()),
            };
            let level = 20.0 * (peak + 1e-9).log10();
            let target = (level - threshold).max(0.0) * slope;
            let c = if target > self.env { att } else { rel };
            self.env = target + (self.env - target) * c;
            let g = db(makeup - self.env);
            *a *= g;
            *b *= g;
        }
    }
}

// ---------------------------------------------------------------------------
// Saturator: tanh drive, tone low-pass, dry/wet
// ---------------------------------------------------------------------------

pub struct Saturator {
    sr: f32,
    lp: [f32; 2],
}

impl Saturator {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let drive = db(p[0]);
        let trim = 1.0 / drive.sqrt(); // keeps the level roughly steady as drive goes up
        let c = 1.0 - (-TAU * p[1] / self.sr).exp();
        for (lp, buf) in self.lp.iter_mut().zip([l, r]) {
            for x in buf.iter_mut() {
                *lp += c * ((*x * drive).tanh() * trim - *lp);
                *x = mix(*x, *lp, p[2]);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Delay lines (chorus, delay)
// ---------------------------------------------------------------------------

struct Line {
    buf: Vec<f32>,
    w: usize,
}

impl Line {
    fn new(samples: f32) -> Self {
        Self { buf: vec![0.0; samples as usize + 2], w: 0 }
    }

    /// The sample written `delay` samples ago (fractional, linear interpolation).
    fn read(&self, delay: f32) -> f32 {
        let n = self.buf.len();
        let d = delay.clamp(1.0, (n - 2) as f32);
        let pos = (self.w + n) as f32 - d;
        let (i, frac) = (pos as usize, pos.fract());
        let (a, b) = (self.buf[i % n], self.buf[(i + 1) % n]);
        a + (b - a) * frac
    }

    fn write(&mut self, x: f32) {
        self.buf[self.w] = x;
        self.w = (self.w + 1) % self.buf.len();
    }
}

pub struct Chorus {
    sr: f32,
    lines: [Line; 2],
    phase: f32,
}

impl Chorus {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let (base, depth) = (0.012 * self.sr, p[1] * 0.006 * self.sr);
        let step = p[0] / self.sr;
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            for (i, x) in [a, b].into_iter().enumerate() {
                let lfo = ((self.phase + i as f32 * 0.25) * TAU).sin();
                let line = &mut self.lines[i];
                line.write(*x);
                *x = mix(*x, line.read(base + depth * lfo), p[2]);
            }
            self.phase = (self.phase + step).fract();
        }
    }
}

pub struct Delay {
    sr: f32,
    lines: [Line; 2],
}

impl Delay {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32], tempo: f32) {
        // ponytail: lines hold 4 s, so very slow tempos clamp long times
        let time = p[0] * 15.0 / tempo.max(1.0) * self.sr;
        let (feedback, ping_pong, amount) = (p[1], p[2] >= 0.5, p[3]);
        let [left, right] = &mut self.lines;
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let (dl, dr) = (left.read(time), right.read(time));
            if ping_pong {
                left.write((*a + *b) * 0.5 + dr * feedback);
                right.write(dl * feedback);
            } else {
                left.write(*a + dl * feedback);
                right.write(*b + dr * feedback);
            }
            *a = mix(*a, dl, amount);
            *b = mix(*b, dr, amount);
        }
    }
}

// ---------------------------------------------------------------------------
// Reverb: Freeverb (8 damped combs + 4 all-passes per side)
// ---------------------------------------------------------------------------

const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
const SPREAD: usize = 23;

struct Comb {
    buf: Vec<f32>,
    i: usize,
    store: f32,
}

struct Allpass {
    buf: Vec<f32>,
    i: usize,
}

pub struct Reverb {
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
}

impl Reverb {
    fn new(sr: f32) -> Self {
        // tunings are in samples at 44.1 kHz; the right side is detuned for width
        let len = |n: usize, side: usize| ((n + side * SPREAD) as f32 * sr / 44_100.0) as usize + 1;
        let combs = |s| COMBS.iter().map(|&n| Comb { buf: vec![0.0; len(n, s)], i: 0, store: 0.0 }).collect();
        let allpasses = |s| ALLPASSES.iter().map(|&n| Allpass { buf: vec![0.0; len(n, s)], i: 0 }).collect();
        Self { combs: [combs(0), combs(1)], allpasses: [allpasses(0), allpasses(1)] }
    }

    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let feedback = 0.7 + 0.28 * p[0];
        let damp = 0.4 * p[1];
        let (wet1, wet2) = (p[2] / 2.0 + 0.5, (1.0 - p[2]) / 2.0);
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let input = (*a + *b) * 0.015;
            let mut out = [0.0f32; 2];
            for (side, o) in out.iter_mut().enumerate() {
                for c in &mut self.combs[side] {
                    let y = c.buf[c.i];
                    c.store = y * (1.0 - damp) + c.store * damp;
                    c.buf[c.i] = input + c.store * feedback;
                    c.i = (c.i + 1) % c.buf.len();
                    *o += y;
                }
                for ap in &mut self.allpasses[side] {
                    let y = ap.buf[ap.i];
                    ap.buf[ap.i] = *o + y * 0.5;
                    ap.i = (ap.i + 1) % ap.buf.len();
                    *o = y - *o;
                }
            }
            let (wl, wr) = (out[0] * wet1 + out[1] * wet2, out[1] * wet1 + out[0] * wet2);
            *a = mix(*a, wl * 3.0, p[3]);
            *b = mix(*b, wr * 3.0, p[3]);
        }
    }
}

// ---------------------------------------------------------------------------
// Limiter: 1.5 ms look-ahead, so the gain is already down when a peak arrives; smooth
// release, hard ceiling. It delays its track by the look-ahead.
// ---------------------------------------------------------------------------

/// Seconds the limiter sees ahead (and delays the sound).
pub const LOOK_AHEAD: f32 = 0.0015;

pub struct Limiter {
    sr: f32,
    /// The sound, delayed by the look-ahead.
    delay: [Vec<f32>; 2],
    /// The gain each of the last samples needs, and the lowest of those over the look-ahead.
    need: Vec<f32>,
    lowest: Vec<f32>,
    at: usize,
    gain: f32,
}

impl Limiter {
    fn new(sr: f32) -> Self {
        let n = ((LOOK_AHEAD * sr).round() as usize).max(1);
        Self { sr, delay: [vec![0.0; n], vec![0.0; n]], need: vec![1.0; n], lowest: vec![1.0; n], at: 0, gain: 1.0 }
    }

    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let ceiling = db(p[0]);
        let release = coeff(p[1], self.sr);
        let n = self.need.len();
        // ponytail: plain scans over the look-ahead (~70 samples); a monotonic queue if it ever shows up in the profile
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            self.need[self.at] = ceiling / a.abs().max(b.abs()).max(ceiling);
            // the lowest gain any sample in the window needs, averaged over the window: a ramp
            // that reaches it exactly as that sample leaves the delay
            self.lowest[self.at] = self.need.iter().copied().fold(1.0, f32::min);
            let target = self.lowest.iter().sum::<f32>() / n as f32;
            self.gain = if target < self.gain { target } else { target + (self.gain - target) * release };
            let (dl, dr) = (std::mem::replace(&mut self.delay[0][self.at], *a), std::mem::replace(&mut self.delay[1][self.at], *b));
            *a = (dl * self.gain).clamp(-ceiling, ceiling);
            *b = (dr * self.gain).clamp(-ceiling, ceiling);
            self.at = (self.at + 1) % n;
        }
    }
}
