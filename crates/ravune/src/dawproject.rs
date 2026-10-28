//! DAWproject (M10): the open exchange format of Bitwig and PreSonus: a zip holding
//! `project.xml`, `metadata.xml` and the audio. Export writes the song as other DAWs see a
//! project: a track per channel with its notes unrolled out of the patterns, audio clips
//! with their files inside the zip, the mixer (volume, pan, mute, routing, sends). Import
//! brings note tracks back as VA-1 channels with a pattern per clip, and audio clips with
//! their files unpacked next to the library.
//!
//! ponytail: effects and instruments stay behind (they're Ravune's own or plugins), and an
//! imported clip longer than 32 bars is cut to a pattern's 32.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

use ravune_core::{Channel, Clip, ClipFx, ClipKind, Instrument, MAX_STEPS, Note, PPQ, Pattern, Project, STEP, sample_name};

/// What export needs to know about a sample: its bytes, a file name for inside the zip,
/// its length in seconds and its sample rate.
pub struct Audio {
    pub bytes: Vec<u8>,
    pub file: String,
    pub seconds: f64,
    pub rate: u32,
    pub channels: u16,
}

/// How many channels a WAV or FLAC file holds, from its header (2 when it can't tell).
pub fn channels_of(bytes: &[u8]) -> u16 {
    match bytes {
        [b'R', b'I', b'F', b'F', ..] if bytes.len() > 23 => u16::from_le_bytes([bytes[22], bytes[23]]).max(1),
        [b'f', b'L', b'a', b'C', ..] if bytes.len() > 20 => ((bytes[20] >> 1) & 7) as u16 + 1,
        _ => 2,
    }
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn beats(ticks: f64) -> String {
    format!("{:.6}", ticks / PPQ as f64)
}

fn hex(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}

/// The project as a `.dawproject` zip.
pub fn export(project: &Project, title: &str, mut audio: impl FnMut(&str) -> Option<Audio>) -> Result<Vec<u8>, String> {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Project version=\"1.0\">\n");
    xml += &format!("  <Application name=\"Ravune\" version=\"{}\"/>\n", env!("CARGO_PKG_VERSION"));
    xml += &format!("  <Transport>\n    <Tempo unit=\"bpm\" value=\"{}\" id=\"tempo\" name=\"Tempo\"/>\n    <TimeSignature numerator=\"4\" denominator=\"4\" id=\"signature\"/>\n  </Transport>\n", project.tempo);
    xml += "  <Structure>\n";
    // the mixer: the master, then every track as a bus channel
    let channel = |i: usize, role: &str, t: &ravune_core::MixerTrack| -> String {
        let destination = if i == 0 { String::new() } else { format!(" destination=\"mix{}\"", t.route) };
        let mut c = format!(
            "      <Channel audioChannels=\"2\" role=\"{role}\" solo=\"{}\" id=\"mix{i}\"{destination}>\n        <Mute value=\"{}\" id=\"mix{i}-mute\"/>\n        <Pan unit=\"normalized\" value=\"{:.4}\" id=\"mix{i}-pan\"/>\n        <Volume unit=\"linear\" value=\"{:.4}\" id=\"mix{i}-volume\"/>\n",
            t.solo,
            t.mute,
            (t.pan + 1.0) / 2.0,
            t.gain
        );
        if !t.sends.is_empty() {
            c += "        <Sends>\n";
            for (k, s) in t.sends.iter().enumerate() {
                c += &format!("          <Send destination=\"mix{}\" type=\"post\" id=\"mix{i}-send{k}\"><Volume unit=\"linear\" value=\"{:.4}\" id=\"mix{i}-send{k}-volume\"/></Send>\n", s.to, s.amount);
            }
            c += "        </Sends>\n";
        }
        c + "      </Channel>\n"
    };
    for (i, t) in project.mixer.iter().enumerate() {
        let role = if i == 0 { "master" } else if project.mixer.iter().any(|o| o.sends.iter().any(|s| s.to == i)) { "effect" } else { "regular" };
        let color = t.color.map_or(String::new(), |c| format!(" color=\"{}\"", hex(c)));
        xml += &format!("    <Track contentType=\"audio\" loaded=\"true\" id=\"bus{i}\" name=\"{}\"{color}>\n{}    </Track>\n", esc(&t.name), channel(i, role, t));
    }
    // a note track per channel, playing into its mixer track
    for c in &project.channels {
        xml += &format!(
            "    <Track contentType=\"notes\" loaded=\"true\" id=\"ch{}\" name=\"{}\">\n      <Channel audioChannels=\"2\" role=\"regular\" id=\"ch{}-mix\" destination=\"mix{}\">\n        <Mute value=\"{}\" id=\"ch{}-mute\"/>\n        <Volume unit=\"linear\" value=\"{:.4}\" id=\"ch{}-volume\"/>\n      </Channel>\n    </Track>\n",
            c.slot,
            esc(&c.name),
            c.slot,
            c.track,
            c.mute,
            c.slot,
            c.gain,
            c.slot
        );
    }
    // an audio track per lane that holds audio clips
    let audio_lanes: BTreeMap<u32, usize> = project.clips.iter().filter(|c| matches!(c.kind, ClipKind::Audio { .. })).map(|c| (c.lane, c.fx.track)).collect();
    for (lane, track) in &audio_lanes {
        let name = project.lanes.get(*lane as usize).map(|l| l.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Audio {}", lane + 1));
        xml += &format!("    <Track contentType=\"audio\" loaded=\"true\" id=\"lane{lane}\" name=\"{}\">\n      <Channel audioChannels=\"2\" role=\"regular\" id=\"lane{lane}-mix\" destination=\"mix{track}\"/>\n    </Track>\n", esc(&name));
    }
    xml += "  </Structure>\n  <Arrangement id=\"arrangement\">\n    <Lanes timeUnit=\"beats\" id=\"lanes\">\n";

    // notes: every pattern clip, unrolled per channel (a pattern loops when its clip is longer)
    let mut by_slot: BTreeMap<u8, String> = BTreeMap::new();
    for clip in &project.clips {
        let ClipKind::Pattern { pattern } = clip.kind else { continue };
        let Some(p) = project.patterns.get(pattern) else { continue };
        let span = p.length as u32 * STEP;
        for (slot, notes) in p.notes.iter().filter(|(_, n)| !n.is_empty()) {
            let mut list = String::new();
            let mut lap = 0;
            while lap * span < clip.offset + clip.length {
                for n in notes {
                    let at = (lap * span + n.start) as i64 - clip.offset as i64;
                    if at >= 0 && (at as u32) < clip.length {
                        let length = n.length.min(clip.length - at as u32);
                        list += &format!("            <Note time=\"{}\" duration=\"{}\" channel=\"0\" key=\"{}\" vel=\"{:.4}\" rel=\"0.5\"/>\n", beats(at as f64), beats(length as f64), n.key, n.velocity as f32 / 127.0);
                    }
                }
                lap += 1;
            }
            if !list.is_empty() {
                by_slot.entry(*slot).or_default().push_str(&format!(
                    "          <Clip time=\"{}\" duration=\"{}\" playStart=\"0\" name=\"{}\">\n            <Notes>\n{list}            </Notes>\n          </Clip>\n",
                    beats(clip.start as f64),
                    beats(clip.length as f64),
                    esc(&p.name)
                ));
            }
        }
    }
    for (slot, clips) in &by_slot {
        xml += &format!("      <Lanes track=\"ch{slot}\" id=\"ch{slot}-lanes\">\n        <Clips id=\"ch{slot}-clips\">\n{clips}        </Clips>\n      </Lanes>\n");
    }

    // audio: the files go in the zip once each
    let mut files: BTreeMap<String, Audio> = BTreeMap::new();
    let mut by_lane: BTreeMap<u32, String> = BTreeMap::new();
    for clip in &project.clips {
        let ClipKind::Audio { sample } = &clip.kind else { continue };
        if !files.contains_key(sample)
            && let Some(a) = audio(sample)
        {
            files.insert(sample.clone(), a);
        }
        let Some(a) = files.get(sample) else { continue };
        by_lane.entry(clip.lane).or_default().push_str(&format!(
            "          <Clip time=\"{}\" duration=\"{}\" playStart=\"{}\" name=\"{}\">\n            <Audio channels=\"{}\" duration=\"{:.6}\" sampleRate=\"{}\" id=\"audio-{}-{}\">\n              <File path=\"audio/{}\"/>\n            </Audio>\n          </Clip>\n",
            beats(clip.start as f64),
            beats(clip.length as f64),
            beats(clip.offset as f64),
            esc(sample_name(sample)),
            a.channels,
            a.seconds,
            a.rate,
            clip.lane,
            clip.start,
            esc(&a.file)
        ));
    }
    for (lane, clips) in &by_lane {
        xml += &format!("      <Lanes track=\"lane{lane}\" id=\"lane{lane}-lanes\">\n        <Clips id=\"lane{lane}-clips\">\n{clips}        </Clips>\n      </Lanes>\n");
    }
    xml += "    </Lanes>\n  </Arrangement>\n</Project>\n";
    let metadata = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<MetaData>\n  <Title>{}</Title>\n  <Comment>Made with Ravune</Comment>\n</MetaData>\n", esc(title));

    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, bytes: &[u8]| -> Result<(), String> {
        zip.start_file(name, options).map_err(|e| e.to_string())?;
        zip.write_all(bytes).map_err(|e| e.to_string())
    };
    put("project.xml", xml.as_bytes())?;
    put("metadata.xml", metadata.as_bytes())?;
    for a in files.values() {
        put(&format!("audio/{}", a.file), &a.bytes)?;
    }
    Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
}

/// A `.dawproject` as a Ravune project; its audio files are unpacked into `audio_dir`.
pub fn import(bytes: &[u8], audio_dir: &Path) -> Result<Project, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("not a DAWproject: {e}"))?;
    let mut xml = String::new();
    zip.by_name("project.xml").map_err(|_| "not a DAWproject: no project.xml".to_string())?.read_to_string(&mut xml).map_err(|e| e.to_string())?;
    let doc = roxmltree::Document::parse(&xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let num = |n: roxmltree::Node, attr: &str| n.attribute(attr).and_then(|v| v.parse::<f64>().ok());
    let ticks = |b: f64| (b * PPQ as f64).round().max(0.0) as u32;

    let mut p = Project { channels: Vec::new(), patterns: Vec::new(), clips: Vec::new(), ..Project::default() };
    if let Some(t) = root.descendants().find(|n| n.has_tag_name("Tempo")).and_then(|n| num(n, "value")) {
        p.tempo = (t as f32).clamp(20.0, 300.0);
    }
    // note tracks become VA-1 channels, their lanes in order
    let mut slots: BTreeMap<String, (u8, u32)> = BTreeMap::new();
    let mut audio_lanes: BTreeMap<String, u32> = BTreeMap::new();
    let mut lane = 0u32;
    for track in root.descendants().filter(|n| n.has_tag_name("Track")) {
        let (Some(id), kind) = (track.attribute("id"), track.attribute("contentType").unwrap_or("")) else { continue };
        let name = track.attribute("name").unwrap_or("Track").to_string();
        let has_clips = root.descendants().any(|n| n.has_tag_name("Lanes") && n.attribute("track") == Some(id) && n.descendants().any(|c| c.has_tag_name("Clip")));
        if !has_clips {
            continue;
        }
        if kind.contains("notes") {
            let slot = p.channels.len() as u8;
            let mut c = Channel::new(slot, name, Instrument::Synth { params: ravune_core::default_synth() });
            if let Some(v) = track.descendants().find(|n| n.has_tag_name("Volume")).and_then(|n| num(n, "value")) {
                c.gain = (v as f32).clamp(0.0, 1.0);
            }
            p.channels.push(c);
            slots.insert(id.to_string(), (slot, lane));
        } else {
            audio_lanes.insert(id.to_string(), lane);
        }
        lane += 1;
    }
    for lanes in root.descendants().filter(|n| n.has_tag_name("Lanes")) {
        let Some(track) = lanes.attribute("track") else { continue };
        for clip in lanes.descendants().filter(|n| n.has_tag_name("Clip")) {
            let (start, length) = (num(clip, "time").unwrap_or(0.0), num(clip, "duration").unwrap_or(4.0));
            if let Some(&(slot, lane)) = slots.get(track) {
                let notes: Vec<Note> = clip
                    .descendants()
                    .filter(|n| n.has_tag_name("Note"))
                    .map(|n| Note {
                        start: ticks(num(n, "time").unwrap_or(0.0)),
                        length: ticks(num(n, "duration").unwrap_or(0.25)).max(1),
                        key: num(n, "key").unwrap_or(60.0).clamp(0.0, 127.0) as u8,
                        velocity: (num(n, "vel").unwrap_or(0.8) * 127.0).round().clamp(1.0, 127.0) as u8,
                        ..Default::default()
                    })
                    .collect();
                // a pattern holds 32 bars at most: a longer clip comes in as pieces, and a piece
                // that repeats one before it (a loop) plays the same pattern
                let (total, chunk) = (ticks(length).max(1), MAX_STEPS as u32 * STEP);
                let base = clip.attribute("name").map_or_else(|| p.channels[slot as usize].name.clone(), String::from);
                let mut from = 0;
                while from < total {
                    let span = chunk.min(total - from);
                    let mut piece: Vec<Note> = notes.iter().filter(|n| (from..from + span).contains(&n.start)).map(|n| Note { start: n.start - from, length: n.length.min(from + span - n.start), ..*n }).collect();
                    piece.sort_by_key(|n| (n.start, n.key));
                    let steps = (span as usize).div_ceil(STEP as usize).clamp(1, MAX_STEPS);
                    let same = p.patterns.iter().position(|q| q.length == steps && q.notes.len() == 1 && q.notes.get(&slot) == Some(&piece));
                    let pattern = same.unwrap_or_else(|| {
                        let name = if total > chunk { format!("{base} {}", p.patterns.len() + 1) } else { base.clone() };
                        let mut q = Pattern { length: steps, ..Pattern::new(&name) };
                        *q.notes_mut(slot) = piece;
                        p.patterns.push(q);
                        p.patterns.len() - 1
                    });
                    p.clips.push(Clip { lane, start: ticks(start) + from, length: span, offset: 0, fx: ClipFx::default(), kind: ClipKind::Pattern { pattern } });
                    from += span;
                }
            } else if let Some(&lane) = audio_lanes.get(track)
                && let Some(file) = clip.descendants().find(|n| n.has_tag_name("File")).and_then(|n| n.attribute("path"))
            {
                let name = Path::new(file).file_name().map_or("audio.wav".into(), |n| n.to_string_lossy().into_owned());
                let out = audio_dir.join(&name);
                if !out.exists() {
                    let mut data = Vec::new();
                    zip.by_name(file).map_err(|e| format!("{file}: {e}"))?.read_to_end(&mut data).map_err(|e| e.to_string())?;
                    std::fs::create_dir_all(audio_dir).map_err(|e| e.to_string())?;
                    std::fs::write(&out, data).map_err(|e| e.to_string())?;
                }
                p.clips.push(Clip {
                    lane,
                    start: ticks(start),
                    length: ticks(length).max(1),
                    offset: ticks(num(clip, "playStart").unwrap_or(0.0)),
                    fx: ClipFx::default(),
                    kind: ClipKind::Audio { sample: out.to_string_lossy().into_owned() },
                });
            }
        }
    }
    if p.patterns.is_empty() {
        p.patterns.push(Pattern::new("Pattern 1"));
    }
    if p.clips.is_empty() {
        return Err("nothing to bring in: no note or audio clips".into());
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A song goes out and comes back with its tempo, its notes and its audio.
    #[test]
    fn a_song_round_trips() {
        let demo: Project = ron::from_str(crate::files::TEMPLATES[0].1).unwrap();
        let mut library = crate::browser::Library::default();
        let zip = export(&demo, "Techno", |path| crate::browser::sample_audio(&mut library, path)).unwrap();
        let dir = std::env::temp_dir().join("ravune-dawproject-test");
        let back = import(&zip, &dir).unwrap();
        assert_eq!(back.tempo, demo.tempo);
        // every note that plays in the song, counted both ways
        let played = |p: &Project| -> usize {
            p.clips
                .iter()
                .filter_map(|c| match c.kind {
                    ClipKind::Pattern { pattern } => {
                        let pat = &p.patterns[pattern];
                        let span = pat.length as u32 * STEP;
                        Some(pat.notes.values().flatten().map(|n| (0..).take_while(|k| k * span + n.start < c.offset + c.length).filter(|k| k * span + n.start >= c.offset).count()).sum::<usize>())
                    }
                    _ => None,
                })
                .sum()
        };
        assert_eq!(played(&back), played(&demo), "the same notes play");
        let audio = |p: &Project| p.clips.iter().filter(|c| matches!(c.kind, ClipKind::Audio { .. })).count();
        assert_eq!(audio(&back), audio(&demo));
        assert!(back.clips.iter().any(|c| matches!(&c.kind, ClipKind::Audio { sample } if Path::new(sample).exists())), "its files unpacked");
    }
}
