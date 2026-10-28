//! End to end: Ravune's side of the bridge drives a real ravune-host process running the
//! test CLAP plugin (examples/testplug.rs, built by `cargo test`).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ravune_bridge::{Command, Event, Format, Hosted, Reply, decode, encode, kind, scan};

const RATE: f64 = 48_000.0;
const WAIT: Duration = Duration::from_secs(10);

fn host() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ravune-host"))
}

/// A test plugin built by cargo test (`example`), copied to a file named the way plugins ship.
fn built(example: &str, file: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap(); // target/<profile>/deps/host-….exe
    let dir = exe.parent().unwrap().parent().unwrap().join("examples");
    let name = if cfg!(windows) { format!("{example}.dll") } else if cfg!(target_os = "macos") { format!("lib{example}.dylib") } else { format!("lib{example}.so") };
    let built = dir.join(name);
    assert!(built.exists(), "{} isn't built", built.display());
    let copy = std::env::temp_dir().join(format!("{}-{file}", std::process::id()));
    std::fs::copy(&built, &copy).unwrap();
    copy
}

fn plugin() -> PathBuf {
    built("testplug", "test-tone.clap")
}

/// Renders `blocks` blocks of 256 frames (left channel).
fn render(hosted: &Hosted, blocks: usize) -> Vec<f32> {
    let mut out = Vec::new();
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    for _ in 0..blocks {
        assert!(hosted.bridge.process(None, (&mut l, &mut r), 120.0, 0.0, true, Duration::from_secs(2)), "the host answers");
        assert_eq!(l, r, "mono sine on both sides");
        out.extend_from_slice(&l);
    }
    out
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

fn hz(x: &[f32]) -> f64 {
    let crossings = x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
    crossings as f64 / 2.0 / (x.len() as f64 / RATE)
}

#[test]
fn a_clap_plugin_plays_takes_params_saves_and_dies_alone() {
    let path = plugin();
    let found = scan(&host(), &path, WAIT).expect("scan");
    assert_eq!(found.len(), 1);
    let tone = &found[0];
    assert_eq!((tone.id.as_str(), tone.name.as_str(), tone.vendor.as_str(), tone.instrument), ("com.ravune.test-tone", "Test Tone", "Ravune", true));

    let mut hosted = Hosted::spawn(&host(), Format::Clap, &path.to_string_lossy(), &tone.id, RATE, None, WAIT).expect("load");
    assert_eq!((hosted.name.as_str(), hosted.instrument), ("Test Tone", true));
    assert_eq!(peak(&render(&hosted, 4)), 0.0, "silent until a note");

    hosted.bridge.event(Event { kind: kind::NOTE_ON, key: 69, value: 1.0, ..Default::default() });
    let out = render(&hosted, 40);
    assert!((peak(&out) - 0.5).abs() < 0.01, "level 0.5, got {}", peak(&out));
    assert!((hz(&out) - 440.0).abs() < 5.0, "A4, got {} Hz", hz(&out));

    let Reply::Params { params } = hosted.ask(&Command::Params, WAIT).unwrap() else { panic!("params") };
    assert_eq!((params.len(), params[0].name.as_str(), params[0].value), (1, "Level", 0.5));
    assert_eq!(hosted.ask(&Command::SetParam { id: 1, value: 0.25 }, WAIT).unwrap(), Reply::Ok);
    render(&hosted, 2); // the change rides along with the next block
    assert!((peak(&render(&hosted, 10)) - 0.25).abs() < 0.01);

    let Reply::State { data } = hosted.ask(&Command::State, WAIT).unwrap() else { panic!("state") };
    assert_eq!(decode(&data).unwrap(), 0.25f64.to_le_bytes());
    let full = encode(&1.0f64.to_le_bytes());
    assert_eq!(hosted.ask(&Command::SetState { data: full }, WAIT).unwrap(), Reply::Ok);
    assert!((peak(&render(&hosted, 10)) - 1.0).abs() < 0.01, "state restored");

    hosted.bridge.event(Event { kind: kind::NOTE_OFF, key: 69, ..Default::default() });
    render(&hosted, 1);
    assert_eq!(peak(&render(&hosted, 2)), 0.0, "note off");

    // a saved state comes back when the plugin is loaded again
    let saved = encode(&0.75f64.to_le_bytes());
    let mut again = Hosted::spawn(&host(), Format::Clap, &path.to_string_lossy(), &tone.id, RATE, Some(saved), WAIT).expect("reload");
    let Reply::Params { params } = again.ask(&Command::Params, WAIT).unwrap() else { panic!("params") };
    assert_eq!(params[0].value, 0.75);

    // killing the process: blocks come back silent at once, nothing hangs
    hosted.kill();
    assert!(!hosted.running());
    let start = Instant::now();
    let (mut l, mut r) = (vec![1.0f32; 256], vec![1.0f32; 256]);
    assert!(!hosted.bridge.process(None, (&mut l, &mut r), 120.0, 0.0, true, Duration::from_secs(2)));
    assert!(start.elapsed() < Duration::from_millis(50) && peak(&l) == 0.0);
    assert!(hosted.ask(&Command::Params, WAIT).is_err(), "control fails cleanly too");
    drop(again);
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_vst3_effect_processes_takes_params_and_keeps_state() {
    let path = built("testgain", "test-gain.vst3");
    let found = scan(&host(), &path, WAIT).expect("scan");
    assert_eq!(found.len(), 1, "one audio class (the controller isn't listed)");
    let gain = &found[0];
    assert_eq!((gain.format, gain.name.as_str(), gain.instrument, gain.id.len()), (Format::Vst3, "Gain (vst3-rs example plugin)", false, 32));

    let mut hosted = Hosted::spawn(&host(), Format::Vst3, &path.to_string_lossy(), &gain.id, RATE, None, WAIT).expect("load");
    assert!(!hosted.instrument);
    let input: Vec<f32> = (0..256).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    assert!(hosted.bridge.process(Some((&input, &input)), (&mut l, &mut r), 120.0, 0.0, true, Duration::from_secs(2)));
    assert_eq!(l, input, "unity gain passes the input through");

    let Reply::Params { params } = hosted.ask(&Command::Params, WAIT).unwrap() else { panic!("params") };
    assert_eq!((params.len(), params[0].name.as_str(), params[0].value), (1, "Gain", 1.0));
    assert_eq!(hosted.ask(&Command::SetParam { id: 0, value: 0.5 }, WAIT).unwrap(), Reply::Ok);
    assert!(hosted.bridge.process(Some((&input, &input)), (&mut l, &mut r), 120.0, 0.0, true, Duration::from_secs(2)));
    assert!(l.iter().zip(&input).all(|(o, i)| (o - i * 0.5).abs() < 1e-6), "half gain");

    let Reply::State { data } = hosted.ask(&Command::State, WAIT).unwrap() else { panic!("state") };
    assert!(decode(&data).unwrap().starts_with(b"PHV3"));
    assert_eq!(hosted.ask(&Command::SetState { data }, WAIT).unwrap(), Reply::Ok);
    assert!(matches!(hosted.ask(&Command::Gui { show: true, title: "Gain".into() }, WAIT).unwrap(), Reply::Error { .. }), "no editor");
    drop(hosted);
    let _ = std::fs::remove_file(path);
}
