//! Following the song's tempo: a time-stretcher for playlist audio clips (FL Studio's
//! "stretch" mode) and a guess at the tempo a loop was made at.
//!
//! The stretcher is WSOLA, on the audio thread: grains of 40 ms, half overlapping under a
//! Hann window, each one started where its waveform best lines up with the grain before
//! (within 10 ms of where the tempo puts it). The beat moves with the tempo, the pitch
//! doesn't. Nothing allocates after [`Stretcher::new`].

use std::f32::consts::PI;
use std::sync::Arc;

use crate::Sample;
use crate::sample::{read, taps};

/// The longest grain: 40 ms at 192 kHz.
const MAX_GRAIN: usize = 7680;
const FADE_SAMPLES: f32 = 96.0;

pub struct Stretcher {
    sample: Option<Arc<Sample>>,
    /// Overlap-add: `acc[read..hop]` is done, the rest is the last grain's tail; `side` the
    /// same for a stereo sound's sides.
    acc: Vec<f32>,
    side: Vec<f32>,
    grain: usize,
    hop: usize,
    read: usize,
    /// Where the next grain belongs by the tempo, and where the last one started (input frames).
    next: f64,
    last: f64,
    /// Input frames per output sample: through the sound (the tempo), and inside a grain (the pitch).
    advance: f64,
    step: f64,
    /// Sample rate over output rate.
    ratio: f64,
    first: bool,
    ending: bool,
    /// Goes round end to start instead of ending (a looped clip).
    looped: bool,
    gain: f32,
    /// Per-sample gain step while fading out (0 = not fading).
    fade: f32,
}

impl Default for Stretcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Stretcher {
    pub fn new() -> Self {
        Self {
            sample: None,
            acc: vec![0.0; MAX_GRAIN],
            side: vec![0.0; MAX_GRAIN],
            grain: 0,
            hop: 0,
            read: 0,
            next: 0.0,
            last: 0.0,
            advance: 1.0,
            step: 1.0,
            ratio: 1.0,
            first: true,
            ending: false,
            looped: false,
            gain: 0.0,
            fade: 0.0,
        }
    }

    /// Plays `sample` from `start_frame`, `speed` seconds of it per second (the tempo) and
    /// `semitones` up, round and round when `looped`. The caller keeps another `Arc` to the
    /// sample, so dropping it here never frees memory on the audio thread.
    #[allow(clippy::too_many_arguments)]
    pub fn trigger(&mut self, sample: Arc<Sample>, gain: f32, out_rate: f32, start_frame: f64, speed: f64, semitones: f32, looped: bool) {
        self.grain = ((0.04 * out_rate) as usize / 2 * 2).clamp(64, MAX_GRAIN);
        self.hop = self.grain / 2;
        self.ratio = (sample.rate / out_rate) as f64;
        self.step = self.ratio * (semitones as f64 / 12.0).exp2();
        self.advance = self.ratio * speed;
        self.acc[..self.grain].fill(0.0);
        self.side[..self.grain].fill(0.0);
        (self.read, self.next, self.last) = (self.hop, start_frame.max(0.0), start_frame.max(0.0));
        (self.first, self.ending, self.gain, self.fade, self.looped) = (true, false, gain, 0.0, looped && sample.frames.len() > 1);
        if self.looped {
            let len = sample.frames.len() as f64;
            (self.next, self.last) = (self.next % len, self.last % len);
        }
        self.sample = Some(sample);
    }

    /// A new tempo while it plays.
    pub fn set_speed(&mut self, speed: f64) {
        self.advance = self.ratio * speed;
    }

    pub fn is_playing(&self) -> bool {
        self.sample.is_some()
    }

    pub fn choke(&mut self) {
        if self.sample.is_some() && self.fade == 0.0 {
            self.fade = self.gain / FADE_SAMPLES;
        }
    }

    /// Adds the next `l.len()` samples to `l` and `r`.
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            if self.read == self.hop {
                self.next_grain();
            }
            if self.sample.is_none() {
                return;
            }
            let (m, s) = (self.acc[self.read], self.side[self.read]);
            *a += (m + s) * self.gain;
            *b += (m - s) * self.gain;
            self.read += 1;
            if self.fade > 0.0 {
                self.gain -= self.fade;
                if self.gain <= 0.0 {
                    self.sample = None;
                    return;
                }
            }
        }
    }

    fn next_grain(&mut self) {
        let Some(sample) = &self.sample else { return };
        let (x, sides) = (&sample.frames[..], &sample.side[..]);
        let (n, hop) = (self.grain, self.hop);
        if !self.first {
            self.acc.copy_within(hop..n, 0);
            self.acc[n - hop..n].fill(0.0);
            self.side.copy_within(hop..n, 0);
            self.side[n - hop..n].fill(0.0);
        }
        self.read = 0;
        // looped: round to the start; reads past the end come from there too
        let (looped, len) = (self.looped, x.len() as f64);
        if looped && self.next >= len {
            (self.next, self.last) = (self.next - len, self.last - len);
        }
        let wrap = |pos: f64| if looped && pos >= len { pos % len } else { pos };
        if self.next >= x.len() as f64 {
            // past the end: the last tail plays out, then it's over
            if std::mem::replace(&mut self.ending, true) {
                self.sample = None;
            }
            return;
        }
        let step = self.step;
        let at = |pos: f64| x.get(wrap(pos) as usize).copied().unwrap_or(0.0);
        let start = if self.first {
            self.next
        } else {
            // the last grain carried on: what this one should sound like where they overlap
            let target = self.last + hop as f64 * step;
            let reach = (0.010 * sample.rate) as i64;
            // every 4th frame of it, read once
            let mut want = [0.0f32; MAX_GRAIN / 8];
            let probes = hop.div_ceil(4);
            for (k, w) in want[..probes].iter_mut().enumerate() {
                *w = at(target + (4 * k) as f64 * step);
            }
            let fit = |d: i64| {
                let from = self.next + d as f64;
                let (mut dot, mut power) = (0.0f32, 1e-9f32);
                for (k, w) in want[..probes].iter().enumerate() {
                    let a = at(from + (4 * k) as f64 * step);
                    dot += a * w;
                    power += a * a;
                }
                dot / power.sqrt()
            };
            // nearest first, so silence (all equal) stays on the beat
            let best = |lags: &mut dyn Iterator<Item = i64>| {
                lags.filter(|d| self.next + (*d as f64) >= 0.0).fold((0, f32::MIN), |best, d| {
                    let f = fit(d);
                    if f > best.1 { (d, f) } else { best }
                })
            };
            // coarse, then to the frame around the best: a fine search over all 10 ms took
            // most of a busy project's time, all of it in the blocks where grains start
            let (coarse, _) = best(&mut (0..=reach / 8).flat_map(|k| [8 * k, -8 * k]));
            let (fine, _) = best(&mut (0..8).flat_map(|k| [coarse + k, coarse - k]));
            self.next + fine as f64
        };
        // the grain lands where the mix lines up best; a stereo sound's sides follow it there
        let last = x.len() as isize - 1;
        for k in 0..n {
            // the first grain keeps its attack: no fade in
            let w = if self.first && k < hop { 1.0 } else { (PI * k as f32 / n as f32).sin().powi(2) };
            let pos = wrap(start + k as f64 * step);
            let i = pos as usize;
            if i as isize > last {
                continue;
            }
            let tap = taps((pos - i as f64) as f32, |d| (i as isize + d).clamp(0, last) as usize);
            self.acc[k] += w * read(x, &tap);
            if !sides.is_empty() {
                self.side[k] += w * read(sides, &tap);
            }
        }
        self.last = start;
        self.next += hop as f64 * self.advance;
        self.first = false;
    }
}

/// The tempo a loop was made at. Its name says so ("…_124bpm…", "90 BPM …"), or a loop is
/// a whole number of bars long and its hits repeat on the beat. None for one-shots and
/// sounds without a steady beat.
pub fn detect_bpm(sample: &Sample, name: &str) -> Option<f32> {
    if let Some(bpm) = bpm_in_name(name) {
        return Some(bpm);
    }
    let seconds = sample.seconds() as f64;
    if seconds < 1.0 {
        return None;
    }
    // how much louder it got, every millisecond over the last 10: the hits. Fine steps keep
    // the beat's length exact; the first 30 s say enough
    let hop = (sample.rate / 1000.0).max(1.0) as usize;
    let power: Vec<f32> = sample.frames[..sample.frames.len().min((30.0 * sample.rate) as usize)].chunks(hop).map(|c| c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).collect();
    let level: Vec<f32> = (0..power.len()).map(|i| (1.0 + 100.0 * (power[i.saturating_sub(9)..=i].iter().sum::<f32>() / 10.0).sqrt()).ln()).collect();
    let mut hits: Vec<f32> = level.windows(2).map(|w| (w[1] - w[0]).max(0.0)).collect();
    let mean = hits.iter().sum::<f32>() / hits.len().max(1) as f32;
    hits.iter_mut().for_each(|h| *h -= mean);
    let frame = hop as f64 / sample.rate as f64;
    let echo = |lag: f64| {
        let (i, f) = (lag as usize, (lag.fract()) as f32);
        let at = |l: usize| {
            if l == 0 || l >= hits.len() {
                return 0.0;
            }
            hits.iter().zip(&hits[l..]).map(|(a, b)| a * b).sum::<f32>() / (hits.len() - l) as f32
        };
        at(i) * (1.0 - f) + at(i + 1) * f
    };
    // a beat that echoes, and so do its two and four beats; tempos near 120 are likelier
    // than half or double that (a beat echoes at two beats too)
    let score = |bpm: f64| {
        let beat = 60.0 / bpm / frame;
        let likely = (-0.5 * (bpm / 120.0).log2().powi(2)).exp() as f32;
        likely * (echo(beat) + 0.5 * echo(2.0 * beat) + 0.25 * echo(4.0 * beat))
    };
    const RANGE: (f64, f64) = (70.0, 180.0);
    let best = |candidates: &mut dyn Iterator<Item = f64>| candidates.map(|b| (b, score(b))).max_by(|a, b| a.1.total_cmp(&b.1));
    // then a hair either way (a tail, a trimmed start)
    let refine = |(guess, _): (f64, f32)| best(&mut (-20..=20).map(|k| guess * (1.0 + k as f64 * 0.001)));
    let heard = refine(best(&mut (0..((RANGE.1 - RANGE.0) * 2.0) as usize).map(|k| RANGE.0 + k as f64 * 0.5))?)?;
    // a loop is 1, 2, 4… bars long: that wins, unless the beat clearly says otherwise
    let loops = (0..8).map(|k| 4.0 * 2f64.powi(k) * 60.0 / seconds).filter(|b| (RANGE.0..RANGE.1).contains(b));
    let (bpm, strength) = match best(&mut loops.into_iter()).filter(|_| seconds <= 32.0).and_then(refine) {
        Some(looped) if looped.1 * 1.3 >= heard.1 => looped,
        _ => heard,
    };
    if strength <= 0.0 {
        return None;
    }
    // whole numbers when close
    let round = bpm.round();
    Some(if (bpm - round).abs() < 0.3 { round } else { (bpm * 10.0).round() / 10.0 } as f32)
}

/// "loop_124bpm", "Drums 90 BPM", "bpm-140": a number next to "bpm".
fn bpm_in_name(name: &str) -> Option<f32> {
    let lower = name.to_lowercase();
    let bytes = lower.as_bytes();
    let number = |range: std::ops::Range<usize>| lower.get(range).and_then(|t| t.parse::<f32>().ok()).filter(|b| (40.0..=300.0).contains(b));
    let mut from = 0;
    while let Some(i) = lower[from..].find("bpm").map(|i| i + from) {
        // before: digits, maybe a space, _ or -
        let mut end = i;
        while end > 0 && b" _-".contains(&bytes[end - 1]) {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b'.') {
            start -= 1;
        }
        if let Some(b) = number(start..end) {
            return Some(b);
        }
        // after
        let mut start = i + 3;
        while start < bytes.len() && b" _-".contains(&bytes[start]) {
            start += 1;
        }
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'.') {
            end += 1;
        }
        if let Some(b) = number(start..end) {
            return Some(b);
        }
        from = i + 3;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A beat: a short click on every beat for `bars` bars.
    fn clicks(bpm: f32, bars: usize, rate: f32) -> Sample {
        let beat = (60.0 / bpm * rate) as usize;
        let mut frames = vec![0.0; beat * 4 * bars];
        for (k, start) in (0..4 * bars).map(|b| b * beat).enumerate() {
            let loud = if k % 4 == 0 { 1.0 } else { 0.6 };
            for i in 0..400 {
                frames[start + i] = loud * (i as f32 * 0.3).sin() * (1.0 - i as f32 / 400.0);
            }
        }
        Sample::new(frames, rate)
    }

    #[test]
    fn finds_the_tempo_from_the_name_or_the_beat() {
        let quiet = Sample::new(vec![0.0; 48_000], 48_000.0);
        assert_eq!(detect_bpm(&quiet, "Funky Drums_124bpm.wav"), Some(124.0));
        assert_eq!(detect_bpm(&quiet, "Pad 90 BPM C minor.wav"), Some(90.0));
        assert_eq!(detect_bpm(&quiet, "bpm-140 loop.wav"), Some(140.0));
        assert_eq!(detect_bpm(&Sample::new(vec![0.0; 4_800], 48_000.0), "kick.wav"), None, "a one-shot has no tempo");
        for bpm in [92.0, 120.0, 128.0, 174.0] {
            assert_eq!(detect_bpm(&clicks(bpm, 2, 44_100.0), "loop.wav"), Some(bpm), "{bpm}");
        }
        // 4 bars at 140 and a tail: not quite a whole number of bars long
        let mut tail = clicks(140.0, 4, 48_000.0);
        tail.frames.extend(std::iter::repeat_n(0.0, 4_000));
        assert_eq!(detect_bpm(&tail, "loop"), Some(140.0));
    }

    /// A drum loop: a kick on the beat, a snare on 2 and 4, noisy hats on the off-beats.
    fn drums(bpm: f32, bars: usize, rate: f32) -> Sample {
        let beat = (60.0 / bpm * rate) as usize;
        let mut frames = vec![0.0f32; beat * 4 * bars];
        let mut seed = 1u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        };
        for b in 0..4 * bars {
            let at = b * beat;
            for i in 0..(0.25 * rate) as usize {
                let t = i as f32 / rate;
                frames[at + i] += (std::f32::consts::TAU * (50.0 + 120.0 * (-t * 30.0).exp()) * t).sin() * (-t * 12.0).exp();
                if b % 2 == 1 {
                    frames[at + i] += 0.5 * noise() * (-t * 25.0).exp();
                }
            }
            for i in 0..(0.05 * rate) as usize {
                frames[at + beat / 2 + i] += 0.2 * noise() * (-(i as f32) / rate * 80.0).exp();
            }
        }
        Sample::new(frames, rate)
    }

    #[test]
    fn finds_the_tempo_of_a_drum_loop() {
        for (bpm, bars) in [(97.0, 4), (140.0, 1), (126.0, 2), (85.0, 2)] {
            assert_eq!(detect_bpm(&drums(bpm, bars, 44_100.0), "drum loop.wav"), Some(bpm), "{bpm} BPM, {bars} bars");
        }
        // not a loop: as long as it happens to be
        let mut take = drums(126.0, 4, 44_100.0);
        take.frames.truncate(take.frames.len() * 5 / 7);
        assert_eq!(detect_bpm(&take, "drums take 3.wav"), Some(126.0));
    }

    #[test]
    fn stretching_keeps_the_pitch_and_follows_the_tempo() {
        let rate = 48_000.0;
        // one second of 1 kHz
        let sine = Arc::new(Sample::new((0..48_000).map(|i| (i as f32 * std::f32::consts::TAU * 1000.0 / rate).sin() * 0.5).collect(), rate));
        let run = |speed: f64, semitones: f32| {
            let mut s = Stretcher::new();
            s.trigger(sine.clone(), 1.0, rate, 0.0, speed, semitones, false);
            let (mut out, mut right) = (vec![0.0; 96_000], vec![0.0; 96_000]);
            s.render(&mut out, &mut right);
            let long = out.iter().rposition(|x| x.abs() > 0.05).unwrap_or(0) as f32 / rate;
            // pitch: zero crossings in the steady middle
            let mid = &out[2_000..(long * rate) as usize - 2_000];
            let crossings = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32 / (mid.len() as f32 / rate);
            (long, crossings)
        };
        let (long, hz) = run(140.0 / 120.0, 0.0);
        assert!((long - 120.0 / 140.0).abs() < 0.03, "a 120 loop at 140 is shorter: {long} s");
        assert!((hz - 1000.0).abs() < 20.0, "and keeps its pitch: {hz} Hz");
        let (long, hz) = run(0.75, 12.0);
        assert!((long - 1.0 / 0.75).abs() < 0.03, "slower: {long} s");
        assert!((hz - 2000.0).abs() < 40.0, "an octave up: {hz} Hz");
    }

    /// A sound only on the left stays on the left, stretched.
    #[test]
    fn stretching_keeps_the_sides() {
        let rate = 48_000.0;
        let left: Vec<f32> = (0..48_000).map(|i| (i as f32 * std::f32::consts::TAU * 440.0 / rate).sin() * 0.5).collect();
        let half: Vec<f32> = left.iter().map(|x| x / 2.0).collect();
        let mut s = Stretcher::new();
        s.trigger(Arc::new(Sample::stereo(half.clone(), half, rate)), 1.0, rate, 0.0, 1.2, 0.0, false);
        let (mut l, mut r) = (vec![0.0; 48_000], vec![0.0; 48_000]);
        s.render(&mut l, &mut r);
        let peak = |x: &[f32]| x[4_000..30_000].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak(&l) > 0.4 && peak(&r) < 1e-4, "left {}, right {}", peak(&l), peak(&r));
    }
}
