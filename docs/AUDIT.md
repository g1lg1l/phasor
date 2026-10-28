# Ravune: feature audit

> 2026-10-26. What the plan ([PLAN.md](PLAN.md)) promises that the app doesn't do yet, by milestone,
> with what was done about it. ✅ done · 🟡 partly · ⬜ not yet.

## Deferred from the closed milestones (M2–M7)

| Feature | Plan | Status |
|---|---|---|
| Per-channel swing | M2 | ✅ right-click a channel → Swing: the song's, straight, or its own amount |
| Sampler envelope (attack / release) | M2 | ✅ Attack fades each hit in; Release holds a hit for its note, then fades |
| Starter templates | M2 | ✅ File → New from Template: Techno, House, Drum & Bass, Liquid DnB, Hardgroove, IDM, Hip-hop |
| Automation clips, loop region | M3 | ✅ |
| Markers | M3 | ✅ chips on the ruler: click jumps, drag moves, double-click renames |
| Clip aliases, multiple arrangements | M3 | 🟡 every pattern clip is an alias of its pattern until Make unique; one arrangement per project |
| Song-structure helper | M3 | ✅ Create → Song from the patterns (intro … outro, with markers) |
| Piano roll: multi-select, chord stamp, strum, humanize, quantize, legato | M3 | ✅ |
| Piano roll: arpeggiator, randomize | M3 | ✅ as built-in scripts (Arpeggiate chords, Random bassline in key, Euclid, Reverse, Half / Double time) |
| Tempo-sync stretch of audio clips | M3 | ✅ (WSOLA) |
| FLAC export | M3 | ✅ (plus WAV 16/24/32-bit float, Ogg, MP3 through ffmpeg) |
| Stem export | M3 | ✅ File → Export → Stems (a file per mixer track, with its returns) |
| Recording what plays | none | ✅ (record button → a new clip) |
| Routing cables view, track colors and icons | M4 | ✅ |
| Mixer solo | M4 | ✅ S on every strip (keeps the buses, returns and inputs of what is soloed) |
| Strip EQ | M4 | ✅ low, mid and high on every strip (EQ Three's bands, after the effects, before the fader); not yet automatable or modulated |
| 16 macros, variations, Rand | M4 | ✅ Rand rolls every mapped macro; up to 8 stored variations, recalled with a click (8 macros, not 16) |
| Envelope / random / keytrack / velocity modulators, per-voice | M4 | 🟡 LFOs have a Random shape (sample and hold); velocity and brightness per note in the piano roll; no per-voice envelope or keytrack sources |
| Drag-to-assign modulation | M4 | ✅ Alt+drag a macro, or an LFO by its handle, onto any knob or fader (and the right-click menu) |
| Automation recording | M4 | ✅ arm it next to the record button: knobs turned while the song plays write into automation clips (one undo step per drag) |
| Plugin delay compensation, limiter look-ahead | M4 | 🟡 the limiter looks 1.5 ms ahead (its track is that much later; on the master it doesn't matter); no plugin delay compensation |
| Stereo samples | M4 | ✅ stereo files keep their sides (in the sampler, audio clips and the stretcher); mono stays mono |
| Audio clips into their own mixer track | M4 | ✅ |
| Lua controller scripts | M7 | 🟡 → M8: Luau scripts drive operators; MIDI controller scripts not yet |

## M8: "Talk To It"

| Feature | Status |
|---|---|
| Built-in MCP server: search_operators, run_operator, query_project, browse_sounds, render_and_analyze | ✅ `ravune-mcp` (stdio) ↔ Ravune (localhost, token); also checkpoint, undo, save_project |
| One labeled undo group per agent session, visible command log, permission tiers, off by default | ✅ Preferences → Agents; exit test: an agent builds a 4-bar beat + bassline, one Ctrl+Z reverts it |
| Lua API (operators + project queries), piano-roll scripts, MIDI generators | ✅ Luau, sandboxed, 2 s limit, one undo step per run; Piano Roll → Tools → Scripts and the palette |

## M9: "Smart Helpers"

| Feature | Status |
|---|---|
| Stem separation (Demucs ONNX) | 🟡 without a model: "Split into drums and tones" (harmonic / percussive separation); four stems need Demucs |
| Audio → MIDI (Basic Pitch ONNX) | 🟡 without a model: "Notes from this sound" (YIN, one voice); chords need Basic Pitch |
| Neural device host | ⬜ |
| "Explain this" assistant, loop → song suggestions | 🟡 the assistant is any MCP client (M8), and a chat in Ravune (Ctrl I, full height on the right) that runs the user's own Claude Code with only Ravune's commands; loop → song is "Song from the patterns" |

## M10: "Hello World"

| Feature | Status |
|---|---|
| Interactive lessons | ✅ Help → Lessons: four lessons that check the project and move on by themselves |
| Templates, more sounds | ✅ seven templates; 138 CC0 sounds in seven genre packs (Drum & Bass, Liquid DnB, Techno, Hardgroove, IDM, House, Electronic) |
| The whole Manual (a chapter per editor and workflow) | ✅ eleven chapters (Help → Manual), in English and Italian |
| DAWproject import / export | ✅ notes (unrolled from patterns), audio in the zip, the mixer; import: note tracks → VA-1 channels, audio unpacked |
| macOS and Linux builds, installers, auto-update | 🟡 CI tests Windows, Linux and macOS; the release workflow builds a Windows installer (Inno Setup), a macOS disk image and a Linux tarball (0.9.0-beta); updates ✅ (checked at start, Update swaps the files in place on all three, the Windows installer only for all-users installs; the macOS swap untried on a Mac); not signed or notarized; plugin editors open on all three (Win32, Cocoa, X11 with the run loop Linux editors need), untried by hand on macOS and Linux |
| Extension package format and gallery | ✅ `.ravunext` (themes, scripts, templates, sounds); Preferences → Extensions installs from a file or the gallery (`gallery/index.json`, two packs) and removes; the gallery and update check need the repository to be public |
| Performance pass, docs site | 🟡 docs site ✅ (VitePress in `site/`, live at g1lg1l.github.io/ravune; the manual and every command generated from the app); no performance pass yet |
| Opt-in crash reporting | ✅ a report stays local; the next start offers a prefilled GitHub issue |
