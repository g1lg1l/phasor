//! What a render sounds like, in numbers an agent (or a test) can read: level, balance across
//! the spectrum, stereo width, onsets, key and tempo. Time-domain only: biquads for the
//! bands, Goertzel for the pitch classes.

use std::f32::consts::TAU;

use crate::{Sample, detect_bpm};

/// Bands of [`Analysis::bands`]: sub, low, mid, high, air.
pub const BANDS: [(&str, f32, f32); 5] = [("sub", 20.0, 60.0), ("low", 60.0, 250.0), ("mid", 250.0, 2000.0), ("high", 2000.0, 8000.0), ("air", 8000.0, 20000.0)];
pub const KEYS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub seconds: f32,
    pub peak_db: f32,
    pub rms_db: f32,
    /// Each band's share of the energy, dB (0 = all of it), in [`BANDS`] order.
    pub bands: [f32; 5],
    /// Side energy over mid energy, dB: very low is mono, around 0 is very wide.
    pub width_db: f32,
    /// When something hits, in seconds.
    pub onsets: Vec<f32>,
    /// The best-fitting key: pitch class (0 = C), minor, and how well it fits (-1..1).
    pub key: (u8, bool, f32),
    pub bpm: Option<f32>,
}

impl Analysis {
    pub fn key_name(&self) -> String {
        format!("{} {}", KEYS[self.key.0 as usize % 12], if self.key.1 { "minor" } else { "major" })
    }
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

/// RBJ biquad, low- or high-pass.
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    fn new(high: bool, freq: f32, rate: f32) -> Self {
        let w = TAU * (freq / rate).min(0.49);
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b = if high { [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0] } else { [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0] };
        Self { b: b.map(|v| v / a0), a: [-2.0 * cos / a0, (1.0 - alpha) / a0], z: [0.0; 2] }
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Analyzes interleaved stereo `frames` at `rate`.
pub fn analyze(frames: &[f32], rate: f32) -> Analysis {
    let n = frames.len() / 2;
    let mono: Vec<f32> = frames.as_chunks::<2>().0.iter().map(|[l, r]| (l + r) * 0.5).collect();
    let peak = frames.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let energy = |x: &[f32]| x.iter().map(|s| s * s).sum::<f32>();
    let total = energy(frames) / frames.len().max(1) as f32;
    let side = frames.as_chunks::<2>().0.iter().map(|[l, r]| ((l - r) * 0.5).powi(2)).sum::<f32>();

    // each band: a high-pass at its bottom, a low-pass at its top (two poles each)
    let mono_energy = energy(&mono).max(1e-12);
    let bands = BANDS.map(|(_, lo, hi)| {
        let (mut h, mut l) = (Biquad::new(true, lo, rate), Biquad::new(false, hi.min(rate * 0.45), rate));
        let e: f32 = mono.iter().map(|&x| l.run(h.run(x)).powi(2)).sum();
        10.0 * (e / mono_energy).max(1e-12).log10()
    });

    // onsets: the level every 10 ms jumping well above the last 100 ms
    let hop = ((rate / 100.0) as usize).max(1);
    let env: Vec<f32> = mono.chunks(hop).map(|c| (energy(c) / c.len() as f32).sqrt()).collect();
    let mut onsets = Vec::new();
    let mut last = -1.0f32;
    for i in 1..env.len() {
        let from = i.saturating_sub(10);
        let recent = env[from..i].iter().sum::<f32>() / (i - from) as f32;
        let t = i as f32 * hop as f32 / rate;
        if env[i] > 1.6 * recent + 1e-4 && env[i] > env[i - 1] && db(env[i]) > -45.0 && t - last > 0.05 {
            onsets.push(t);
            last = t;
        }
    }

    Analysis {
        seconds: n as f32 / rate,
        peak_db: db(peak),
        rms_db: db(total.sqrt()),
        bands,
        width_db: 10.0 * (side / mono_energy).max(1e-12).log10(),
        onsets,
        key: key(&mono, rate),
        bpm: detect_bpm(&Sample::new(mono, rate), ""),
    }
}

/// Krumhansl–Schmuckler: the pitch-class profile (C2–B5, by Goertzel on a 4× decimated
/// signal) against the major and minor key profiles, every root.
fn key(mono: &[f32], rate: f32) -> (u8, bool, f32) {
    const MAJOR: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    const MINOR: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    let mut lp = Biquad::new(false, rate / 10.0, rate);
    let small: Vec<f32> = mono.iter().map(|&x| lp.run(x)).step_by(4).collect();
    let sr = rate / 4.0;
    let mut chroma = [0.0f32; 12];
    for note in 36..84u32 {
        let f = 440.0 * ((note as f32 - 69.0) / 12.0).exp2();
        let k = 2.0 * (TAU * f / sr).cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &x in &small {
            let s0 = x + k * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        chroma[(note % 12) as usize] += (s1 * s1 + s2 * s2 - k * s1 * s2).max(0.0).sqrt();
    }
    let corr = |profile: &[f32; 12], root: usize| {
        let p: Vec<f32> = (0..12).map(|i| profile[(i + 12 - root) % 12]).collect();
        let (mx, my) = (chroma.iter().sum::<f32>() / 12.0, p.iter().sum::<f32>() / 12.0);
        let num: f32 = (0..12).map(|i| (chroma[i] - mx) * (p[i] - my)).sum();
        let den = ((0..12).map(|i| (chroma[i] - mx).powi(2)).sum::<f32>() * (0..12).map(|i| (p[i] - my).powi(2)).sum::<f32>()).sqrt();
        if den > 0.0 { num / den } else { 0.0 }
    };
    let mut best = (0u8, false, f32::MIN);
    for root in 0..12 {
        for (minor, profile) in [(false, &MAJOR), (true, &MINOR)] {
            let c = corr(profile, root);
            if c > best.2 {
                best = (root as u8, minor, c);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An A minor arpeggio of plucks, two a second, the right side a little quieter.
    #[test]
    fn hears_key_hits_and_balance() {
        let rate = 22_050.0;
        let mut frames = vec![0.0f32; (rate * 8.0) as usize * 2];
        for (k, key) in [57, 60, 64, 69, 57, 60, 64, 72].iter().cycle().take(16).enumerate() {
            let f = 440.0 * ((*key as f32 - 69.0) / 12.0).exp2();
            let start = (k as f32 * 0.5 * rate) as usize;
            for i in 0..(0.4 * rate) as usize {
                let t = i as f32 / rate;
                let s = (TAU * f * t).sin() * (-t * 6.0).exp() * 0.5;
                frames[2 * (start + i)] += s;
                frames[2 * (start + i) + 1] += s * 0.8;
            }
        }
        let a = analyze(&frames, rate);
        assert_eq!(a.key_name(), "A minor", "{a:?}");
        assert!((14..=18).contains(&a.onsets.len()), "one onset per pluck: {:?}", a.onsets);
        assert!(a.bands[2] > a.bands[0] && a.bands[2] > a.bands[4], "notes in the mids: {:?}", a.bands);
        assert!(a.width_db < -15.0, "nearly mono: {}", a.width_db);
        assert!((a.peak_db - -6.0).abs() < 1.0 && a.rms_db < a.peak_db);
    }
}
