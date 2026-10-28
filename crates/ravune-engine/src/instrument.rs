//! Multi-sample instruments: SFZ and Decent Sampler (.dspreset) files read into key and
//! velocity regions. Parsed on the UI thread when the song compiles; samples come from the
//! caller's cache, so the audio thread never frees them.
//!
//! SFZ: `<control> <global> <master> <group> <region>` with `#define` and `#include`;
//! sample, key ranges, velocity ranges, pitch, volume, release, offsets, loops and round
//! robins. Decent Sampler: `<groups> <group> <sample>` attributes, inherited downwards.
//! ponytail: release triggers, crossfades, filters and per-region envelopes beyond release
//! are skipped; add them when a library needs them.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::Sample;

#[derive(Clone)]
pub struct Region {
    pub sample: Arc<Sample>,
    pub lokey: u8,
    pub hikey: u8,
    pub lovel: u8,
    pub hivel: u8,
    /// The key that plays the sample at its own pitch (tuning folded in).
    pub root: f32,
    pub gain: f32,
    /// Seconds to fade after note-off.
    pub release: f32,
    /// Plays to the end whatever the note-off.
    pub one_shot: bool,
    /// Loop points in frames.
    pub looped: Option<(usize, usize)>,
    pub offset: usize,
    /// Round robin: (cycle length, this region's turn), 1-based.
    pub seq: (u32, u32),
}

pub struct MultiMap {
    pub regions: Vec<Region>,
}

impl MultiMap {
    /// Reads an .sfz or .dspreset file; `sample` loads (and keeps) the audio files.
    pub fn load(path: &Path, sample: &mut dyn FnMut(&str) -> Option<Arc<Sample>>) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        let specs = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("sfz") => sfz(&text, dir),
            Some("dspreset") => dspreset(&text),
            _ => return Err(format!("{}: not an .sfz or .dspreset file", path.display())),
        };
        let mut regions = Vec::new();
        let mut missing = 0;
        for spec in specs {
            let file = dir.join(spec.sample.replace('\\', "/"));
            match sample(&file.to_string_lossy()) {
                Some(audio) => regions.push(spec.build(audio)),
                None => missing += 1,
            }
        }
        if regions.is_empty() {
            return Err(format!("{}: no playable regions ({missing} samples missing)", path.display()));
        }
        Ok(Self { regions })
    }

    /// The regions that sound for `key` at `velocity` on round-robin `turn` (0-based).
    pub fn regions(&self, key: u8, velocity: u8, turn: u32) -> impl Iterator<Item = &Region> {
        self.regions.iter().filter(move |r| {
            (r.lokey..=r.hikey).contains(&key) && (r.lovel..=r.hivel).contains(&velocity) && turn % r.seq.0.max(1) + 1 == r.seq.1.clamp(1, r.seq.0.max(1))
        })
    }

    /// Lowest and highest key any region covers.
    pub fn range(&self) -> (u8, u8) {
        let lo = self.regions.iter().map(|r| r.lokey).min().unwrap_or(0);
        let hi = self.regions.iter().map(|r| r.hikey).max().unwrap_or(127);
        (lo, hi)
    }
}

/// One region as written, before its sample is loaded.
#[derive(Clone, Debug, PartialEq)]
struct Spec {
    sample: String,
    lokey: u8,
    hikey: u8,
    lovel: u8,
    hivel: u8,
    root: f32,
    volume_db: f32,
    release: f32,
    one_shot: bool,
    looped: Option<(usize, usize)>,
    offset: usize,
    seq: (u32, u32),
}

impl Spec {
    fn build(self, sample: Arc<Sample>) -> Region {
        let len = sample.frames.len();
        let looped = self.looped.filter(|&(a, b)| a < b && b <= len);
        Region {
            sample,
            lokey: self.lokey,
            hikey: self.hikey,
            lovel: self.lovel,
            hivel: self.hivel,
            root: self.root,
            gain: 10f32.powf(self.volume_db / 20.0),
            release: self.release,
            one_shot: self.one_shot,
            looped,
            offset: self.offset.min(len.saturating_sub(1)),
            seq: self.seq,
        }
    }
}

/// "60", "c4", "C#4", "db3" → MIDI key (SFZ: c4 = 60).
fn key(value: &str) -> Option<u8> {
    let v = value.trim();
    if let Ok(n) = v.parse::<i32>() {
        return Some(n.clamp(0, 127) as u8);
    }
    let lower = v.to_ascii_lowercase();
    let mut chars = lower.chars();
    let base = match chars.next()? {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest: String = chars.collect();
    let (shift, octave) = match rest.chars().next()? {
        '#' => (1, &rest[1..]),
        'b' if rest.len() > 1 => (-1, &rest[1..]),
        _ => (0, rest.as_str()),
    };
    let octave: i32 = octave.parse().ok()?;
    Some(((octave + 1) * 12 + base + shift).clamp(0, 127) as u8)
}

// ---------------------------------------------------------------------------
// SFZ
// ---------------------------------------------------------------------------

/// Comments out, `#include`s inlined (relative to the main file, as the spec says),
/// `#define`s applied.
fn preprocess(text: &str, dir: &Path, defines: &mut Vec<(String, String)>, depth: usize, out: &mut String) {
    let mut rest = text;
    while !rest.is_empty() {
        let (line, next) = rest.split_once('\n').unwrap_or((rest, ""));
        rest = next;
        let line = line.split("//").next().unwrap_or("");
        let trimmed = line.trim();
        if let Some(def) = trimmed.strip_prefix("#define") {
            let mut parts = def.split_whitespace();
            if let (Some(name), Some(value)) = (parts.next(), parts.next()) {
                defines.retain(|(n, _)| n != name);
                defines.push((name.to_string(), value.to_string()));
                defines.sort_by_key(|(n, _)| std::cmp::Reverse(n.len())); // $LONG before $LO
            }
        } else if let Some(file) = trimmed.strip_prefix("#include") {
            let file = file.trim().trim_matches('"');
            if depth < 8
                && let Ok(text) = std::fs::read_to_string(dir.join(file.replace('\\', "/")))
            {
                preprocess(&text, dir, defines, depth + 1, out);
            }
        } else {
            let mut line = line.to_string();
            for (name, value) in defines.iter() {
                line = line.replace(name.as_str(), value);
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
}

/// Headers and `opcode=value` pairs in order. A value runs to the next opcode or header,
/// so sample paths may contain spaces.
fn tokens(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut rest = line.trim();
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('<') {
                let (name, tail) = after.split_once('>').unwrap_or((after, ""));
                out.push((format!("<{}>", name.trim()), String::new()));
                rest = tail.trim_start();
                continue;
            }
            let Some(eq) = rest.find('=') else { break };
            let name = rest[..eq].trim().to_string();
            let tail = &rest[eq + 1..];
            // the value ends where the next "name=" or "<header>" begins
            let mut end = tail.len();
            for (i, c) in tail.char_indices() {
                if c == '<' {
                    end = i;
                    break;
                }
                if c.is_whitespace() {
                    let word = tail[i..].trim_start();
                    let ident = word.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(word.len());
                    if ident > 0 && word[ident..].starts_with('=') {
                        end = i;
                        break;
                    }
                }
            }
            out.push((name, tail[..end].trim().to_string()));
            rest = tail[end..].trim_start();
        }
    }
    out
}

fn sfz(text: &str, dir: &Path) -> Vec<Spec> {
    let mut clean = String::new();
    preprocess(text, dir, &mut Vec::new(), 0, &mut clean);
    // opcodes per header level; a region sees control < global < master < group < its own
    const LEVELS: [&str; 5] = ["<control>", "<global>", "<master>", "<group>", "<region>"];
    let mut scopes: [HashMap<String, String>; 5] = Default::default();
    let mut level = None;
    let mut specs = Vec::new();
    let finish = |scopes: &[HashMap<String, String>; 5], level: Option<usize>, specs: &mut Vec<Spec>| {
        if level == Some(4) {
            let mut ops = HashMap::new();
            for s in scopes.iter() {
                ops.extend(s.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            if let Some(spec) = sfz_region(&ops) {
                specs.push(spec);
            }
        }
    };
    for (name, value) in tokens(&clean) {
        if let Some(l) = LEVELS.iter().position(|h| *h == name) {
            finish(&scopes, level, &mut specs);
            for s in &mut scopes[l..] {
                s.clear(); // a new header resets itself and everything below it
            }
            level = Some(l);
        } else if name.starts_with('<') {
            finish(&scopes, level, &mut specs);
            level = None; // <curve>, <effect>, <midi>…: not ours
        } else if let Some(l) = level {
            scopes[l].insert(name.to_ascii_lowercase(), value);
        }
    }
    finish(&scopes, level, &mut specs);
    specs
}

fn sfz_region(ops: &HashMap<String, String>) -> Option<Spec> {
    let get = |k: &str| ops.get(k).map(String::as_str);
    let num = |k: &str, default: f32| get(k).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(default);
    if get("trigger").is_some_and(|t| t != "attack" && t != "first" && t != "legato") {
        return None; // release triggers
    }
    let sample = format!("{}{}", get("default_path").unwrap_or(""), get("sample")?);
    if sample.starts_with('*') {
        return None; // *sine, *noise: built-in generators
    }
    let single = get("key").and_then(key);
    let lokey = get("lokey").and_then(key).or(single).unwrap_or(0);
    let hikey = get("hikey").and_then(key).or(single).unwrap_or(127);
    let center = get("pitch_keycenter").and_then(key).or(single).unwrap_or(60) as f32;
    let tune = num("tune", 0.0) / 100.0 + num("transpose", 0.0);
    let mode = get("loop_mode").or(get("loopmode")).unwrap_or("");
    let (start, end) = (get("loop_start").or(get("loopstart")), get("loop_end").or(get("loopend")));
    let looped = match (mode, start, end) {
        ("loop_continuous" | "loop_sustain", Some(a), Some(b)) => Some((a.parse().ok()?, b.parse::<usize>().ok()? + 1)),
        _ => None,
    };
    Some(Spec {
        sample,
        lokey,
        hikey,
        lovel: num("lovel", 1.0) as u8,
        hivel: num("hivel", 127.0) as u8,
        // the sample's own pitch sits at the key center; tuning moves it the other way
        root: center - tune,
        volume_db: num("volume", 0.0) + num("amplitude", 100.0).max(0.01).log10() * 20.0 - 40.0,
        release: num("ampeg_release", 0.001),
        one_shot: mode == "one_shot",
        looped,
        offset: num("offset", 0.0) as usize,
        seq: (num("seq_length", 1.0) as u32, num("seq_position", 1.0) as u32),
    })
}

// ---------------------------------------------------------------------------
// Decent Sampler
// ---------------------------------------------------------------------------

/// The tags in an XML document: (name, attributes), closing tags as "/name".
fn xml_tags(text: &str) -> Vec<(String, HashMap<String, String>)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        rest = &rest[open + 1..];
        if let Some(body) = rest.strip_prefix("!--") {
            rest = body.split_once("-->").map_or("", |(_, r)| r);
            continue;
        }
        let Some(close) = rest.find('>') else { break };
        let tag = rest[..close].trim_end_matches('/').trim();
        rest = &rest[close + 1..];
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        let name_end = tag.find(char::is_whitespace).unwrap_or(tag.len());
        let mut attrs = HashMap::new();
        let mut a = &tag[name_end..];
        while let Some(eq) = a.find('=') {
            let key = a[..eq].trim().to_string();
            let v = a[eq + 1..].trim_start();
            let Some(quote) = v.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
            let Some(end) = v[1..].find(quote) else { break };
            attrs.insert(key, xml_unescape(&v[1..1 + end]));
            a = &v[end + 2..];
        }
        out.push((tag[..name_end].to_string(), attrs));
    }
    out
}

fn xml_unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// "-3dB", "-3 dB" or a linear "0.7" → dB.
fn ds_volume(v: &str) -> f32 {
    let v = v.trim();
    match v.strip_suffix("dB").or_else(|| v.strip_suffix("db")) {
        Some(db) => db.trim().parse().unwrap_or(0.0),
        None => v.parse::<f32>().map_or(0.0, |x| 20.0 * x.max(1e-4).log10()),
    }
}

fn dspreset(text: &str) -> Vec<Spec> {
    let mut stack: Vec<HashMap<String, String>> = Vec::new();
    let mut specs = Vec::new();
    for (name, attrs) in xml_tags(text) {
        match name.as_str() {
            "groups" | "group" => stack.push(attrs),
            "/groups" | "/group" => {
                stack.pop();
            }
            "sample" if !stack.is_empty() => {
                let mut ops: HashMap<String, String> = HashMap::new();
                // volumes add up down the tree; everything else is inherited
                let mut volume = 0.0;
                for scope in stack.iter().chain(std::iter::once(&attrs)) {
                    volume += scope.get("volume").map_or(0.0, |v| ds_volume(v));
                    ops.extend(scope.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
                let get = |k: &str| ops.get(k).map(String::as_str);
                let num = |k: &str, default: f32| get(k).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(default);
                let Some(path) = get("path") else { continue };
                if get("trigger").is_some_and(|t| t == "release") {
                    continue;
                }
                let root = get("rootNote").and_then(key).unwrap_or(60) as f32;
                let looped = match (get("loopEnabled"), get("loopStart"), get("loopEnd")) {
                    (Some("true"), Some(a), Some(b)) => a.parse().ok().zip(b.parse::<usize>().ok().map(|b| b + 1)),
                    _ => None,
                };
                specs.push(Spec {
                    sample: path.to_string(),
                    lokey: get("loNote").and_then(key).unwrap_or(0),
                    hikey: get("hiNote").and_then(key).unwrap_or(127),
                    lovel: num("loVel", 1.0) as u8,
                    hivel: num("hiVel", 127.0) as u8,
                    root: root - num("tuning", 0.0),
                    volume_db: volume,
                    release: num("release", 0.1),
                    one_shot: false,
                    looped,
                    offset: num("start", 0.0) as usize,
                    seq: (num("seqLength", 1.0) as u32, num("seqPosition", 1.0) as u32),
                });
            }
            _ => {}
        }
    }
    specs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_tokens() {
        assert_eq!(key("c4"), Some(60));
        assert_eq!(key("C#4"), Some(61));
        assert_eq!(key("db4"), Some(61));
        assert_eq!(key("a-1"), Some(9));
        assert_eq!(key("72"), Some(72));
        let t = tokens("<region> sample=My Piano/C4 soft.wav lokey=c4 hikey=e4 <region>sample=x.wav");
        assert_eq!(t[1], ("sample".into(), "My Piano/C4 soft.wav".into()));
        assert_eq!(t[2], ("lokey".into(), "c4".into()));
        assert_eq!(t[5], ("sample".into(), "x.wav".into()));
    }

    #[test]
    fn sfz_inherits_and_defines() {
        let text = "// piano\n#define $VEL 64\n<control> default_path=Samples/\n<global> ampeg_release=0.5\n<group> hivel=$VEL\n<region> sample=a.wav key=60\n<region> sample=b.wav lokey=61 hikey=72 pitch_keycenter=66 tune=-50\n<group> lovel=65 seq_length=2\n<region> sample=c.wav seq_position=2 trigger=attack\n<region> sample=r.wav trigger=release";
        let specs = sfz(text, Path::new("."));
        assert_eq!(specs.len(), 3, "the release-trigger region is skipped");
        assert_eq!(specs[0].sample, "Samples/a.wav");
        assert_eq!((specs[0].lokey, specs[0].hikey, specs[0].root), (60, 60, 60.0));
        assert_eq!((specs[0].hivel, specs[0].release), (64, 0.5));
        assert_eq!((specs[1].lokey, specs[1].hikey, specs[1].root), (61, 72, 66.5));
        assert_eq!((specs[2].lovel, specs[2].hivel, specs[2].seq), (65, 127, (2, 2)), "a new <group> forgets the old one");
    }

    #[test]
    fn decent_sampler_inherits_down_the_tree() {
        let text = r#"<?xml version="1.0"?><DecentSampler><groups release="0.8" volume="-6dB"><!-- soft --><group loVel="1" hiVel="80" volume="-3dB">
            <sample path="Samples/C3.wav" rootNote="48" loNote="40" hiNote="52"/><sample path="Samples/C4.wav" rootNote="60" loNote="53" hiNote="64" release="0.2" loopEnabled="true" loopStart="100" loopEnd="999"/>
            </group><group><sample path="Samples/hard.wav" rootNote="60" trigger="release"/></group></groups></DecentSampler>"#;
        let specs = dspreset(text);
        assert_eq!(specs.len(), 2);
        assert_eq!((specs[0].lokey, specs[0].hikey, specs[0].root, specs[0].hivel), (40, 52, 48.0, 80));
        assert_eq!((specs[0].release, specs[0].volume_db), (0.8, -9.0));
        assert_eq!((specs[1].release, specs[1].looped), (0.2, Some((100, 1000))));
    }
}
