//! Smart helpers (M9), offline and without models: a sound split into its drums and its
//! tones, and a melody heard as notes.
//!
//! - [`split_hpss`]: harmonic / percussive separation (Fitzgerald, 2010). In the spectrogram
//!   tones are horizontal lines and hits are vertical ones; a median along time keeps the
//!   tones, one along frequency keeps the hits, and soft masks share every bin out.
//! - [`melody_notes`]: one voice (a bassline, a vocal, a lead) to notes: YIN pitch every
//!   few milliseconds, held pitches grouped into notes.
//!
//! ponytail: no neural models (Demucs, Basic Pitch): two parts, not four stems, and one note
//! at a time; an ONNX extension can replace either later.

use std::f32::consts::PI;

use realfft::RealFftPlanner;
use realfft::num_complex::Complex;

const FRAME: usize = 2048;
const HOP: usize = 512;
const MEDIAN: usize = 17;

fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos()).collect()
}

fn median(values: &mut [f32]) -> f32 {
    let mid = values.len() / 2;
    *values.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1
}

/// (percussive, harmonic): the hits and the tones of `mono`, each as long as it, summing
/// back to it (up to the window's edges).
pub fn split_hpss(mono: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let n = mono.len();
    if n < FRAME {
        return (mono.to_vec(), vec![0.0; n]);
    }
    let window = hann(FRAME);
    let mut planner = RealFftPlanner::<f32>::new();
    let (fft, ifft) = (planner.plan_fft_forward(FRAME), planner.plan_fft_inverse(FRAME));
    let bins = FRAME / 2 + 1;
    // frames centred on the hops, the signal padded half a frame each side
    let padded: Vec<f32> = std::iter::repeat_n(0.0, FRAME / 2).chain(mono.iter().copied()).chain(std::iter::repeat_n(0.0, FRAME)).collect();
    let frames = n / HOP + 1;
    let mut spectra: Vec<Vec<Complex<f32>>> = Vec::with_capacity(frames);
    let mut input = fft.make_input_vec();
    for f in 0..frames {
        let at = f * HOP;
        input.iter_mut().zip(&padded[at..at + FRAME]).zip(&window).for_each(|((x, s), w)| *x = s * w);
        let mut out = fft.make_output_vec();
        fft.process(&mut input, &mut out).expect("frame sizes match the plan");
        spectra.push(out);
    }
    let mag: Vec<Vec<f32>> = spectra.iter().map(|s| s.iter().map(|c| c.norm()).collect()).collect();
    let half = MEDIAN / 2;
    let mut scratch = Vec::with_capacity(MEDIAN);
    // the tones: each bin's median over neighboring frames; the hits: each frame's over neighboring bins
    let mut harmonic = vec![vec![0.0f32; bins]; frames];
    for b in 0..bins {
        for (f, row) in harmonic.iter_mut().enumerate() {
            scratch.clear();
            scratch.extend((f.saturating_sub(half)..(f + half + 1).min(frames)).map(|g| mag[g][b]));
            row[b] = median(&mut scratch);
        }
    }
    let mut percussive = vec![vec![0.0f32; bins]; frames];
    for f in 0..frames {
        for b in 0..bins {
            scratch.clear();
            scratch.extend_from_slice(&mag[f][b.saturating_sub(half)..(b + half + 1).min(bins)]);
            percussive[f][b] = median(&mut scratch);
        }
    }
    // soft (Wiener) masks, then back to sound, overlap-added under the same window
    let mut outs = [vec![0.0f32; n + 2 * FRAME], vec![0.0f32; n + 2 * FRAME]];
    let mut norm = vec![0.0f32; n + 2 * FRAME];
    let mut spectrum = ifft.make_input_vec();
    let mut back = ifft.make_output_vec();
    for f in 0..frames {
        let at = f * HOP;
        for (k, out) in outs.iter_mut().enumerate() {
            for b in 0..bins {
                let (h, p) = (harmonic[f][b].powi(2), percussive[f][b].powi(2));
                let mask = if h + p > 1e-12 { if k == 0 { p / (h + p) } else { h / (h + p) } } else { 0.5 };
                spectrum[b] = spectra[f][b] * mask;
            }
            // the DC and Nyquist bins of a real signal have no imaginary part
            spectrum[0].im = 0.0;
            spectrum[bins - 1].im = 0.0;
            ifft.process(&mut spectrum, &mut back).expect("frame sizes match the plan");
            for (i, x) in back.iter().enumerate() {
                out[at + i] += x / FRAME as f32 * window[i];
            }
        }
        for (i, w) in window.iter().enumerate() {
            norm[at + i] += w * w;
        }
    }
    let [hits, tones] = outs.map(|o| o.into_iter().zip(&norm).map(|(x, w)| if *w > 1e-3 { x / w } else { 0.0 }).skip(FRAME / 2).take(n).collect::<Vec<f32>>());
    (hits, tones)
}

/// A note heard in a melody: start and length in seconds, MIDI key, velocity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeardNote {
    pub start: f32,
    pub length: f32,
    pub key: u8,
    pub velocity: u8,
}

/// YIN: the fundamental period of `x` (in samples) where the cumulative mean normalized
/// difference first dips under the threshold, refined by a parabola; none if unvoiced.
fn yin(x: &[f32], min_lag: usize, max_lag: usize) -> Option<f32> {
    let w = x.len() - max_lag;
    let mut d = vec![0.0f32; max_lag + 1];
    for (lag, dl) in d.iter_mut().enumerate().skip(1) {
        *dl = (0..w).map(|i| (x[i] - x[i + lag]).powi(2)).sum();
    }
    let mut running = 0.0;
    let mut cmnd = vec![1.0f32; max_lag + 1];
    for lag in 1..=max_lag {
        running += d[lag];
        cmnd[lag] = if running > 0.0 { d[lag] * lag as f32 / running } else { 1.0 };
    }
    let mut lag = min_lag;
    while lag < max_lag {
        if cmnd[lag] < 0.15 {
            while lag + 1 < max_lag && cmnd[lag + 1] < cmnd[lag] {
                lag += 1;
            }
            let (a, b, c) = (cmnd[lag - 1], cmnd[lag], cmnd[lag + 1]);
            let shift = if a - 2.0 * b + c != 0.0 { 0.5 * (a - c) / (a - 2.0 * b + c) } else { 0.0 };
            return Some(lag as f32 + shift.clamp(-1.0, 1.0));
        }
        lag += 1;
    }
    None
}

/// The notes of one voice in `mono` (40 Hz to 1 kHz): pitch every 5 ms, a note wherever the
/// same key holds for at least 60 ms above the noise.
pub fn melody_notes(mono: &[f32], rate: f32) -> Vec<HeardNote> {
    let (min_lag, max_lag) = ((rate / 1000.0) as usize, (rate / 40.0) as usize);
    let window = max_lag * 2;
    let hop = ((rate * 0.005) as usize).max(1);
    if mono.len() < window + max_lag {
        return Vec::new();
    }
    let peak = mono.iter().fold(0.0f32, |m, s| m.max(s.abs())).max(1e-6);
    // per hop: (key, level) or none
    let track: Vec<Option<(u8, f32)>> = (0..(mono.len() - window - max_lag) / hop)
        .map(|f| {
            let x = &mono[f * hop..f * hop + window + max_lag];
            let level = (x[..window].iter().map(|s| s * s).sum::<f32>() / window as f32).sqrt();
            if level < peak * 0.05 {
                return None;
            }
            let lag = yin(x, min_lag, max_lag)?;
            let key = 69.0 + 12.0 * (rate / lag / 440.0).log2();
            (0.0..128.0).contains(&key).then(|| (key.round() as u8, level))
        })
        .collect();
    let mut notes = Vec::new();
    let mut i = 0;
    while i < track.len() {
        let Some((key, _)) = track[i] else {
            i += 1;
            continue;
        };
        let start = i;
        let mut loudest = 0.0f32;
        // a note lasts while the key holds (a one-hop wobble doesn't end it)
        while i < track.len() && (track[i].is_some_and(|(k, _)| k == key) || (i + 1 < track.len() && track[i + 1].is_some_and(|(k, _)| k == key) && track[i].is_some())) {
            loudest = loudest.max(track[i].map_or(0.0, |(_, l)| l));
            i += 1;
        }
        let length = (i - start) as f32 * hop as f32 / rate;
        if length >= 0.06 {
            let velocity = (40.0 + 87.0 * (loudest / peak / 0.7).min(1.0)) as u8;
            notes.push(HeardNote { start: start as f32 * hop as f32 / rate, length, key, velocity });
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 22_050.0;

    fn energy(x: &[f32]) -> f32 {
        x.iter().map(|s| s * s).sum()
    }

    /// A steady chord with clicks on every beat: the clicks go to the hits, the chord to the tones.
    #[test]
    fn hits_and_tones_part_ways() {
        let n = (RATE * 3.0) as usize;
        let chord: Vec<f32> = (0..n).map(|i| [220.0f32, 277.0, 330.0].iter().map(|f| (2.0 * PI * f * i as f32 / RATE).sin() * 0.2).sum()).collect();
        let mut clicks = vec![0.0f32; n];
        for beat in 0..6 {
            let at = (beat as f32 * 0.5 * RATE) as usize + 1000;
            // a short noise burst (deterministic)
            let mut seed = 1u32 + beat;
            for k in 0..200 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                clicks[at + k] = (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * 0.8 * (1.0 - k as f32 / 200.0);
            }
        }
        let mix: Vec<f32> = chord.iter().zip(&clicks).map(|(a, b)| a + b).collect();
        let (hits, tones) = split_hpss(&mix);
        assert_eq!((hits.len(), tones.len()), (n, n));
        let sum: Vec<f32> = hits.iter().zip(&tones).map(|(a, b)| a + b).collect();
        let error = energy(&sum.iter().zip(&mix).map(|(a, b)| a - b).collect::<Vec<_>>()[FRAME..n - FRAME]) / energy(&mix[FRAME..n - FRAME]);
        assert!(error < 0.01, "the parts add back up: {error}");
        // how much of each source landed where it belongs
        let tones_in_tones = energy(&tones.iter().zip(&chord).map(|(t, c)| t * c).collect::<Vec<_>>()).max(0.0);
        let clicks_in_hits: f32 = hits.iter().zip(&clicks).map(|(h, c)| h * c).sum();
        let clicks_in_tones: f32 = tones.iter().zip(&clicks).map(|(t, c)| t * c).sum();
        assert!(tones_in_tones > 0.0 && clicks_in_hits > 3.0 * clicks_in_tones.abs(), "hits {clicks_in_hits}, leaked {clicks_in_tones}");
    }

    /// A bassline of plucked saw notes comes back as its keys and timing.
    #[test]
    fn hears_a_bassline() {
        let line = [(0.0, 0.3, 45u8), (0.4, 0.3, 48), (0.8, 0.2, 52), (1.2, 0.5, 43)];
        let mut x = vec![0.0f32; (RATE * 2.0) as usize];
        for (start, length, key) in line {
            let f = 440.0 * ((key as f32 - 69.0) / 12.0).exp2();
            for i in 0..(length * RATE) as usize {
                let t = i as f32 / RATE;
                let saw = 2.0 * (f * t).fract() - 1.0;
                x[(start * RATE) as usize + i] += saw * 0.5 * (-t * 3.0).exp();
            }
        }
        let notes = melody_notes(&x, RATE);
        let keys: Vec<u8> = notes.iter().map(|n| n.key).collect();
        assert_eq!(keys, vec![45, 48, 52, 43], "{notes:?}");
        for (heard, (start, length, _)) in notes.iter().zip(line) {
            assert!((heard.start - start).abs() < 0.05, "{heard:?}");
            assert!((heard.length - length).abs() < 0.12, "{heard:?}");
        }
    }
}
