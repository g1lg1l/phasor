//! Synthesizes Ravune's built-in drum kit into `assets/library/`.
//! Every sound is generated from scratch here, so the kit is CC0 by construction.
//!
//!     cargo run -p ravune-engine --example make_kit

use std::f32::consts::TAU;
use std::path::Path;

const SR: f32 = 44_100.0;

fn main() -> std::io::Result<()> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/library");
    std::fs::create_dir_all(&dir)?;
    let kit: [(&str, Vec<f32>); 13] = [
        ("kick", kick()),
        ("kick-909", kick_909()),
        ("noise-hit", noise_hit()),
        ("snare", snare()),
        ("clap", clap()),
        ("hat-closed", hat(0.035, 0.12)),
        ("hat-open", hat(0.22, 0.7)),
        ("rim", rim()),
        ("tom-low", tom(88.0)),
        ("tom-high", tom(150.0)),
        ("cowbell", cowbell()),
        ("shaker", shaker()),
        ("sub", sub()),
    ];
    for (name, frames) in kit {
        let path = dir.join(format!("{name}.wav"));
        ravune_engine::write_wav(&path, &normalize(frames), 1, SR as u32)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

/// Renders `seconds` of audio from a function of time.
fn make(seconds: f32, mut f: impl FnMut(f32) -> f32) -> Vec<f32> {
    (0..(seconds * SR) as usize).map(|i| f(i as f32 / SR)).collect()
}

fn normalize(mut x: Vec<f32>) -> Vec<f32> {
    let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs())).max(1e-9);
    x.iter_mut().for_each(|s| *s *= 0.89 / peak); // -1 dBFS
    x
}

fn decay(t: f32, tau: f32) -> f32 {
    (-t / tau).exp()
}

/// Deterministic white noise (xorshift32), so the kit is reproducible.
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

/// RBJ cookbook biquad.
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    fn new(kind: &str, freq: f32, q: f32) -> Self {
        let w = TAU * freq / SR;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b = match kind {
            "bp" => [alpha, 0.0, -alpha],
            _ => [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0], // highpass
        };
        Self { b: b.map(|v| v / a0), a: [-2.0 * cos / a0, (1.0 - alpha) / a0], z: [0.0; 2] }
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Phase accumulator for a sine with a moving frequency.
struct Osc(f32);
impl Osc {
    fn sine(&mut self, freq: f32) -> f32 {
        self.0 = (self.0 + freq / SR).fract();
        (self.0 * TAU).sin()
    }
    fn square(&mut self, freq: f32) -> f32 {
        self.0 = (self.0 + freq / SR).fract();
        if self.0 < 0.5 { 1.0 } else { -1.0 }
    }
}

fn kick() -> Vec<f32> {
    let (mut osc, mut noise) = (Osc(0.0), Noise(1));
    make(0.6, |t| {
        let body = osc.sine(45.0 + 110.0 * decay(t, 0.04)) * decay(t, 0.25);
        let click = noise.next() * decay(t, 0.002) * 0.3;
        (1.6 * (body + click)).tanh()
    })
}

/// 909-style: short and dry, the pitch dropping onto A1 (55 Hz) so its sub is in tune, a
/// noisy beater click on top.
fn kick_909() -> Vec<f32> {
    let (mut osc, mut noise) = (Osc(0.0), Noise(5));
    let mut hp = Biquad::new("hp", 3000.0, 0.7);
    make(0.45, |t| {
        let body = osc.sine(55.0 + 190.0 * decay(t, 0.03)) * decay(t, 0.16);
        let click = hp.run(noise.next()) * decay(t, 0.004) * 0.5;
        (2.2 * (body + click)).tanh()
    })
}

/// A noise burst whose band falls from 7 kHz to 1.5 kHz: a hit, not a riser.
fn noise_hit() -> Vec<f32> {
    let mut noise = Noise(6);
    let (mut bp, mut hp) = (Biquad::new("bp", 7000.0, 0.8), Biquad::new("hp", 900.0, 0.7));
    make(0.6, |t| {
        bp = Biquad { z: bp.z, ..Biquad::new("bp", 1500.0 + 5500.0 * decay(t, 0.12), 0.8) };
        let env = if t < 0.002 { t / 0.002 } else { decay(t - 0.002, 0.11) };
        hp.run(bp.run(noise.next())) * env
    })
}

fn snare() -> Vec<f32> {
    let (mut a, mut b, mut noise) = (Osc(0.0), Osc(0.0), Noise(2));
    let mut hp = Biquad::new("hp", 1800.0, 0.7);
    make(0.35, |t| {
        let tone = (a.sine(185.0) + 0.6 * b.sine(330.0)) * decay(t, 0.06) * 0.5;
        let rattle = hp.run(noise.next()) * decay(t, 0.11);
        tone + rattle
    })
}

fn clap() -> Vec<f32> {
    let mut noise = Noise(3);
    let mut bp = Biquad::new("bp", 1200.0, 1.4);
    make(0.4, |t| {
        let bursts: f32 = [0.0, 0.011, 0.022].iter().filter(|&&s| t >= s).map(|s| decay(t - s, 0.004)).sum();
        let tail = if t >= 0.03 { decay(t - 0.03, 0.09) } else { 0.0 };
        bp.run(noise.next()) * (bursts + tail)
    })
}

/// 808-style metallic hat: six detuned squares through a high band-pass.
fn hat(tau: f32, seconds: f32) -> Vec<f32> {
    const FREQS: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];
    let mut oscs = FREQS.map(|_| Osc(0.0));
    let (mut bp, mut hp) = (Biquad::new("bp", 10_000.0, 1.0), Biquad::new("hp", 7_000.0, 0.7));
    make(seconds, |t| {
        let metal: f32 = oscs.iter_mut().zip(FREQS).map(|(o, f)| o.square(f * 1.7)).sum::<f32>() / 6.0;
        hp.run(bp.run(metal)) * decay(t, tau)
    })
}

fn rim() -> Vec<f32> {
    let (mut a, mut b) = (Osc(0.0), Osc(0.0));
    make(0.08, |t| (2.0 * (a.sine(1720.0) + 0.7 * b.sine(470.0)) * decay(t, 0.008)).tanh())
}

fn tom(base: f32) -> Vec<f32> {
    let mut osc = Osc(0.0);
    make(0.5, |t| osc.sine(base + base * 0.45 * decay(t, 0.08)) * decay(t, 0.18))
}

fn cowbell() -> Vec<f32> {
    let (mut a, mut b) = (Osc(0.0), Osc(0.0));
    let mut bp = Biquad::new("bp", 2600.0, 1.2);
    make(0.4, |t| {
        let env = decay(t, 0.06) * 0.6 + decay(t, 0.25) * 0.4;
        bp.run(a.square(540.0) + b.square(800.0)) * env
    })
}

fn shaker() -> Vec<f32> {
    let mut noise = Noise(4);
    let mut hp = Biquad::new("hp", 6000.0, 0.7);
    make(0.15, |t| {
        let env = if t < 0.015 { t / 0.015 } else { decay(t - 0.015, 0.05) };
        hp.run(noise.next()) * env
    })
}

/// Long sub-bass boom, tuned near G1 so it sits under most keys.
fn sub() -> Vec<f32> {
    let mut osc = Osc(0.0);
    make(1.6, |t| (1.3 * osc.sine(49.0 + 30.0 * decay(t, 0.03)) * decay(t, 0.9)).tanh())
}
