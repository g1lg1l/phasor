//! The M7 exit test: every CLAP / VST3 plugin installed in the standard folders is scanned,
//! loaded in its own host, played, given a parameter change, saved and loaded again from that
//! state. Depends on what's installed, so it only runs when asked:
//! `cargo test -p ravune-host --release --test plugins -- --ignored --nocapture`
//! (`RAVUNE_PLUGIN_FILTER=surge` keeps the plugins whose name contains "surge").

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ravune_bridge::{Command, Event, Found, Hosted, ParamInfo, Reply, kind, scan};

const RATE: f64 = 48_000.0;
const WAIT: Duration = Duration::from_secs(60);
const BLOCK: usize = 256;

fn host() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ravune-host"))
}

/// Plugin files under the user and system CLAP / VST3 folders (a .vst3 bundle is a folder).
fn plugin_files() -> Vec<PathBuf> {
    let bases = [std::env::var("LOCALAPPDATA").map(|d| Path::new(&d).join("Programs/Common")), std::env::var("CommonProgramFiles").map(PathBuf::from)];
    let mut stack: Vec<PathBuf> = bases.into_iter().flatten().flat_map(|b| [b.join("CLAP"), b.join("VST3")]).collect();
    let mut found = Vec::new();
    while let Some(dir) = stack.pop() {
        for path in std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()) {
            if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("clap") || e.eq_ignore_ascii_case("vst3")) {
                found.push(path);
            } else if path.is_dir() {
                stack.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Runs `blocks` blocks (noise in for effects) and returns the output peak; non-finite output is an error.
fn render(hosted: &Hosted, blocks: usize, input: bool, seed: &mut u32) -> Result<f32, String> {
    let (mut l, mut r, mut noise) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
    let mut peak = 0.0f32;
    for i in 0..blocks {
        for s in &mut noise {
            *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *s = (*seed >> 8) as f32 / (1 << 24) as f32 * 0.5 - 0.25;
        }
        let beats = i as f64 * BLOCK as f64 / RATE * 2.0;
        let input = input.then_some((&noise[..], &noise[..]));
        if !hosted.bridge.process(input, (&mut l, &mut r), 120.0, beats, true, Duration::from_secs(if i == 0 { 20 } else { 2 })) {
            return Err(format!("no audio back at block {i}"));
        }
        if l.iter().chain(&r).any(|s| !s.is_finite()) {
            return Err(format!("NaN / inf out at block {i}"));
        }
        peak = l.iter().chain(&r).fold(peak, |m, s| m.max(s.abs()));
    }
    Ok(peak)
}

/// Note in for instruments, noise for effects; returns the peak.
fn play(hosted: &Hosted, blocks: usize, seed: &mut u32) -> Result<f32, String> {
    if !hosted.instrument {
        return render(hosted, blocks, true, seed);
    }
    hosted.bridge.event(Event { kind: kind::NOTE_ON, key: 60, value: 0.8, ..Default::default() });
    let peak = render(hosted, blocks, false, seed)?;
    hosted.bridge.event(Event { kind: kind::NOTE_OFF, key: 60, ..Default::default() });
    render(hosted, 8, false, seed)?;
    Ok(peak)
}

fn params(hosted: &mut Hosted) -> Result<Vec<ParamInfo>, String> {
    match hosted.ask(&Command::Params, WAIT)? {
        Reply::Params { params } => Ok(params),
        other => Err(format!("params: {other:?}")),
    }
}

struct Row {
    load: Duration,
    peak: f32,
    params: usize,
    state: usize,
    moved: String,
}

fn state(hosted: &mut Hosted) -> Result<String, String> {
    match hosted.ask(&Command::State, WAIT)? {
        Reply::State { data } => Ok(data),
        other => Err(format!("state: {other:?}")),
    }
}

/// A moved parameter: id, name, the value it took.
type Moved = Option<(u32, String, f64)>;

/// Moves a continuous parameter that the plugin keeps in its state (some are UI switches it
/// doesn't save); returns it and the state with it.
fn move_param(hosted: &mut Hosted, list: &[ParamInfo], seed: &mut u32) -> Result<(Moved, String), String> {
    let mut before = state(hosted)?;
    for p in list.iter().filter(|p| !p.stepped && p.max > p.min).take(8) {
        let at = (p.value - p.min) / (p.max - p.min);
        let want = p.min + (p.max - p.min) * if at < 0.5 { 0.8 } else { 0.2 };
        if !matches!(hosted.ask(&Command::SetParam { id: p.id, value: want }, WAIT)?, Reply::Ok) {
            continue;
        }
        render(hosted, 4, !hosted.instrument, seed)?; // the change rides along with the next blocks
        let now = params(hosted)?.into_iter().find(|q| q.id == p.id).map_or(p.value, |q| q.value);
        let after = state(hosted)?;
        if (now - p.value).abs() > (p.max - p.min) * 0.05 && after != before {
            return Ok((Some((p.id, p.name.clone(), now)), after));
        }
        before = after;
    }
    Ok((None, before))
}

fn check(found: &Found) -> Result<Row, String> {
    let mut seed = 1;
    let start = Instant::now();
    let mut hosted = Hosted::spawn(&host(), found.format, &found.path, &found.id, RATE, None, WAIT)?;
    let load = start.elapsed();
    let peak = play(&hosted, 150, &mut seed)?;
    let list = params(&mut hosted)?;
    let (moved, data) = move_param(&mut hosted, &list, &mut seed)?;
    let bytes = ravune_bridge::decode(&data)?.len();
    drop(hosted);

    let mut again = Hosted::spawn(&host(), found.format, &found.path, &found.id, RATE, Some(data.clone()), WAIT).map_err(|e| format!("reload: {e}"))?;
    play(&again, 40, &mut seed).map_err(|e| format!("after reload: {e}"))?;
    // saved again byte for byte is proof enough; else the moved parameter must be back
    let same = state(&mut again)? == data;
    let moved = match moved {
        Some((id, name, value)) => {
            let p = params(&mut again)?.into_iter().find(|p| p.id == id).ok_or(format!("{name} is gone after reload"))?;
            // a few % of slack: some plugins snap a continuous-looking value to steps on load
            if !same && (p.value - value).abs() > (p.max - p.min) * 0.05 {
                return Err(format!("{name} saved as {value:.3}, came back {:.3}", p.value));
            }
            name
        }
        None => "-".into(),
    };
    let moved = if same { format!("{moved}, state identical") } else { moved };
    Ok(Row { load, peak, params: list.len(), state: bytes, moved })
}

#[test]
#[ignore = "needs installed plugins; run with --ignored"]
fn installed_plugins_load_play_and_keep_their_state() {
    let filter = std::env::var("RAVUNE_PLUGIN_FILTER").unwrap_or_default().to_lowercase();
    let (mut passed, mut failed) = (BTreeSet::new(), Vec::new());
    println!("{:<34} {:<5} {:<5} {:>7} {:>6} {:>6} {:>8}  param moved and restored", "plugin", "fmt", "kind", "load ms", "peak", "params", "state B");
    for file in plugin_files() {
        let found = match scan(&host(), &file, WAIT) {
            Ok(found) => found,
            Err(e) => {
                println!("{:<34} scan failed: {e}", file.file_name().unwrap().to_string_lossy());
                failed.push(file.display().to_string());
                continue;
            }
        };
        for plugin in found.iter().filter(|p| p.name.to_lowercase().contains(&filter)) {
            let what = if plugin.instrument { "inst" } else { "fx" };
            if plugin.vendor.starts_with("Elektron") {
                println!("{:<34} {:<5} {:<5} skipped: Overbridge streams from the hardware, none is connected", plugin.name, plugin.format.name(), what);
                continue;
            }
            match check(plugin) {
                Ok(row) => {
                    let silent = if plugin.instrument && row.peak < 1e-4 { "  (silent after a note)" } else { "" };
                    println!("{:<34} {:<5} {:<5} {:>7} {:>6.3} {:>6} {:>8}  {}{silent}", plugin.name, plugin.format.name(), what, row.load.as_millis(), row.peak, row.params, row.state, row.moved);
                    passed.insert(plugin.name.clone());
                }
                Err(e) => {
                    println!("{:<34} {:<5} {:<5} FAILED: {e}", plugin.name, plugin.format.name(), what);
                    failed.push(format!("{} ({})", plugin.name, plugin.format.name()));
                }
            }
        }
    }
    println!("\n{} distinct plugins passed; failed: {failed:?}", passed.len());
    assert!(failed.is_empty(), "failed: {failed:?}");
    assert!(filter.is_empty() || !passed.is_empty(), "nothing matched the filter");
    assert!(!filter.is_empty() || passed.len() >= 20, "the exit test wants 20 plugins, {} passed", passed.len());
}
