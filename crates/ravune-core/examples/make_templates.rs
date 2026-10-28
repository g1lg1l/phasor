//! Writes the starter templates (File → New from Template) into `assets/templates`: a kit
//! and a beat in each genre, a bass and a chord voice, a loop from the genre's pack, the
//! usual mixer, eight bars to build on.
//!
//!     cargo run -p ravune-core --example make_templates

use ravune_core::{
    BAR, Channel, Clip, ClipFx, ClipKind, DUCK, Device, DeviceKind as Fx, Instrument, LaneInfo, MixerTrack, Note, Param, Pattern, Project, STEP,
    Scale, Send, default_synth,
};

fn pattern(name: &str, bars: usize, color: [u8; 3]) -> Pattern {
    Pattern { length: bars * 16, color: Some(color), ..Pattern::new(name) }
}

/// Steps the rack can show: on the grid, so the song's swing moves the off-beat ones.
fn steps(pattern: &mut Pattern, slot: u8, list: &[(usize, u8)]) {
    list.iter().for_each(|&(s, v)| pattern.set_step(slot, s, v));
}

/// A note from a 16th step, `nudge` ticks early or late (one tick ≈ 4 ms at 147 BPM), `ticks` long.
fn note(pattern: &mut Pattern, slot: u8, step: u32, nudge: i32, ticks: u32, key: u8, velocity: u8) {
    let start = ((step * STEP) as i32 + nudge).max(0) as u32;
    pattern.notes_mut(slot).push(Note { start, length: ticks, key, velocity, ..Default::default() });
}

fn synth(params: &[(Param, f32)]) -> Instrument {
    let mut p = default_synth();
    params.iter().for_each(|(k, v)| p[*k as usize] = *v);
    Instrument::Synth { params: p }
}

/// Adds an effect with some parameters changed from their defaults; returns its id.
fn device(p: &mut Project, track: usize, kind: Fx, params: &[(usize, f32)]) -> u32 {
    let id = p.next_id;
    p.next_id += 1;
    let mut values = kind.defaults();
    params.iter().for_each(|&(i, v)| values[i] = v);
    p.mixer[track].devices.push(Device { id, kind, params: values, bypass: false, sidechain: None, plugin: None, name: String::new(), color: None, icon: None });
    id
}

/// A compressor on `track` keyed by `source` ("Duck to…"), `depth` 0..1 of the full duck,
/// `makeup` dB back up between the hits.
fn duck(p: &mut Project, track: usize, source: usize, depth: f32, makeup: f32) {
    device(p, track, Fx::Compressor, &[]);
    let d = p.mixer[track].devices.last_mut().unwrap();
    d.params.copy_from_slice(&DUCK);
    (d.params[0], d.params[1], d.params[4]) = (DUCK[0] * depth, 1.0 + (DUCK[1] - 1.0) * depth, makeup);
    d.sidechain = Some(source);
}

fn track(name: &str, gain: f32, pan: f32, route: usize, color: [u8; 3], icon: &str) -> MixerTrack {
    MixerTrack { gain, pan, route, color: Some(color), icon: Some(icon.into()), ..MixerTrack::new(name) }
}

fn sends(p: &mut Project, track: usize, list: &[(usize, f32)]) {
    p.mixer[track].sends = list.iter().map(|&(to, amount)| Send { to, amount }).collect();
}

// palette: drums warm, loops sand, bass blue, synths violet
const RED: [u8; 3] = [232, 84, 58];
const ORANGE: [u8; 3] = [238, 132, 70];
const SAND: [u8; 3] = [222, 166, 78];
const BLUE: [u8; 3] = [78, 138, 240];
const VIOLET: [u8; 3] = [166, 108, 230];

/// A starter for File → New from Template: a kit and a beat in the genre, a bass and a
/// chord voice, a loop from the genre's pack, the usual mixer, eight bars to build on.
struct Starter {
    file: &'static str,
    tempo: f32,
    swing: f32,
    root: u8,
    scale: Scale,
    /// Kick, snare or clap, closed hat, open hat and two percussion sounds (`builtin:` names).
    kit: [(&'static str, &'static str); 6],
    /// The steps each kit sound plays on, over `bars`.
    beat: [&'static [usize]; 6],
    bars: usize,
    /// (step, length in steps, key)
    bass: &'static [(u32, u32, u8)],
    bass_sound: &'static [(Param, f32)],
    /// (step, length in steps, chord)
    chords: &'static [(u32, u32, [u8; 4])],
    chord_sound: &'static [(Param, f32)],
    /// A loop from the pack: file, its tempo, its length in bars.
    groove: Option<(&'static str, f32, u32)>,
}

fn starter(s: &Starter) -> Project {
    const DRUMS: usize = 1;
    const KICK: usize = 2;
    const BASS: usize = 3;
    const MUSIC: usize = 4;
    const LOOP: usize = 5;
    const VERB: usize = 6;
    const DELAY: usize = 7;
    let mut channels: Vec<Channel> = s.kit.iter().enumerate().map(|(slot, (name, file))| {
        let mut c = Channel::new(slot as u8, *name, Instrument::Sampler { sample: format!("builtin:{file}") });
        c.track = if slot == 0 { KICK } else { DRUMS };
        c.choke = if slot == 2 || slot == 3 { 1 } else { 0 };
        c
    }).collect();
    for (slot, name, sound, track) in [(6u8, "Bass", s.bass_sound, BASS), (7, "Chords", s.chord_sound, MUSIC)] {
        let mut c = Channel::new(slot, name, synth(sound));
        (c.gain, c.track) = (1.0, track);
        channels.push(c);
    }
    let mut p = Project { tempo: s.tempo, swing: s.swing, root: s.root, scale: s.scale, channels, ..Project::default() };

    let mut beat = pattern("Beat", s.bars, RED);
    for (slot, list) in s.beat.iter().enumerate() {
        // accents on the beat, hats softer between
        let hat = slot == 2;
        steps(&mut beat, slot as u8, &list.iter().map(|&st| (st, if st % 4 == 0 { 112 } else if hat && st % 2 == 1 { 62 } else { 88 })).collect::<Vec<_>>());
    }
    let mut bass = pattern("Bass", s.bars, BLUE);
    for &(st, len, key) in s.bass {
        note(&mut bass, 6, st, 0, len * STEP, key, 104);
    }
    let mut chords = pattern("Chords", s.bars.max(2), VIOLET);
    for &(st, len, chord) in s.chords {
        chord.iter().for_each(|&key| note(&mut chords, 7, st, 0, len * STEP, key, 92));
    }
    p.patterns = vec![beat, bass, chords];

    // eight bars: the drums and the loop, then the bass and chords join halfway
    const LANES: [(&str, [u8; 3]); 4] = [("Drums", RED), ("Loop", ORANGE), ("Bass", BLUE), ("Chords", VIOLET)];
    p.lanes = LANES.iter().map(|(name, color)| LaneInfo { name: (*name).into(), color: Some(*color), mute: false, solo: false }).collect();
    let clip = |lane: u32, pattern: usize, from: u32| Clip { lane, start: from * BAR, length: (8 - from) * BAR, offset: 0, fx: ClipFx::default(), kind: ClipKind::Pattern { pattern } };
    p.clips = vec![clip(0, 0, 0), clip(2, 1, 4), clip(3, 2, 4)];
    if let Some((file, bpm, bars)) = s.groove {
        for from in (0..8).step_by(bars as usize) {
            let fx = ClipFx { track: LOOP, stretch: ravune_core::Stretch::Stretch, bpm, gain: -3.0, ..ClipFx::default() };
            p.clips.push(Clip { lane: 1, start: from * BAR, length: bars * BAR, offset: 0, fx, kind: ClipKind::Audio { sample: format!("builtin:{file}") } });
        }
    }

    p.mixer = vec![
        track("Master", 1.0, 0.0, 0, [214, 214, 214], "speaker"),
        track("Drums", 0.7, 0.0, 0, ORANGE, "bus"),
        track("Kick", 0.85, 0.0, DRUMS, RED, "fire"),
        track("Bass", 1.0, 0.0, 0, BLUE, "planet"),
        track("Music", 1.0, 0.0, 0, VIOLET, "synth"),
        track("Loop", 0.6, 0.0, 0, SAND, "spiral"),
        track("Reverb", 0.6, 0.0, 0, [120, 170, 220], "snow"),
        track("Delay", 0.5, 0.0, 0, [110, 200, 210], "drop"),
    ];
    sends(&mut p, DRUMS, &[(VERB, 0.06)]);
    sends(&mut p, MUSIC, &[(VERB, 0.25), (DELAY, 0.18)]);
    sends(&mut p, LOOP, &[(VERB, 0.08)]);
    device(&mut p, 0, Fx::Eq, &[]);
    device(&mut p, 0, Fx::Compressor, &[(0, -18.0), (1, 2.0), (2, 0.02), (3, 0.2), (4, 6.0)]);
    device(&mut p, 0, Fx::Limiter, &[]);
    device(&mut p, DRUMS, Fx::Compressor, &[(0, -22.0), (1, 2.0), (2, 0.012), (3, 0.12), (4, 2.0)]);
    device(&mut p, BASS, Fx::Eq, &[]);
    duck(&mut p, BASS, KICK, 0.7, 0.0);
    device(&mut p, LOOP, Fx::Eq, &[(0, -8.0)]);
    device(&mut p, VERB, Fx::Reverb, &[(0, 0.6), (1, 0.5), (2, 1.0), (3, 1.0)]);
    device(&mut p, DELAY, Fx::Delay, &[(0, 3.0), (1, 0.35), (2, 1.0), (3, 1.0)]);
    p
}

const SUB: &[(Param, f32)] = &[(Param::Osc1Wave, 0.0), (Param::Osc2Wave, 1.0), (Param::OscMix, 0.2), (Param::Cutoff, 300.0), (Param::Attack, 0.002), (Param::Decay, 0.3), (Param::Sustain, 0.7), (Param::Release, 0.08), (Param::Gain, 1.0)];
const ACID: &[(Param, f32)] = &[(Param::Osc1Wave, 2.0), (Param::OscMix, 0.0), (Param::Cutoff, 500.0), (Param::Resonance, 0.6), (Param::EnvAmount, 0.6), (Param::Attack, 0.001), (Param::Decay, 0.18), (Param::Sustain, 0.0), (Param::Release, 0.05), (Param::Gain, 1.0)];
const REESE: &[(Param, f32)] = &[(Param::Osc1Wave, 2.0), (Param::Osc2Wave, 2.0), (Param::Osc2Detune, 18.0), (Param::OscMix, 0.5), (Param::Cutoff, 700.0), (Param::Resonance, 0.2), (Param::Attack, 0.005), (Param::Decay, 0.4), (Param::Sustain, 0.8), (Param::Release, 0.1), (Param::Gain, 1.0)];
const STAB: &[(Param, f32)] = &[(Param::Osc1Wave, 2.0), (Param::Osc2Wave, 3.0), (Param::Osc2Detune, 10.0), (Param::OscMix, 0.5), (Param::Cutoff, 1200.0), (Param::Resonance, 0.4), (Param::EnvAmount, 0.5), (Param::Attack, 0.001), (Param::Decay, 0.2), (Param::Sustain, 0.0), (Param::Release, 0.2), (Param::Gain, 1.0)];
const PAD: &[(Param, f32)] = &[(Param::Osc1Wave, 2.0), (Param::Osc2Wave, 1.0), (Param::Osc2Detune, 8.0), (Param::OscMix, 0.5), (Param::Cutoff, 1800.0), (Param::Attack, 0.4), (Param::Decay, 1.0), (Param::Sustain, 0.8), (Param::Release, 1.2), (Param::Gain, 0.45)];
const BELLS: &[(Param, f32)] = &[(Param::Osc1Wave, 0.0), (Param::Osc2Wave, 0.0), (Param::Osc2Pitch, 19.0), (Param::OscMix, 0.35), (Param::Cutoff, 6000.0), (Param::Attack, 0.001), (Param::Decay, 0.5), (Param::Sustain, 0.0), (Param::Release, 0.6), (Param::Gain, 1.0)];

const FOUR: &[usize] = &[0, 4, 8, 12];
const OFFBEATS: &[usize] = &[2, 6, 10, 14];
const SIXTEENTHS: &[usize] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
const EIGHTHS: &[usize] = &[0, 2, 4, 6, 8, 10, 12, 14];

const STARTERS: [Starter; 7] = [
    Starter {
        file: "techno",
        tempo: 130.0,
        swing: 0.0,
        root: 9,
        scale: Scale::Minor,
        kit: [("Kick", "Techno/kick-tight.flac"), ("Clap", "Techno/clap-snare.flac"), ("Hat", "Techno/hat-closed-analog.flac"), ("Open Hat", "Techno/hat-open-short.flac"), ("Ride", "Techno/ride-edge.flac"), ("Rumble", "Techno/rumble-kick-128.flac")],
        beat: [FOUR, &[4, 12], &[0, 1, 3, 4, 5, 7, 8, 9, 11, 12, 13, 15], OFFBEATS, &[2, 6, 10, 14], &[]],
        bars: 1,
        bass: &[(2, 1, 33), (6, 1, 33), (10, 1, 33), (13, 1, 36), (14, 1, 33)],
        bass_sound: ACID,
        chords: &[(6, 1, [57, 60, 64, 67]), (27, 1, [57, 60, 64, 67])],
        chord_sound: STAB,
        groove: Some(("Techno/perc-loop-dub-128.flac", 128.0, 2)),
    },
    Starter {
        file: "house",
        tempo: 124.0,
        swing: 0.25,
        root: 5,
        scale: Scale::Minor,
        kit: [("Kick", "House/kick-deep.flac"), ("Clap", "House/clap-house.flac"), ("Hat", "House/hat-closed.flac"), ("Open Hat", "Hi-hats/hihat-open-minipops.wav"), ("Shaker", "House/shaker.flac"), ("Organ", "House/organ-rave.flac")],
        beat: [FOUR, &[4, 12], &[0, 4, 8, 12], OFFBEATS, SIXTEENTHS, &[3, 11, 14]],
        bars: 1,
        bass: &[(2, 2, 29), (6, 1, 29), (10, 2, 32), (14, 1, 29)],
        bass_sound: SUB,
        chords: &[(0, 3, [53, 56, 60, 63]), (6, 2, [53, 56, 60, 63]), (16, 3, [51, 55, 58, 62]), (22, 2, [51, 55, 58, 62])],
        chord_sound: STAB,
        groove: Some(("House/bongo-loop-120.flac", 120.0, 2)),
    },
    Starter {
        file: "drum-and-bass",
        tempo: 174.0,
        swing: 0.0,
        root: 4,
        scale: Scale::Minor,
        kit: [("Kick", "Drum & Bass/kick-punchy.flac"), ("Snare", "Drum & Bass/snare-crack.flac"), ("Hat", "Drum & Bass/hat-closed.flac"), ("Ride", "Drum & Bass/ride.flac"), ("Ghost Snare", "Drum & Bass/snare-tight.flac"), ("Stab", "Drum & Bass/stab-neuro.flac")],
        beat: [&[0, 10, 16, 26], &[4, 12, 20, 28], EIGHTHS, &[], &[7, 15, 23, 30], &[]],
        bars: 2,
        bass: &[(0, 6, 28), (10, 4, 28), (16, 6, 31), (26, 4, 26)],
        bass_sound: REESE,
        chords: &[],
        chord_sound: PAD,
        groove: Some(("Drum & Bass/break-rolling-174.flac", 174.0, 2)),
    },
    Starter {
        file: "liquid-dnb",
        tempo: 174.0,
        swing: 0.0,
        root: 9,
        scale: Scale::Minor,
        kit: [("Kick", "Drum & Bass/kick-punchy.flac"), ("Snare", "Liquid DnB/snare-soft.flac"), ("Hat", "Drum & Bass/hat-closed.flac"), ("Ride", "Drum & Bass/ride.flac"), ("Shaker", "Liquid DnB/shaker-egg.flac"), ("Rim", "Liquid DnB/rim-soft.flac")],
        beat: [&[0, 10, 16, 26], &[4, 12, 20, 28], EIGHTHS, &[], &[2, 6, 10, 14, 18, 22, 26, 30], &[7, 23]],
        bars: 2,
        bass: &[(0, 8, 33), (10, 6, 31), (16, 8, 29), (26, 6, 28)],
        bass_sound: SUB,
        chords: &[(0, 16, [57, 60, 64, 67]), (16, 16, [53, 57, 60, 64])],
        chord_sound: PAD,
        groove: Some(("Liquid DnB/break-liquid-174.flac", 174.0, 4)),
    },
    Starter {
        file: "hardgroove",
        tempo: 145.0,
        swing: 0.36,
        root: 9,
        scale: Scale::Minor,
        kit: [("Kick", "kick-909.wav"), ("Clap", "Claps/clap-electronic-snappy.wav"), ("Hat", "hat-closed.wav"), ("Open Hat", "hat-open.wav"), ("Bongo", "Hardgroove/bongo-high.flac"), ("Rim", "Hardgroove/rim-tight.flac")],
        beat: [FOUR, &[4, 12], SIXTEENTHS, OFFBEATS, &[3, 6, 11, 14], &[2, 5, 7, 13]],
        bars: 1,
        bass: &[(2, 1, 33), (7, 1, 45), (10, 1, 33), (11, 1, 36), (14, 1, 33)],
        bass_sound: ACID,
        chords: &[(6, 1, [57, 60, 64, 67])],
        chord_sound: STAB,
        groove: Some(("Hardgroove/perc-loop-wood-145.flac", 145.0, 4)),
    },
    Starter {
        file: "idm",
        tempo: 120.0,
        swing: 0.0,
        root: 2,
        scale: Scale::Dorian,
        kit: [("Kick", "IDM/kick-crushed.flac"), ("Snare", "IDM/snare-crushed.flac"), ("Click", "IDM/click-glitch.flac"), ("Blip", "IDM/blip-fm-ping.flac"), ("Glitch", "IDM/glitch-hit.flac"), ("Rim", "IDM/rim-crushed.flac")],
        beat: [&[0, 3, 10, 19, 22, 27], &[8, 24, 30], &[1, 2, 5, 9, 11, 13, 14, 17, 21, 25, 26, 29, 31], &[6, 15, 28], &[12, 23], &[4, 18, 20]],
        bars: 2,
        bass: &[(0, 3, 38), (10, 2, 41), (19, 3, 36), (27, 2, 38)],
        bass_sound: SUB,
        chords: &[(0, 8, [62, 65, 69, 72]), (16, 8, [60, 64, 67, 71])],
        chord_sound: BELLS,
        groove: Some(("IDM/loop-glitch-120.flac", 120.0, 2)),
    },
    Starter {
        file: "hip-hop",
        tempo: 90.0,
        swing: 0.45,
        root: 0,
        scale: Scale::Minor,
        kit: [("Kick", "Kicks/kick-808-deep.wav"), ("Snare", "Snares/snare-acoustic-tight.wav"), ("Hat", "Hi-hats/hihat-closed-acoustic.wav"), ("Open Hat", "Hi-hats/hihat-open-acoustic.wav"), ("Clap", "Claps/clap-acoustic.wav"), ("Rim", "rim.wav")],
        beat: [&[0, 7, 10, 16, 23, 26, 27], &[4, 12, 20, 28], &[0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30], &[30], &[12, 28], &[15]],
        bars: 2,
        bass: &[(0, 6, 24), (10, 4, 24), (16, 6, 27), (26, 4, 22)],
        bass_sound: SUB,
        chords: &[(0, 16, [60, 63, 67, 70]), (16, 16, [56, 60, 63, 67])],
        chord_sound: PAD,
        groove: None,
    },
];

fn main() -> std::io::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let write = |dir: &str, name: &str, p: &Project| -> std::io::Result<()> {
        std::fs::create_dir_all(root.join(dir))?;
        let text = ron::ser::to_string_pretty(p, ron::ser::PrettyConfig::default().compact_arrays(true)).expect("serializable");
        std::fs::write(root.join(dir).join(format!("{name}.ravune")), text)?;
        println!("wrote {dir}/{name}.ravune ({} bars at {} BPM)", p.song_end() / BAR, p.tempo);
        Ok(())
    };
    for s in &STARTERS {
        write("templates", s.file, &starter(s))?;
    }
    Ok(())
}
