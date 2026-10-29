//! Offline render tests: the same code path the audio thread runs, without a device.

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use ravune_core::{
    ClipFx, EnvTarget, Envelope, PatternAuto, Stretch,
    BAR, Channel, Clip, ClipKind, Device, DeviceKind, Instrument, Lfo, Mapping, MixerTrack, Note, Pattern, Project, STEP, Send, Source,
    Target, default_synth,
};
use ravune_engine::{Effects, Garbage, Mode, Msg, PluginTarget, Renderer, Sample, Song, Transport, render_offline};

// Any allocation inside `assert_no_alloc` aborts the test binary.
#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

const SR: f32 = 48_000.0;
const SIXTEENTH: usize = 6_000; // samples per 16th at 120 bpm, 48 kHz
const BAR_SAMPLES: usize = 16 * SIXTEENTH;

fn render_stereo(r: &mut Renderer, seconds: f32) -> (Vec<f32>, Vec<f32>) {
    let n = (SR * seconds) as usize;
    let (mut l, mut rt) = (vec![0.0; n], vec![0.0; n]);
    assert_no_alloc(|| r.render(&mut l, &mut rt));
    (l, rt)
}

/// The left channel: everything here is panned centre unless a test says otherwise.
fn render(r: &mut Renderer, seconds: f32) -> Vec<f32> {
    render_stereo(r, seconds).0
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

fn onset(x: &[f32], from: usize) -> usize {
    (from..x.len()).find(|&i| x[i].abs() > 0.1).expect("no onset")
}

/// A 100 ms full-scale square burst: easy to spot in the output.
fn burst() -> Arc<Sample> {
    Arc::new(Sample::new((0..4_800).map(|i| if i % 20 < 10 { 0.9 } else { -0.9 }).collect(), SR))
}

fn sampler(slot: u8) -> Channel {
    let mut c = Channel::new(slot, "s", Instrument::Sampler { sample: "burst".into() });
    c.gain = 1.0;
    c
}

/// One sampler channel (slot 0) whose pattern 0 has `steps` on, and a bare master (the
/// starter's limiter would delay what these tests time to the sample).
fn beat(steps: &[usize]) -> Project {
    let mut p = Project { channels: vec![sampler(0)], patterns: vec![Pattern::default()], ..Project::default() };
    p.mixer[0].devices.clear();
    steps.iter().for_each(|&s| p.patterns[0].set_step(0, s, 127));
    p
}

const PAT: Transport = Transport { playing: true, mode: Mode::Pattern(0), start: 0, jump: 0, looped: None };
const SONG: Transport = Transport { playing: true, mode: Mode::Song, start: 0, jump: 0, looped: None };

/// Sends a project to the renderer; returns the old song so it's dropped outside `assert_no_alloc`.
fn load(r: &mut Renderer, project: &Project, transport: Transport, sample: &Arc<Sample>) -> Option<Garbage> {
    let song = Arc::new(Song::compile(project, transport, |_| Some(sample.clone())));
    assert_no_alloc(|| r.handle(Msg::Song(song)))
}

#[test]
fn silent_when_idle() {
    let mut r = Renderer::new(SR);
    assert_eq!(peak(&render(&mut r, 0.5)), 0.0);
}

#[test]
fn keys_play_the_target_synth_then_release() {
    let mut r = Renderer::new(SR);
    let synth = Channel::new(7, "va", Instrument::Synth { params: default_synth() });
    let project = Project { channels: vec![synth], ..Project::default() };
    let stopped = Transport { playing: false, ..PAT };
    let _old = load(&mut r, &project, stopped, &burst());
    assert_no_alloc(|| {
        r.handle(Msg::Target(Some(7)));
        r.handle(Msg::NoteOn { note: 60, velocity: 1.0 });
    });
    assert!(peak(&render(&mut r, 0.3)) > 0.05, "note should be audible");
    assert_ne!(r.held_notes() & 1 << 60, 0);
    r.handle(Msg::NoteOff { note: 60 });
    let tail = render(&mut r, 2.0);
    assert!(peak(&tail[tail.len() - 4800..]) < 1e-3, "release should fade to silence");
}

#[test]
fn steps_fire_on_the_grid_and_loop() {
    let mut r = Renderer::new(SR);
    let _old = load(&mut r, &beat(&[0, 4]), PAT, &burst());
    let out = render(&mut r, 2.2);
    assert!(peak(&out[..200]) > 0.5, "step 1 fires immediately");
    assert!(peak(&out[5_000..4 * SIXTEENTH - 10]) < 1e-3, "silence until step 5");
    assert!(onset(&out, 4 * SIXTEENTH - 10).abs_diff(4 * SIXTEENTH) <= 1, "step 5 is sample-accurate");
    assert!(onset(&out, BAR_SAMPLES - 10).abs_diff(BAR_SAMPLES) <= 1, "the 1-bar pattern loops");
}

#[test]
fn swing_delays_odd_steps() {
    let mut r = Renderer::new(SR);
    let mut p = beat(&[1]);
    p.swing = 1.0;
    let _old = load(&mut r, &p, PAT, &burst());
    let out = render(&mut r, 0.3);
    let expected = SIXTEENTH + SIXTEENTH / 3; // full swing = triplet feel
    assert!(onset(&out, 0).abs_diff(expected) <= 1);
}

#[test]
fn a_channel_swings_on_its_own() {
    for (song, own, expected) in [(1.0, Some(0.0), SIXTEENTH), (0.0, Some(1.0), SIXTEENTH + SIXTEENTH / 3)] {
        let mut r = Renderer::new(SR);
        let mut p = beat(&[1]);
        (p.swing, p.channels[0].swing) = (song, own);
        let _old = load(&mut r, &p, PAT, &burst());
        assert!(onset(&render(&mut r, 0.3), 0).abs_diff(expected) <= 1, "song {song}, own {own:?}");
    }
}

/// Attack fades each hit in; a release holds it for the note and fades it after.
#[test]
fn sampler_envelope() {
    let long = Arc::new(Sample::new((0..SR as usize).map(|i| if i % 20 < 10 { 0.9 } else { -0.9 }).collect(), SR));
    let mut p = beat(&[0]);
    (p.channels[0].attack, p.channels[0].release) = (0.05, 0.01);
    let mut r = Renderer::new(SR);
    let _old = load(&mut r, &p, PAT, &long);
    let out = render(&mut r, 0.3);
    assert!(peak(&out[..240]) < 0.1, "fading in");
    assert!(peak(&out[2_400..2_600]) > 0.8, "in after 50 ms");
    assert!(peak(&out[SIXTEENTH + 600..SIXTEENTH + 5_000]) < 1e-3, "gone 10 ms after the 16th ends");
    p.channels[0].release = 0.0;
    let mut r = Renderer::new(SR);
    let _old = load(&mut r, &p, PAT, &long);
    assert!(peak(&render(&mut r, 0.3)[SIXTEENTH + 600..SIXTEENTH + 5_000]) > 0.8, "no release: the whole sound");
}

/// A sampler channel plays only the part of its sound between its start and end.
#[test]
fn sampler_start_and_end_cut_the_sound() {
    // a second of sound: silent in its first half, loud in its second
    let half = SR as usize / 2;
    let halves = Arc::new(Sample::new((0..SR as usize).map(|i| if i < half { 0.0 } else if i % 20 < 10 { 0.9 } else { -0.9 }).collect(), SR));
    let mut p = beat(&[0]);
    p.channels[0].start = 0.5;
    let mut r = Renderer::new(SR);
    let _old = load(&mut r, &p, PAT, &halves);
    assert!(peak(&render(&mut r, 0.1)[..200]) > 0.8, "starts where the loud half begins");
    // from 1/2 to 3/4: a quarter of a second, then nothing (no click: it fades out)
    p.channels[0].end = 0.75;
    let mut r = Renderer::new(SR);
    let _old = load(&mut r, &p, PAT, &halves);
    let out = render(&mut r, 0.4);
    let quarter = SR as usize / 4;
    assert!(peak(&out[quarter - 1_000..quarter - 500]) > 0.8, "plays up to the end");
    assert!(peak(&out[quarter - 20..quarter]) < 0.5, "fading as it gets there");
    assert!(peak(&out[quarter + 10..quarter + 6_000]) < 1e-3, "and stops");
}

/// Looped, a sampler channel goes round its start..end while the note is held, then stops.
#[test]
fn a_looped_sampler_repeats_its_part_until_the_note_ends() {
    // loud only in 0.50..0.55 of a second: 2 400 samples, shorter than a 16th (6 000)
    let half = SR as usize / 2;
    let blip = Arc::new(Sample::new((0..SR as usize).map(|i| if (half..half + 2_400).contains(&i) { 0.9 } else { 0.0 }).collect(), SR));
    let mut p = beat(&[0]);
    (p.channels[0].start, p.channels[0].end) = (0.5, 0.55);
    let mut r = Renderer::new(SR);
    let _old = load(&mut r, &p, PAT, &blip);
    assert!(peak(&render(&mut r, 0.2)[3_000..5_500]) < 1e-3, "once: over after its 2 400 samples");
    p.channels[0].looped = true;
    let mut r = Renderer::new(SR);
    let _old = load(&mut r, &p, PAT, &blip);
    let out = render(&mut r, 0.3);
    assert!(peak(&out[3_000..5_500]) > 0.8, "looped: round again while the 16th lasts");
    assert!(peak(&out[SIXTEENTH + 200..2 * SIXTEENTH]) < 1e-3, "and still at the note's end");
}

/// A looped audio clip repeats its sound under the clip; not looped, it plays once.
#[test]
fn a_looped_clip_repeats_its_sound() {
    for looped in [false, true] {
        let mut r = Renderer::new(SR);
        let mut p = beat(&[]);
        let fx = ClipFx { looped, ..Default::default() };
        // a 0.1 s sound in a clip of a beat (0.5 s)
        p.clips = vec![Clip { lane: 0, start: 0, length: BAR / 4, offset: 0, fx, kind: ClipKind::Audio { sample: "burst".into() } }];
        let song = Arc::new(Song::compile(&p, SONG, |_| Some(burst())));
        let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
        let out = render(&mut r, 0.8);
        assert_eq!(peak(&out[14_400..19_000]) > 0.3, looped, "looped: {looped}, sound at 0.3 s");
        assert!(peak(&out[24_200..]) < 1e-3, "nothing after the clip");
    }
}

/// A stereo file keeps its sides through the sampler: here the sound is only on the left.
#[test]
fn stereo_samples_keep_their_sides() {
    let path = std::env::temp_dir().join("ravune-left-only.wav");
    let left_only: Vec<f32> = (0..9_600).flat_map(|i| [if i % 20 < 10 { 0.8 } else { -0.8 }, 0.0]).collect();
    ravune_engine::write_wav(&path, &left_only, 2, SR as u32).unwrap();
    let sound = Arc::new(Sample::decode_file(&path).unwrap());
    assert_eq!(sound.side.len(), sound.frames.len(), "stereo");
    let mut r = Renderer::new(SR);
    let _old = load(&mut r, &beat(&[0]), PAT, &sound);
    let (l, rt) = render_stereo(&mut r, 0.1);
    assert!(peak(&l) > 0.7 && peak(&rt) < 1e-3, "left {}, right {}", peak(&l), peak(&rt));
    let mono: Vec<f32> = left_only.iter().step_by(2).copied().collect();
    ravune_engine::write_wav(&path, &mono, 1, SR as u32).unwrap();
    assert!(Sample::decode_file(&path).unwrap().side.is_empty(), "mono stays mono");
}

#[test]
fn piano_roll_notes_play_and_stop() {
    let mut r = Renderer::new(SR);
    let mut p = Project { channels: vec![Channel::new(2, "va", Instrument::Synth { params: default_synth() })], ..Project::default() };
    p.channels[0].gain = 1.0;
    p.patterns[0].notes_mut(2).push(Note { start: 2 * STEP, length: STEP, key: 64, velocity: 127, ..Default::default() });
    let _old = load(&mut r, &p, PAT, &burst());
    let out = render(&mut r, 1.0);
    assert!(peak(&out[..2 * SIXTEENTH - 10]) < 1e-3, "nothing before the note");
    let start = onset(&out, 0);
    // the onset threshold is reached after the synth's 5 ms attack and the filter
    assert!((2 * SIXTEENTH..2 * SIXTEENTH + 600).contains(&start), "note starts on step 3, got {start}");
    assert!(peak(&out[3 * SIXTEENTH + 24_000..]) < 1e-2, "note off after one step, then release");
}

#[test]
fn choke_group_cuts_the_other_channel() {
    let mut r = Renderer::new(SR);
    let long = Arc::new(Sample::new(vec![0.5; 48_000], SR)); // "open hat": 1 s of DC
    let quiet = Arc::new(Sample::new(vec![0.0; 10], SR)); // "closed hat": silent, only chokes
    let mut p = beat(&[0]);
    let mut closed = sampler(1);
    closed.instrument = Instrument::Sampler { sample: "quiet".into() };
    p.channels.push(closed);
    (p.channels[0].choke, p.channels[1].choke) = (1, 1);
    p.patterns[0].set_step(1, 2, 127);
    let song = Arc::new(Song::compile(&p, PAT, |path| Some(if path == "quiet" { quiet.clone() } else { long.clone() })));
    let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
    let out = render(&mut r, 0.5);
    assert!(peak(&out[..2 * SIXTEENTH]) > 0.3, "open hat rings until step 3");
    assert!(peak(&out[2 * SIXTEENTH + 200..]) < 1e-3, "then the closed hat chokes it");
}

#[test]
fn song_mode_plays_clips_where_they_sit() {
    let mut r = Renderer::new(SR);
    let mut p = beat(&[0]);
    let clip = |start: u32, length: u32| Clip { lane: 0, start, length, offset: 0, fx: Default::default(), kind: ClipKind::Pattern { pattern: 0 } };
    p.clips = vec![clip(0, BAR), clip(2 * BAR, 2 * BAR)]; // gap in bar 2; second clip loops the 1-bar pattern
    let _old = load(&mut r, &p, SONG, &burst());
    let out = render(&mut r, 8.5);
    assert!(peak(&out[..200]) > 0.5, "bar 1");
    assert!(peak(&out[BAR_SAMPLES - 100..2 * BAR_SAMPLES - 10]) < 1e-3, "bar 2 is empty");
    for (from, expected, what) in [
        (2 * BAR_SAMPLES - 10, 2 * BAR_SAMPLES, "bar 3"),
        (3 * BAR_SAMPLES - 10, 3 * BAR_SAMPLES, "clip loops its pattern in bar 4"),
        (3 * BAR_SAMPLES + 5_000, 4 * BAR_SAMPLES, "song (4 bars) loops to the top"),
    ] {
        let o = onset(&out, from);
        assert!(o.abs_diff(expected) <= 1, "{what}: expected {expected}, got {o}");
    }
}

#[test]
fn audio_clips_start_trimmed_and_stop() {
    let mut r = Renderer::new(SR);
    let ramp = Arc::new(Sample::new((0..96_000).map(|i| 0.2 + 0.6 * i as f32 / 96_000.0).collect(), SR)); // 2 s
    let mut p = beat(&[]);
    p.clips = vec![Clip { lane: 1, start: BAR, length: BAR / 2, offset: BAR / 4, fx: Default::default(), kind: ClipKind::Audio { sample: "ramp".into() } }];
    let song = Arc::new(Song::compile(&p, SONG, |_| Some(ramp.clone())));
    let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
    let out = render(&mut r, 3.5);
    assert!(peak(&out[..BAR_SAMPLES - 10]) < 1e-3, "silent before the clip");
    let first = out[BAR_SAMPLES + 2];
    let expected = (0.2 + 0.6 * (BAR_SAMPLES / 4) as f32 / 96_000.0) * 0.8;
    assert!((first - expected.tanh()).abs() < 0.02, "starts a beat into the sample: {first} vs {expected}");
    assert!(peak(&out[BAR_SAMPLES + BAR_SAMPLES / 2 + 200..]) < 1e-3, "stops at the clip end");
}

#[test]
fn starting_mid_clip_chases_audio() {
    let mut r = Renderer::new(SR);
    let dc = Arc::new(Sample::new(vec![0.5; 4 * BAR_SAMPLES], SR));
    let mut p = beat(&[]);
    p.clips = vec![Clip { lane: 0, start: 0, length: 4 * BAR, offset: 0, fx: Default::default(), kind: ClipKind::Audio { sample: "dc".into() } }];
    let from_bar_2 = Transport { start: BAR, ..SONG };
    let song = Arc::new(Song::compile(&p, from_bar_2, |_| Some(dc.clone())));
    let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
    assert!(peak(&render(&mut r, 0.1)) > 0.3, "audio already under the playhead starts right away");
}

#[test]
fn a_jump_moves_the_playing_song_at_once() {
    let mut r = Renderer::new(SR);
    let dc = Arc::new(Sample::new(vec![0.5; 4 * BAR_SAMPLES], SR));
    let mut p = beat(&[]);
    p.clips = vec![Clip { lane: 0, start: 2 * BAR, length: 2 * BAR, offset: 0, fx: Default::default(), kind: ClipKind::Audio { sample: "dc".into() } }];
    let play = |r: &mut Renderer, t: Transport| {
        let song = Arc::new(Song::compile(&p, t, |_| Some(dc.clone())));
        assert_no_alloc(|| r.handle(Msg::Song(song)))
    };
    let _old = play(&mut r, SONG);
    assert!(peak(&render(&mut r, 0.1)) < 1e-3, "bar 1 is empty");
    let _old = play(&mut r, Transport { start: 2 * BAR, ..SONG });
    assert!(peak(&render(&mut r, 0.1)) < 1e-3, "a new start alone waits for the next play");
    let _old = play(&mut r, Transport { start: 2 * BAR, jump: 1, ..SONG });
    assert!(peak(&render(&mut r, 0.1)) > 0.3, "a jump lands in the clip right away");
}

#[test]
fn audio_clips_play_with_their_gain_fades_pan_filter_and_mute() {
    let dc = Arc::new(Sample::new(vec![0.5; 4 * BAR_SAMPLES], SR));
    let with = |fx: ClipFx| {
        let mut p = beat(&[]);
        p.clips = vec![Clip { lane: 0, start: 0, length: 2 * BAR, offset: 0, fx, kind: ClipKind::Audio { sample: "dc".into() } }];
        let mut r = Renderer::new(SR);
        let song = Arc::new(Song::compile(&p, SONG, |_| Some(dc.clone())));
        let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
        render_stereo(&mut r, 0.5)
    };
    let (l, r) = with(ClipFx::default());
    let plain = peak(&l[4000..]);
    assert!((plain - 0.4).abs() < 0.02 && (peak(&r[4000..]) - plain).abs() < 1e-4, "0.5 at 0.8, centred: {plain}");
    let (l, _) = with(ClipFx { gain: -6.0206, ..Default::default() });
    assert!((peak(&l[4000..]) - plain / 2.0).abs() < 0.01, "-6 dB halves it");
    let (l, _) = with(ClipFx { fade_in: BAR, ..Default::default() });
    assert!(peak(&l[..1000]) < 0.01 && peak(&l[20_000..]) > 0.05, "fades in");
    let env = |target, value| Envelope { target, points: vec![(0, value)], curves: vec![], shown: true };
    let (l, r) = with(ClipFx { envelopes: vec![env(EnvTarget::Pan, 0.0)], ..Default::default() });
    assert!(peak(&r[4000..]) < 1e-4 && peak(&l[4000..]) > 0.5, "hard left");
    let (l, _) = with(ClipFx { envelopes: vec![env(EnvTarget::Volume, 0.5)], ..Default::default() });
    assert!((peak(&l[4000..]) - plain / 2.0).abs() < 0.01, "volume envelope at half");
    let (l, _) = with(ClipFx { envelopes: vec![env(EnvTarget::Filter, 0.9)], ..Default::default() });
    assert!(peak(&l[20_000..]) < 0.01, "a high-pass takes the DC away");
    let (l, _) = with(ClipFx { mute: true, ..Default::default() });
    assert_eq!(peak(&l), 0.0, "muted");
}

#[test]
fn an_audio_clip_plays_into_its_mixer_track() {
    let dc = Arc::new(Sample::new(vec![0.5; 4 * BAR_SAMPLES], SR));
    let level = |track: usize| {
        let mut p = beat(&[]);
        p.mixer[1].mute = true;
        let fx = ClipFx { track, ..Default::default() };
        p.clips = vec![Clip { lane: 0, start: 0, length: 2 * BAR, offset: 0, fx, kind: ClipKind::Audio { sample: "dc".into() } }];
        let mut r = Renderer::new(SR);
        let song = Arc::new(Song::compile(&p, SONG, |_| Some(dc.clone())));
        let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
        peak(&render(&mut r, 0.1)[2000..])
    };
    assert!(level(0) > 0.3, "the master by default");
    assert!(level(1) < 1e-4, "through track 1, which is muted");
    assert!(level(99) > 0.3, "a track that doesn't exist falls back to the master");
}

#[test]
fn the_output_volume_scales_everything_after_the_mixer() {
    let dc = Arc::new(Sample::new(vec![0.5; 4 * BAR_SAMPLES], SR));
    let mut p = beat(&[]);
    p.clips = vec![Clip { lane: 0, start: 0, length: 2 * BAR, offset: 0, fx: Default::default(), kind: ClipKind::Audio { sample: "dc".into() } }];
    let mut r = Renderer::new(SR);
    let song = Arc::new(Song::compile(&p, SONG, |_| Some(dc.clone())));
    let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
    let full = peak(&render(&mut r, 0.1)[2000..]);
    assert_no_alloc(|| r.handle(Msg::Output(0.5)));
    let half = peak(&render(&mut r, 0.1)[2000..]);
    assert!((half - full / 2.0).abs() < 0.01, "{full} → {half}");
}

#[test]
fn notes_carry_their_own_pitch_and_brightness() {
    // (high-frequency share, zero crossings) of a VA-1 note
    let play = |n: Note| {
        let mut p = beat(&[]);
        p.channels.push(ravune_core::Channel::new(1, "VA-1", ravune_core::Instrument::Synth { params: ravune_core::default_synth() }));
        p.patterns[0].notes_mut(1).push(n);
        let mut r = Renderer::new(SR);
        let _old = load(&mut r, &p, PAT, &burst());
        let out = render(&mut r, 0.2);
        let edge: f32 = out.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        let level: f32 = out.iter().map(|x| x * x).sum();
        (edge / level.max(1e-9), out.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count())
    };
    let base = Note { key: 57, length: 8 * STEP, ..Default::default() };
    let (dull, _) = play(Note { bright: 0.0, ..base });
    let (bright, _) = play(Note { bright: 1.0, ..base });
    assert!(bright > dull * 1.5, "brighter per note: {dull} → {bright}");
    let (_, plain) = play(base);
    let (_, up) = play(Note { fine: 2.0, ..base });
    assert!(up as f32 > plain as f32 * 1.08, "two semitones up: {plain} → {up} crossings");
}

#[test]
fn metronome_clicks_on_each_beat() {
    let mut r = Renderer::new(SR);
    let mut p = beat(&[]);
    p.metronome = true; // 120 bpm → a beat every 24 000 samples
    let _old = load(&mut r, &p, PAT, &burst());
    let out = render(&mut r, 2.1);
    assert!(peak(&out[12_000..23_900]) < 1e-3, "silence between beats");
    // every beat of the bar, then the next bar's first as the pattern loops
    for (n, at) in [0, 24_000, 48_000, 72_000, 96_000].into_iter().enumerate() {
        assert!(peak(&out[at..at + 2_000]) > 0.1, "click on beat {}", n + 1);
    }
    // the same in a sound card's 480-sample blocks, where beats land on a block's edge
    let mut r = Renderer::new(SR);
    let _old = load(&mut r, &p, PAT, &burst());
    let (mut out, mut l, mut rt) = (Vec::new(), vec![0.0; 480], vec![0.0; 480]);
    for _ in 0..(2.1 * SR) as usize / 480 {
        l.fill(0.0);
        rt.fill(0.0);
        assert_no_alloc(|| r.render(&mut l, &mut rt));
        out.extend_from_slice(&l);
    }
    for (n, at) in [0, 24_000, 48_000, 72_000, 96_000].into_iter().enumerate() {
        assert!(peak(&out[at..at + 2_000]) > 0.1, "in blocks: click on beat {}", n + 1);
    }
}

#[test]
fn offline_render_covers_the_song_and_its_tail() {
    let mut p = beat(&[0, 8]);
    p.clips = vec![Clip { lane: 0, start: 0, length: 2 * BAR, offset: 0, fx: Default::default(), kind: ClipKind::Pattern { pattern: 0 } }];
    let song = Song::compile(&p, SONG, |_| Some(burst()));
    let stereo = render_offline(song, SR, 1.0, |_| {});
    let out: Vec<f32> = stereo.chunks(2).map(|f| f[0]).collect();
    assert_eq!(stereo.len(), 2 * out.len(), "interleaved stereo");
    assert_eq!(out.len(), 2 * BAR_SAMPLES + SR as usize, "2 bars + 1 s tail");
    let hits = [0, 8 * SIXTEENTH, BAR_SAMPLES, BAR_SAMPLES + 8 * SIXTEENTH];
    for h in hits {
        assert!(peak(&out[h..h + 100]) > 0.5, "hit at {h}");
    }
    assert!(peak(&out[2 * BAR_SAMPLES + 5_000..]) < 1e-3, "no loop after the end");
}

#[test]
fn song_swaps_and_mashing_never_allocate() {
    let mut r = Renderer::new(SR);
    let synth = Channel::new(3, "va", Instrument::Synth { params: default_synth() });
    let sample = burst();
    let mut p = beat(&[0, 2]);
    p.channels.push(synth);
    let mut olds = Vec::new();
    for t in [Transport { playing: false, ..PAT }, PAT, SONG, Transport { playing: false, ..SONG }] {
        olds.push(load(&mut r, &p, t, &sample));
    }
    assert!(olds.iter().all(Option::is_some), "the renderer hands back every replaced song");
    assert_no_alloc(|| {
        r.handle(Msg::Target(Some(3)));
        for n in 30..90 {
            r.handle(Msg::NoteOn { note: n, velocity: 0.8 });
        }
        r.handle(Msg::Preview(Some(sample.clone())));
    });
    let out = render(&mut r, 0.2);
    assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    r.handle(Msg::AllNotesOff);
    assert_eq!(r.held_notes(), 0);
}

#[test]
fn wav_round_trip() {
    let dir = std::env::temp_dir().join("ravune-wav-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tone.wav");
    let tone: Vec<f32> = (0..4_410).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    ravune_engine::write_wav(&path, &tone, 1, 44_100).unwrap();
    let back = Sample::decode_file(&path).unwrap();
    assert_eq!((back.frames.len(), back.rate), (4_410, 44_100.0));
    assert!(back.frames.iter().zip(&tone).all(|(a, b)| (a - b).abs() < 1e-3));
    assert!(Sample::decode_file(&dir.join("missing.wav")).is_err());
}

/// Like [`load`], also creating the effect instances the song's devices need.
fn load_fx(r: &mut Renderer, fx: &mut Effects, project: &Project, sample: &Arc<Sample>) -> Vec<Garbage> {
    let song = Song::compile(project, PAT, |_| Some(sample.clone()));
    let mut old: Vec<Garbage> = fx.messages(&song, SR).into_iter().filter_map(|m| r.handle(m)).collect();
    old.extend(r.handle(Msg::Song(Arc::new(song))));
    old
}

/// The burst channel on track 1; track i + 1 routes to `routes[i]`.
fn mixer(routes: &[usize]) -> Project {
    let mut p = beat(&[0]);
    p.channels[0].track = 1;
    p.mixer = std::iter::once(MixerTrack::new("Master"))
        .chain(routes.iter().map(|&to| MixerTrack { route: to, ..MixerTrack::new("Track") }))
        .collect();
    p
}

#[test]
fn tracks_route_send_and_mute() {
    let sample = burst();
    let level = |p: &Project| {
        let mut r = Renderer::new(SR);
        let _old = load(&mut r, p, PAT, &sample);
        peak(&render(&mut r, 0.05))
    };
    let mut p = mixer(&[2, 0, 0]); // 1 → 2 → master, 3 → master
    assert!(level(&p) > 0.85, "through two tracks at unity");
    p.mixer[2].mute = true;
    assert_eq!(level(&p), 0.0, "track 1 only reaches the master through track 2");
    p.mixer[1].sends.push(Send { to: 3, amount: 0.5 });
    assert!((level(&p) - 0.45).abs() < 0.01, "a half-level send around the muted track");
}

#[test]
fn pan_moves_the_signal() {
    let sample = burst();
    let mut p = beat(&[0]);
    p.mixer[0].pan = -1.0;
    let mut r = Renderer::new(SR);
    let _old = load(&mut r, &p, PAT, &sample);
    let (l, right) = render_stereo(&mut r, 0.05);
    assert!(peak(&l) > 0.85 && peak(&right) == 0.0, "hard left");
}

/// A track whose effects have had nothing to do for a second sleeps; the next sound wakes
/// it with nothing lost, and an echo still ringing keeps it awake.
#[test]
fn silent_tracks_sleep_and_wake() {
    let sample = burst();
    let mut p = mixer(&[0]);
    p.patterns[0].length = 32; // two bars, 4 s at 120 bpm
    p.patterns[0].set_step(0, 16, 127); // a second hit at 2 s, after a quiet stretch
    // one quarter-note echo (0.5 s), half wet, no feedback
    p.mixer[1].devices.push(Device { id: 7, kind: DeviceKind::Delay, params: vec![4.0, 0.0, 0.0, 0.5], bypass: false, sidechain: None, plugin: None, name: String::new(), color: None, icon: None });
    let (mut r, mut fx) = (Renderer::new(SR), Effects::default());
    let _old = load_fx(&mut r, &mut fx, &p, &sample);
    let out = render(&mut r, 3.0);
    let at = |s: f32| peak(&out[(s * SR) as usize..((s + 0.08) * SR) as usize]);
    assert!(at(0.0) > 0.3 && at(0.5) > 0.3, "the first hit and its echo");
    assert!(at(1.2) < 1e-6, "then quiet: long enough for the track to sleep");
    assert!(at(2.0) > 0.3 && at(2.5) > 0.3, "the next hit wakes it, echo and all");
}

#[test]
fn delay_echoes_and_a_macro_turns_it_fully_wet() {
    let sample = burst();
    let mut p = beat(&[0]);
    // a quarter-note delay, no feedback, no ping-pong, mix at 0 (dry)…
    p.mixer[0].devices.push(Device { id: 7, kind: DeviceKind::Delay, params: vec![4.0, 0.0, 0.0, 0.0], bypass: false, sidechain: None, plugin: None , name: String::new(), color: None, icon: None });
    // …that a macro pushes all the way wet
    p.mappings.push(Mapping { source: Source::Macro(0), target: Target::Device { id: 7, param: 3 }, amount: 1.0 });
    p.macros[0].value = 1.0;
    let (mut r, mut fx) = (Renderer::new(SR), Effects::default());
    let _old = load_fx(&mut r, &mut fx, &p, &sample);
    let out = render(&mut r, 0.6);
    assert!(peak(&out[..4 * SIXTEENTH - 100]) < 1e-3, "fully wet: the dry hit is gone");
    assert_eq!(onset(&out, 0), 4 * SIXTEENTH, "the echo lands a quarter note later");

    let song = |p: &Project| Song::compile(p, PAT, |_| None);
    assert!(fx.messages(&song(&p), SR).is_empty(), "same devices: the delay keeps its state");
    p.mixer[0].devices.clear();
    assert!(matches!(fx.messages(&song(&p), SR)[..], [Msg::RemoveEffect(7)]));
}

#[test]
fn synced_lfo_moves_the_fader_every_beat() {
    let dc = Arc::new(Sample::new(vec![0.5; 2 * SR as usize], SR));
    let mut p = beat(&[0]);
    p.lfos.push(Lfo { shape: 3, ..Lfo::default() }); // square, one cycle per beat
    p.mappings.push(Mapping { source: Source::Lfo(0), target: Target::TrackGain { track: 0 }, amount: -0.4 });
    let mut r = Renderer::new(SR);
    let _old = load(&mut r, &p, PAT, &dc);
    let out = render(&mut r, 0.5);
    let mean = |a: usize, b: usize| out[a..b].iter().map(|x| x.abs()).sum::<f32>() / (b - a) as f32;
    let beat = 4 * SIXTEENTH;
    assert!((mean(1_000, beat / 2 - 500) - 0.5 * 0.5).abs() < 0.01, "first half: fader down to 0.5");
    assert!((mean(beat / 2 + 500, beat - 500) - 0.5 * 1.25).abs() < 0.01, "second half: up to the 1.25 ceiling");
}

/// The random shape holds one level a cycle, a different one each beat, the same on every play.
#[test]
fn random_lfo_holds_a_level_per_cycle() {
    let dc = Arc::new(Sample::new(vec![0.5; 4 * SR as usize], SR));
    let mut p = beat(&[0]);
    p.lfos.push(Lfo { shape: 4, ..Lfo::default() });
    // small enough that no level reaches the fader's ends
    p.mappings.push(Mapping { source: Source::Lfo(0), target: Target::TrackGain { track: 0 }, amount: 0.1 });
    let levels = || {
        let mut r = Renderer::new(SR);
        let _old = load(&mut r, &p, PAT, &dc);
        let out = render(&mut r, 1.9);
        let beat = 4 * SIXTEENTH;
        let mean = |k: usize| out[k * beat + 1_000..(k + 1) * beat - 1_000].iter().map(|x| x.abs()).sum::<f32>() / (beat - 2_000) as f32;
        let held = |k: usize| out[k * beat + 1_000..(k + 1) * beat - 1_000].iter().all(|x| (x.abs() - mean(k)).abs() < 1e-3);
        assert!((0..3).all(held), "steady within a beat");
        [mean(0), mean(1), mean(2)]
    };
    let first = levels();
    assert!(first[0] != first[1] && first[1] != first[2], "{first:?}");
    assert_eq!(first, levels(), "the same levels the next time");
}

/// The limiter turns down before a peak gets there: never above the ceiling, and the gain
/// glides instead of jumping.
#[test]
fn limiter_looks_ahead() {
    let mut fx = ravune_engine::Effect::new(DeviceKind::Limiter, SR).unwrap();
    let mut params = [0.0; ravune_core::MAX_DEVICE_PARAMS];
    params[..2].copy_from_slice(&[0.0, 0.1]); // ceiling 0 dB, 100 ms release
    let input: Vec<f32> = (0..9_600).map(|i| if i < 4_800 { 0.5 } else { 2.0 }).collect();
    let (mut l, mut r) = (input.clone(), input.clone());
    fx.process(&params, &mut l, &mut r, 120.0, None);
    let ahead = (ravune_engine::LOOK_AHEAD * SR).round() as usize;
    assert!(l[..ahead].iter().all(|x| *x == 0.0), "delayed by the look-ahead");
    assert!(l.iter().all(|x| x.abs() <= 1.0 + 1e-6));
    let gain: Vec<f32> = (ahead..input.len()).map(|i| l[i] / input[i - ahead]).collect();
    assert!((gain[4_800] - 0.5).abs() < 1e-3, "down to half as the jump comes out");
    let steepest = gain.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
    assert!(steepest < 0.6 / ahead as f32, "a ramp, not a step: {steepest}");
}

#[test]
fn every_effect_stays_finite_and_never_allocates() {
    for kind in DeviceKind::ALL {
        let mut fx = ravune_engine::Effect::new(kind, SR).expect("built-in effect");
        let mut params = [0.0; ravune_core::MAX_DEVICE_PARAMS];
        params.iter_mut().zip(kind.defaults()).for_each(|(p, d)| *p = d);
        // loud pseudo-random noise, opposite polarity per side
        let (mut l, mut r): (Vec<f32>, Vec<f32>) =
            (0..SR as usize).map(|i| ((i * 7919 % 1000) as f32 / 500.0 - 1.0) * 0.9).map(|x| (x, -x)).unzip();
        assert_no_alloc(|| l.chunks_mut(256).zip(r.chunks_mut(256)).for_each(|(a, b)| fx.process(&params, a, b, 120.0, None)));
        assert!(l.iter().chain(&r).all(|x| x.is_finite() && x.abs() < 4.0), "{kind:?} blew up");
    }
}

#[test]
fn overview_splits_bands() {
    let tone = |hz: f32| -> Vec<f32> { (0..SR as usize).map(|i| (i as f32 * hz * std::f32::consts::TAU / SR).sin() * 0.8).collect() };
    let mean = |s: &Sample, k: usize| s.overview.iter().map(|b| b.bands[k]).sum::<f32>() / s.overview.len() as f32;
    let (bass, keys, hats) = (Sample::new(tone(60.0), SR), Sample::new(tone(900.0), SR), Sample::new(tone(8_000.0), SR));
    let loudest = |s: &Sample| (0..3).max_by(|&a, &b| mean(s, a).total_cmp(&mean(s, b))).unwrap();
    assert_eq!([loudest(&bass), loudest(&keys), loudest(&hats)], [0, 1, 2], "60 Hz low, 900 Hz mid, 8 kHz high");
    assert!(mean(&bass, 0) > 3.0 * mean(&bass, 1).max(mean(&bass, 2)), "bass doesn't leak into the mids");
    let peak = bass.overview.iter().fold(0.0f32, |m, b| m.max(b.hi).max(-b.lo));
    assert!((peak - 0.8).abs() < 0.01, "peaks keep the real level");
}

#[test]
fn a_ghost_kick_ducks_the_bass() {
    let (dc, kick) = (Arc::new(Sample::new(vec![0.5; 2 * SR as usize], SR)), burst());
    // channel 0: a steady "bass" on track 1; channel 1: a kick at step 4 on track 2, fader down
    let mut p = beat(&[0]);
    p.channels[0].instrument = Instrument::Sampler { sample: "dc".into() };
    p.channels[0].track = 1;
    let mut kick_ch = sampler(1);
    kick_ch.track = 2;
    p.channels.push(kick_ch);
    p.patterns[0].set_step(1, 4, 127);
    p.mixer = vec![MixerTrack::new("Master"), MixerTrack::new("Bass"), MixerTrack { gain: 0.0, ..MixerTrack::new("Kick") }];
    let mut duck = DeviceKind::Compressor.defaults();
    duck.copy_from_slice(&ravune_core::DUCK);
    p.mixer[1].devices.push(Device { id: 1, kind: DeviceKind::Compressor, params: duck, bypass: false, sidechain: Some(2), plugin: None , name: String::new(), color: None, icon: None });

    let song = Song::compile(&p, PAT, |path| Some(if path == "dc" { dc.clone() } else { kick.clone() }));
    let (mut r, mut fx) = (Renderer::new(SR), Effects::default());
    let _old: Vec<Garbage> = fx.messages(&song, SR).into_iter().filter_map(|m| r.handle(m)).collect();
    let _song = r.handle(Msg::Song(Arc::new(song)));
    let out = render(&mut r, 0.6);
    let mean = |a: usize, b: usize| out[a..b].iter().map(|x| x.abs()).sum::<f32>() / (b - a) as f32;
    let hit = 4 * SIXTEENTH;
    assert!((mean(1_000, hit - 500) - 0.5).abs() < 0.02, "steady before the kick");
    assert!(mean(hit + 200, hit + 4_000) < 0.1, "ducked while the (silent) kick plays");
}

/// Zero crossings per second over `x`: twice the frequency of a sine.
fn hz(x: &[f32]) -> f32 {
    let crossings = x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
    crossings as f32 / 2.0 / (x.len() as f32 / SR)
}

#[test]
fn sfz_regions_pitch_loop_and_release() {
    let dir = std::env::temp_dir().join("ravune-sfz-test");
    std::fs::create_dir_all(dir.join("Samples")).unwrap();
    let sine = |freq: f32, seconds: f32| -> Vec<f32> { (0..(SR * seconds) as usize).map(|i| (i as f32 * freq * std::f32::consts::TAU / SR).sin() * 0.5).collect() };
    ravune_engine::write_wav(&dir.join("Samples/low.wav"), &sine(220.0, 1.0), 1, SR as u32).unwrap();
    // 0.1 s long, looped over whole cycles of 880 Hz (every 600 frames at 48 kHz… 4800 frames = 88 cycles)
    ravune_engine::write_wav(&dir.join("Samples/high.wav"), &sine(880.0, 0.1), 1, SR as u32).unwrap();
    let sfz = "<control> default_path=Samples/\n<global> ampeg_release=0.05\n\
               <region> sample=low.wav lokey=0 hikey=71 pitch_keycenter=a3\n\
               <region> sample=high.wav lokey=72 hikey=127 pitch_keycenter=81 loop_mode=loop_continuous loop_start=0 loop_end=4799\n";
    let file = dir.join("test.sfz");
    std::fs::write(&file, sfz).unwrap();

    let mut p = Project { channels: vec![Channel::new(3, "multi", Instrument::Multi { file: file.to_string_lossy().into() })], ..Project::default() };
    p.channels[0].gain = 1.0;
    // A4 (69: the low region an octave up = 440 Hz) for a beat, then E5 (76: the high one) for six 16ths
    p.patterns[0].notes_mut(3).push(Note { start: 0, length: 4 * STEP, key: 57 + 12, velocity: 127, ..Default::default() });
    p.patterns[0].notes_mut(3).push(Note { start: 8 * STEP, length: 6 * STEP, key: 76, velocity: 100, ..Default::default() });
    let mut r = Renderer::new(SR);
    let song = Arc::new(Song::compile(&p, PAT, |path| Sample::decode_file(std::path::Path::new(path)).ok().map(Arc::new)));
    let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
    let out = render(&mut r, 2.0);
    let first = hz(&out[2_000..20_000]);
    assert!((first - 440.0).abs() < 5.0, "A4 plays the A3 sample an octave up, got {first} Hz");
    // one beat = 4 16ths; the 50 ms release is over well before step 6
    assert!(peak(&out[6 * SIXTEENTH..8 * SIXTEENTH]) < 1e-3, "note-off fades the held sample out");
    // key 76 is in the high region: the 880 Hz sample at root 81, so 5 semitones down
    let second = hz(&out[8 * SIXTEENTH + 2_000..8 * SIXTEENTH + 20_000]);
    let expect = 880.0 * (-5.0f32 / 12.0).exp2();
    assert!((second - expect).abs() < 5.0, "E5 from the high region, got {second} Hz, want {expect}");
    // the 0.1 s sample (0.13 s at this pitch) keeps sounding to the note's end thanks to its loop
    assert!(peak(&out[13 * SIXTEENTH..14 * SIXTEENTH - 100]) > 0.2, "looped region sustains");
    assert!(peak(&out[15 * SIXTEENTH..16 * SIXTEENTH]) < 1e-3, "and stops after the release");
}

/// A stand-in plugin host on a thread: a "synth" that outputs 0.5 from the frame a note
/// starts, or an "effect" that halves its input; `stall` makes it stop answering.
fn fake_host(path: &std::path::Path, effect: bool, stall: Arc<std::sync::atomic::AtomicBool>) -> std::thread::JoinHandle<()> {
    let mut server = ravune_bridge::Server::open(path).unwrap();
    std::thread::spawn(move || {
        let mut on = false;
        while let Some(block) = server.wait(std::time::Duration::from_secs(2)) {
            if stall.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            let [l, r] = block.output;
            for i in 0..block.frames {
                for e in block.events.iter().filter(|e| e.frame as usize == i) {
                    on = e.kind == ravune_bridge::kind::NOTE_ON;
                }
                let v = if effect { block.input[0][i] * 0.5 } else if on { 0.5 } else { 0.0 };
                (l[i], r[i]) = (v, v);
            }
            server.finish();
        }
    })
}

#[test]
fn plugin_instruments_and_effects_run_through_their_bridges() {
    let dir = std::env::temp_dir();
    let (synth_path, fx_path) = (dir.join(format!("ravune-engine-synth-{}.shm", std::process::id())), dir.join(format!("ravune-engine-fx-{}.shm", std::process::id())));
    let synth = Arc::new(ravune_bridge::Bridge::create(&synth_path).unwrap());
    let fx = Arc::new(ravune_bridge::Bridge::create(&fx_path).unwrap());
    let stall = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hosts = [fake_host(&synth_path, false, stall.clone()), fake_host(&fx_path, true, stall.clone())];

    let plugin = ravune_core::PluginRef { format: ravune_core::PluginFormat::Clap, path: "x.clap".into(), id: "x".into(), name: "X".into(), state: String::new() };
    let mut p = Project { channels: vec![Channel::new(4, "plug", Instrument::Plugin { plugin: plugin.clone() })], ..Project::default() };
    p.channels[0].gain = 1.0;
    p.patterns[0].set_step(4, 1, 127); // a note on step 2
    p.mixer[0].devices = vec![Device { id: 9, kind: DeviceKind::Plugin, params: Vec::new(), bypass: false, sidechain: None, plugin: Some(plugin) , name: String::new(), color: None, icon: None }];
    let mut r = Renderer::new(SR);
    let mut fx_state = Effects::default();
    let _garbage = load_fx(&mut r, &mut fx_state, &p, &burst());
    assert!(r.handle(Msg::Plugin { target: PluginTarget::Channel(4), bridge: Some(synth.clone()) }).is_none());
    assert!(r.handle(Msg::Plugin { target: PluginTarget::Device(9), bridge: Some(fx.clone()) }).is_none());

    let out = render(&mut r, 0.25);
    assert!(peak(&out[..SIXTEENTH]) < 1e-6, "silent before the note");
    // the note lands on its exact frame, then the effect halves it
    assert!((out[SIXTEENTH + 10] - 0.25).abs() < 1e-6, "synth 0.5 through the halving effect, got {}", out[SIXTEENTH + 10]);
    assert!(out[SIXTEENTH - 1].abs() < 1e-6 && out[SIXTEENTH].abs() > 0.2, "sample-accurate");

    // stuck hosts: the instrument goes quiet and the effect passes its (silent) input through
    stall.store(true, std::sync::atomic::Ordering::Relaxed);
    let quiet = render(&mut r, 0.1);
    assert!(peak(&quiet[quiet.len() / 2..]) < 1e-6, "late plugins are silent, never stall the block");
    assert!(synth.late() > 0 && fx.late() > 0, "both counted late");
    let old = r.handle(Msg::Plugin { target: PluginTarget::Channel(4), bridge: None });
    assert!(matches!(old, Some(Garbage::Plugin(_))), "the bridge comes back to be dropped off the audio thread");
    synth.kill();
    fx.kill();
    drop(hosts);
}

#[test]
fn mpe_bends_only_its_own_note() {
    // a pure sine, wide open, no envelope movement: pitch is all there is
    let mut params = default_synth();
    for (p, v) in [(ravune_core::Param::Osc1Wave, 0.0), (ravune_core::Param::OscMix, 0.0), (ravune_core::Param::Cutoff, 18000.0), (ravune_core::Param::EnvAmount, 0.0), (ravune_core::Param::Attack, 0.001), (ravune_core::Param::Sustain, 1.0)] {
        params[p as usize] = v;
    }
    let project = Project { channels: vec![Channel::new(5, "va", Instrument::Synth { params })], ..Project::default() };
    let mut r = Renderer::new(SR);
    let _old = load(&mut r, &project, Transport { playing: false, ..PAT }, &burst());
    r.handle(Msg::Target(Some(5)));
    let hz = |x: &[f32]| x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as f32 / 2.0 / (x.len() as f32 / SR);
    let bend = |channel: u8, v: f32| {
        let raw = ((v + 1.0) * 8192.0).round().min(16383.0) as u16;
        Msg::Midi([0xE0 | channel, (raw & 127) as u8, (raw >> 7) as u8])
    };
    r.handle(Msg::Midi([0x91, 69, 100])); // A4 on member channel 2
    let a = render(&mut r, 0.2);
    assert!((hz(&a[2000..]) - 440.0).abs() < 4.0, "A4, got {}", hz(&a[2000..]));
    r.handle(bend(2, 0.5)); // another channel's bend: not ours
    assert!((hz(&render(&mut r, 0.2)[2000..]) - 440.0).abs() < 4.0);
    r.handle(bend(1, 0.25)); // ours: a quarter of ±48 = +12 semitones
    let up = render(&mut r, 0.2);
    assert!((hz(&up[2000..]) - 880.0).abs() < 6.0, "an octave up, got {}", hz(&up[2000..]));
    r.handle(Msg::Midi([0x81, 69, 0]));
    r.handle(bend(1, 0.0));
    render(&mut r, 1.0); // let it release
    r.handle(Msg::Midi([0x90, 69, 100])); // master channel: a plain keyboard
    r.handle(bend(0, 1.0)); // full wheel = +2 semitones for everything
    let wheel = render(&mut r, 0.2);
    let want = 440.0 * (2.0f32 / 12.0).exp2();
    assert!((hz(&wheel[2000..]) - want).abs() < 5.0, "wheel up a tone, got {} want {want}", hz(&wheel[2000..]));
}

#[test]
fn automation_clips_move_their_knob_while_the_song_plays() {
    let mut r = Renderer::new(SR);
    let dc = Arc::new(Sample::new(vec![0.5; 4 * BAR_SAMPLES], SR));
    let mut p = beat(&[]);
    // the master silent for two bars, then at 0 dB
    let points = vec![(0, 0.0), (2 * BAR, 0.0), (2 * BAR + 1, 0.8)];
    p.clips = vec![
        Clip { lane: 0, start: 0, length: 4 * BAR, offset: 0, fx: Default::default(), kind: ClipKind::Audio { sample: "dc".into() } },
        Clip { lane: 1, start: 0, length: 4 * BAR, offset: 0, fx: Default::default(), kind: ClipKind::Automation { target: Target::TrackGain { track: 0 }, points, curves: vec![] } },
    ];
    let song = Arc::new(Song::compile(&p, SONG, |_| Some(dc.clone())));
    let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
    let out = render(&mut r, 3.0 * BAR_SAMPLES as f32 / SR);
    assert!(peak(&out[BAR_SAMPLES / 2..BAR_SAMPLES]) < 1e-3, "automated down");
    assert!(peak(&out[2 * BAR_SAMPLES + BAR_SAMPLES / 2..]) > 0.3, "automated back up");
}

#[test]
fn recording_takes_the_master_while_the_song_plays() {
    let dc = Arc::new(Sample::new(vec![0.5; 4 * BAR_SAMPLES], SR));
    let mut p = beat(&[]);
    p.clips = vec![Clip { lane: 0, start: 0, length: 4 * BAR, offset: 0, fx: Default::default(), kind: ClipKind::Audio { sample: "dc".into() } }];
    let mut r = Renderer::new(SR);
    let (tap, mut taken) = ravune_engine::rtrb::RingBuffer::<f32>::new(SR as usize * 2);
    assert!(r.handle(Msg::Record(Some(tap))).is_none());
    let song = Arc::new(Song::compile(&p, SONG, |_| Some(dc.clone())));
    let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
    let (l, _) = render_stereo(&mut r, 0.25);
    assert_eq!(taken.slots(), 2 * l.len(), "every frame, left and right");
    let first: Vec<f32> = (0..2 * l.len()).map(|_| taken.pop().unwrap()).collect();
    assert!((first[2 * 4_000] - l[4_000]).abs() < 1e-6, "what was heard (no output volume here)");
    // stopped: nothing more; Record(None) hands the ring back to be freed off the audio thread
    let song = Arc::new(Song::compile(&p, Transport { playing: false, ..SONG }, |_| Some(dc.clone())));
    let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
    render(&mut r, 0.1);
    assert_eq!(taken.slots(), 0);
    assert!(matches!(r.handle(Msg::Record(None)), Some(ravune_engine::Garbage::Tap(_))));
}

#[test]
fn a_loop_region_plays_over_and_over() {
    // sound in bar 1 and bar 4: the song is four bars long
    let dc = Arc::new(Sample::new(vec![0.5; BAR_SAMPLES], SR));
    let mut p = beat(&[]);
    let clip = |start: u32| Clip { lane: 0, start, length: BAR, offset: 0, fx: Default::default(), kind: ClipKind::Audio { sample: "dc".into() } };
    p.clips = vec![clip(0), clip(3 * BAR)];
    let play = |t: Transport| {
        let mut r = Renderer::new(SR);
        let song = Arc::new(Song::compile(&p, t, |_| Some(dc.clone())));
        let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
        render(&mut r, 2.0 * BAR_SAMPLES as f32 / SR)
    };
    let whole = play(SONG);
    assert!(peak(&whole[BAR_SAMPLES + 2_000..]) < 1e-3, "bar 2 is empty");
    let first = play(Transport { looped: Some((0, BAR)), ..SONG });
    assert!(peak(&first[BAR_SAMPLES + 2_000..]) > 0.3, "looping bar 1: it comes round again");
    let quiet = play(Transport { looped: Some((BAR, 2 * BAR)), ..SONG });
    assert!(peak(&quiet) < 1e-3, "looping bar 2 starts there and never reaches bar 4");
}

#[test]
fn a_sound_that_began_before_the_loop_plays_every_time_round() {
    // one long sound under the whole song, a loop on bar 2
    let dc = Arc::new(Sample::new(vec![0.5; 4 * BAR_SAMPLES], SR));
    let mut p = beat(&[]);
    p.clips = vec![Clip { lane: 0, start: 0, length: 4 * BAR, offset: 0, fx: Default::default(), kind: ClipKind::Audio { sample: "dc".into() } }];
    let mut r = Renderer::new(SR);
    let song = Arc::new(Song::compile(&p, Transport { looped: Some((BAR, 2 * BAR)), ..SONG }, |_| Some(dc.clone())));
    let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
    let out = render(&mut r, 3.0 * BAR_SAMPLES as f32 / SR);
    for round in 0..3 {
        let bar = &out[round * BAR_SAMPLES + 1_000..(round + 1) * BAR_SAMPLES - 1_000];
        assert!(bar.iter().all(|x| x.abs() > 0.2), "round {round}: it plays all through");
    }
}

#[test]
fn a_loop_follows_the_tempo_when_stretched_or_resampled() {
    // a 2-second loop made at 120 BPM (one bar), in a song at 160
    let dc = Arc::new(Sample::new(vec![0.5; 2 * SR as usize], SR));
    let long = |stretch: Stretch| {
        let mut r = Renderer::new(SR);
        let mut p = beat(&[]);
        p.tempo = 160.0;
        let fx = ClipFx { stretch, bpm: 120.0, ..Default::default() };
        p.clips = vec![Clip { lane: 0, start: 0, length: 4 * BAR, offset: 0, fx, kind: ClipKind::Audio { sample: "dc".into() } }];
        let song = Arc::new(Song::compile(&p, SONG, |_| Some(dc.clone())));
        let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
        let out = render(&mut r, 2.5);
        out.iter().rposition(|x| x.abs() > 0.1).unwrap_or(0) as f32 / SR
    };
    assert!((long(Stretch::Off) - 2.0).abs() < 0.02, "off: its own speed");
    // one bar at 160 is 1.5 s
    assert!((long(Stretch::Stretch) - 1.5).abs() < 0.05, "stretched to the bar: {}", long(Stretch::Stretch));
    assert!((long(Stretch::Resample) - 1.5).abs() < 0.02, "resampled to the bar: {}", long(Stretch::Resample));
}

#[test]
fn automation_moves_a_channel_volume() {
    let mut r = Renderer::new(SR);
    let mut p = beat(&[0]);
    let points = vec![(0, 0.0), (BAR - 1, 0.0), (BAR, 1.0)];
    p.clips = vec![
        Clip { lane: 0, start: 0, length: 2 * BAR, offset: 0, fx: Default::default(), kind: ClipKind::Pattern { pattern: 0 } },
        Clip { lane: 1, start: 0, length: 2 * BAR, offset: 0, fx: Default::default(), kind: ClipKind::Automation { target: Target::ChannelGain { slot: 0 }, points, curves: vec![] } },
    ];
    let song = Arc::new(Song::compile(&p, SONG, |_| Some(burst())));
    let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
    let out = render(&mut r, 2.0 * BAR_SAMPLES as f32 / SR);
    assert!(peak(&out[..BAR_SAMPLES / 2]) < 1e-3, "silent while automated to 0");
    assert!(peak(&out[BAR_SAMPLES..BAR_SAMPLES + BAR_SAMPLES / 2]) > 0.3, "back at full volume");
}

#[test]
fn a_patterns_automation_goes_where_the_pattern_goes() {
    // hits on beats 1 and 3; inside the pattern the channel is silent for the first half
    let mut p = beat(&[0, 8]);
    let points = vec![(0, 0.0), (BAR / 2 - 1, 0.0), (BAR / 2, 1.0)];
    p.patterns[0].automation = vec![PatternAuto { target: Target::ChannelGain { slot: 0 }, points, curves: vec![] }];
    let half = BAR_SAMPLES / 2;
    let play = |p: &Project, t: Transport, seconds: f32| {
        let mut r = Renderer::new(SR);
        let song = Arc::new(Song::compile(p, t, |_| Some(burst())));
        let _old = assert_no_alloc(|| r.handle(Msg::Song(song)));
        render(&mut r, seconds)
    };
    let out = play(&p, PAT, BAR_SAMPLES as f32 / SR);
    assert!(peak(&out[..half]) < 1e-3 && peak(&out[half..]) > 0.3, "looping the pattern");
    // on the playlist from bar 3
    p.clips = vec![Clip { lane: 0, start: 2 * BAR, length: BAR, offset: 0, fx: Default::default(), kind: ClipKind::Pattern { pattern: 0 } }];
    let out = play(&p, SONG, 3.0 * BAR_SAMPLES as f32 / SR);
    let bar3 = &out[2 * BAR_SAMPLES..];
    assert!(peak(&bar3[..half]) < 1e-3, "silent for the first half of the clip");
    assert!(peak(&bar3[half..]) > 0.3, "then the second hit");
}

/// The record tap: the master as interleaved stereo while playing, nothing once stopped.
#[test]
fn record_tap_copies_the_master_while_playing() {
    let mut r = Renderer::new(SR);
    let _old = load(&mut r, &beat(&[0]), PAT, &burst());
    let (tap, mut ring) = ravune_engine::rtrb::RingBuffer::new(48_000);
    assert!(r.handle(Msg::Record(Some(tap))).is_none());
    let (l, rt) = render_stereo(&mut r, 0.1);
    let got: Vec<f32> = std::iter::from_fn(|| ring.pop().ok()).collect();
    assert_eq!(got.len(), 2 * l.len());
    assert!(got.chunks(2).zip(l.iter().zip(&rt)).all(|(f, (a, b))| f[0].clamp(-1.0, 1.0) == *a && f[1].clamp(-1.0, 1.0) == *b));
    assert!(peak(&got) > 0.5, "the step is in the take");
    let _old = load(&mut r, &beat(&[0]), Transport { playing: false, ..PAT }, &burst());
    render_stereo(&mut r, 0.1);
    assert!(ring.is_empty(), "stopped: nothing recorded");
    assert!(matches!(r.handle(Msg::Record(None)), Some(Garbage::Tap(_))));
}

#[test]
fn the_strip_eq_shapes_a_track() {
    let sample = burst(); // a 2.4 kHz square: harmonics up past the high shelf
    let energy = |eq: [f32; 3]| {
        let mut p = beat(&[0]);
        p.mixer[0].eq = eq;
        p.mixer[0].gain = 0.1; // the fader comes after the EQ: boosts stay clear of the output's clip
        let mut r = Renderer::new(SR);
        let _old = load(&mut r, &p, PAT, &sample);
        render(&mut r, 0.1).iter().map(|x| x * x).sum::<f32>()
    };
    let flat = energy([0.0; 3]);
    assert!(energy([-15.0; 3]) < flat * 0.5, "cut everywhere: quieter");
    assert!(energy([0.0, 0.0, 15.0]) > flat * 1.5, "more highs: brighter, louder");
    assert_eq!(energy([0.0; 3]), flat, "flat leaves it untouched");
}

/// A 44.1 kHz sound played at 48 kHz (the library on most devices), plainly and stretched:
/// the alias resampling folds back (12 kHz → 15.9 kHz) stays far under the tone (a
/// straight line between frames left it 17 dB under).
#[test]
fn resampling_keeps_aliases_down() {
    use std::f32::consts::TAU;
    let rate = 44_100.0;
    let tone = Arc::new(Sample::new((0..2 * rate as usize).map(|i| (i as f32 * TAU * 12_000.0 / rate).sin() * 0.5).collect(), rate));
    // windowed single-bin DFT
    let level = |x: &[f32], hz: f32| {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (k, v) in x.iter().enumerate() {
            let w = (std::f64::consts::PI * k as f64 / x.len() as f64).sin().powi(2) * *v as f64;
            let phase = std::f64::consts::TAU * hz as f64 * k as f64 / SR as f64;
            (re, im) = (re + w * phase.cos(), im + w * phase.sin());
        }
        re.hypot(im)
    };
    let alias = |x: &[f32]| 20.0 * (level(x, 15_900.0) / level(x, 12_000.0)).log10();
    let (mut l, mut r) = (vec![0.0; 24_000], vec![0.0; 24_000]);
    let mut s = ravune_engine::Sampler::default();
    s.trigger(tone.clone(), 1.0, 0.0, SR);
    s.render(&mut l, &mut r);
    let played = alias(&l[4_000..20_000]);
    let (mut l, mut r) = (vec![0.0; 24_000], vec![0.0; 24_000]);
    let mut s = ravune_engine::Stretcher::new();
    s.trigger(tone, 1.0, SR, 0.0, 1.0, 0.0, false);
    s.render(&mut l, &mut r);
    let stretched = alias(&l[4_000..20_000]);
    assert!(played < -45.0 && stretched < -45.0, "aliases: played {played:.1} dB, stretched {stretched:.1} dB");
}
