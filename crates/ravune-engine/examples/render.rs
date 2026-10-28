//! Renders a project to WAV without the app, and prints its loudness per 8 bars:
//!
//!     cargo run -p ravune-engine --example render -- song.ravune out.wav [--mute 2,7] [--solo 1]
//!
//! `--mute` / `--solo` take mixer track numbers (0 = master), for balancing a mix.

use std::path::Path;
use std::sync::Arc;

use ravune_core::{BAR, Project};
use ravune_engine::{Mode, Sample, Song, Transport, render_offline, write_wav};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [song, out, ..] = args.as_slice() else {
        return Err("usage: render <song.ravune> <out.wav> [--mute 2,7] [--solo 1]".into());
    };
    let list = |flag: &str| -> Vec<usize> {
        let at = args.iter().position(|a| a == flag);
        at.and_then(|i| args.get(i + 1)).map_or(Vec::new(), |v| v.split(',').filter_map(|n| n.parse().ok()).collect())
    };
    let mut project: Project = ron::from_str(&std::fs::read_to_string(song).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let (mute, solo) = (list("--mute"), list("--solo"));
    for (i, track) in project.mixer.iter_mut().enumerate().skip(1) {
        track.mute |= mute.contains(&i) || (!solo.is_empty() && !solo.contains(&i));
    }
    // built-in sounds come straight from assets/library
    let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/library");
    let load = |path: &str| {
        let file = path.strip_prefix("builtin:").map_or(Path::new(path).to_path_buf(), |name| library.join(name));
        Sample::decode_file(&file).ok().map(Arc::new)
    };
    let rate = 44_100;
    let compiled = Song::compile(&project, Transport { playing: true, mode: Mode::Song, start: 0, jump: 0, looped: None }, load);
    let frames = render_offline(compiled, rate as f32, 2.0, |_| {});
    write_wav(Path::new(out), &frames, 2, rate).map_err(|e| e.to_string())?;

    let bar = (BAR as f64 * 60.0 / (project.tempo as f64 * ravune_core::PPQ as f64) * rate as f64) as usize * 2;
    let peak = frames.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    println!("{out}: {:.1} s, peak {:.2}", frames.len() as f64 / 2.0 / rate as f64, peak);
    for (k, chunk) in frames.chunks(bar * 8).enumerate() {
        let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt();
        println!("bars {:>3}-{:<3} rms {:>6.1} dB", k * 8, k * 8 + 8, 20.0 * rms.max(1e-9).log10());
    }
    Ok(())
}
