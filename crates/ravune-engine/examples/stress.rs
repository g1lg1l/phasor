//! A busy project: many loops at once, each stretched to the song's tempo through its own
//! effect chain. Renders it block by block as the audio thread would, and prints the
//! slowest block against its time budget and how much of the output was clipped.
//!
//!     cargo run --release -p ravune-engine --example stress -- out.wav [loops] [--limiter] [--resample] [--hot] [--dry]
//!
//! `--limiter` puts a limiter on the master, as the templates do; `--resample` plays the
//! loops faster instead of stretching them; `--hot` brings every loop up to full scale;
//! `--dry` leaves the effects out.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use ravune_core::{BAR, Clip, ClipFx, ClipKind, Device, DeviceKind, MixerTrack, Project, Stretch};
use ravune_engine::{Effects, Mode, Msg, Renderer, Sample, Song, Transport, write_wav};

const LOOPS: [(&str, f32); 8] = [
    ("Electronic/drumloop-130.flac", 130.0),
    ("House/bassline-124.flac", 124.0),
    ("Electronic/arp-loop-120.flac", 120.0),
    ("Electronic/chords-bouncy-132.flac", 132.0),
    ("Hardgroove/perc-loop-noise-145.flac", 145.0),
    ("Drum & Bass/break-rolling-174.flac", 174.0),
    ("Hardgroove/perc-loop-ringy-140.flac", 140.0),
    ("Electronic/arp-pluck-120.flac", 120.0),
];

fn device(id: u32, kind: DeviceKind) -> Device {
    Device { id, kind, params: kind.defaults(), bypass: false, sidechain: None, plugin: None, name: String::new(), color: None, icon: None }
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let out = args.first().ok_or("usage: stress <out.wav> [loops] [--limiter] [--resample] [--hot] [--dry]")?;
    let count = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(LOOPS.len()).min(LOOPS.len());
    let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/library");
    let samples: Vec<Arc<Sample>> = LOOPS[..count]
        .iter()
        .map(|(f, _)| {
            let mut s = Sample::decode_file(&library.join(f))?;
            if flag("--hot") {
                // mastered loops, as most bought ones are: peaks at full scale (left = mid + side)
                let peak = s.frames.iter().enumerate().fold(0.0f32, |m, (i, x)| m.max(x.abs() + s.side.get(i).map_or(0.0, |y| y.abs())));
                s = Sample::stereo(s.frames.iter().map(|x| x / peak).collect(), s.side.iter().map(|x| x / peak).collect(), s.rate);
            }
            Ok(Arc::new(s))
        })
        .collect::<Result<_, String>>()?;

    let mut p = Project { tempo: 128.0, channels: Vec::new(), ..Project::default() };
    p.mixer = vec![MixerTrack::new("Master")];
    let chain = [DeviceKind::Compressor, DeviceKind::Distortion, DeviceKind::Chorus, DeviceKind::Delay, DeviceKind::Reverb];
    for (i, (_, bpm)) in LOOPS[..count].iter().enumerate() {
        let mut t = MixerTrack::new(format!("Loop {i}"));
        let chain = if flag("--dry") { &chain[..0] } else { &chain[..] };
        t.devices = chain.iter().enumerate().map(|(k, kind)| device((i * 10 + k) as u32 + 1, *kind)).collect();
        // a light touch: some drive, the echoes and the room a third in
        for (d, mix) in [(1, 0.3), (3, 0.3), (4, 0.3)] {
            if let Some(d) = t.devices.get_mut(d) {
                d.params[d.kind.params().len() - 1] = mix;
            }
        }
        p.mixer.push(t);
        let fx = ClipFx { track: i + 1, stretch: if flag("--resample") { Stretch::Resample } else { Stretch::Stretch }, bpm: *bpm, looped: true, ..Default::default() };
        p.clips.push(Clip { lane: i as u32, start: 0, length: 8 * BAR, offset: 0, fx, kind: ClipKind::Audio { sample: i.to_string() } });
    }
    if flag("--limiter") {
        p.mixer[0].devices.push(device(1000, DeviceKind::Limiter));
    }

    let sr = 48_000.0;
    let transport = Transport { playing: true, mode: Mode::Song, start: 0, jump: 0, looped: None };
    let song = Song::compile(&p, transport, |name| name.parse::<usize>().ok().and_then(|i| samples.get(i).cloned()));
    let mut r = Renderer::new(sr);
    for msg in Effects::default().messages(&song, sr) {
        r.handle(msg);
    }
    r.handle(Msg::Song(Arc::new(song)));

    let block = 256;
    let seconds = 8.0 * 4.0 * 60.0 / 128.0;
    let blocks = (seconds * sr) as usize / block;
    let (mut l, mut rt) = (vec![0.0f32; block], vec![0.0f32; block]);
    let (mut wav, mut times) = (Vec::with_capacity(blocks * block * 2), Vec::with_capacity(blocks));
    for _ in 0..blocks {
        let t = Instant::now();
        // SAFETY: only flips the FPU's flush-to-zero flags, as the audio thread does
        unsafe { no_denormals::no_denormals(|| r.render(&mut l, &mut rt)) };
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        wav.extend(l.iter().zip(&rt).flat_map(|(a, b)| [*a, *b]));
    }
    write_wav(Path::new(out), &wav, 2, sr as u32).map_err(|e| e.to_string())?;

    let budget = block as f64 / sr as f64 * 1000.0;
    times.sort_by(f64::total_cmp);
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let peak = wav.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let clipped = wav.iter().filter(|x| x.abs() >= 0.9999).count();
    println!("{count} loops: block {block} = {budget:.2} ms budget; mean {mean:.3} ms, p99 {:.3} ms, worst {:.3} ms", times[times.len() * 99 / 100], times[times.len() - 1]);
    println!("peak {peak:.3}, samples at full scale {clipped} ({:.2}%)", 100.0 * clipped as f64 / wav.len() as f64);
    Ok(())
}
