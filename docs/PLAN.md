# Ravune: Plan

> Living document. Last update: 2026-10-26. Research backing every decision: [RESEARCH.md](RESEARCH.md). Theme/layout design: [THEMING.md](THEMING.md).

## 1. Vision

**The DAW anyone can play, and nobody outgrows.**

| Take from | What |
|---|---|
| FL Studio | Free-form Playlist (any clip on any track), patterns spanning all channels, step sequencer, tool-packed piano roll, drag-from-anywhere browser, multiple arrangements, lifetime-free spirit |
| Ableton Live | Device chains, racks with macros + variations, Drum Rack, MIDI transformations/generators, scale awareness, Info View |
| Bitwig | Modulators on every device, per-voice modulation, clip aliases, reusable automation clips |
| Blender | Every action is an operator; split/retype areas; workspaces; granular themes and keymaps; extensions |

**Non-goals (for 1.0):** video, notation/score editing, surround/Atmos, collaboration in the cloud, a mobile app.

## 2. Principles

1. **Beginner first, power discoverable.** Opinionated defaults and ready workspaces; depth reveals itself (Blender pre-2.8 is the warning).
2. **Sound in 10 seconds, beat in 60.** Every milestone is measured against this.
3. **Everything is an operator.** No UI action exists that a keymap, script or agent can't call.
4. **No tiers, no walls.** Every feature for everyone.
5. **The audio thread is sacred.** No allocation, no locks, no I/O in the callback: enforced in debug with `assert_no_alloc`.
6. **No hardcoded look.** Every color, radius, spacing and animation comes from the theme.
7. **AI is a guest.** Optional extension, off by default, local-first, never trains on user projects, fully removable.
8. **Vertical slices.** Each milestone ends with something playable. No engine-framework or GUI-toolkit side quests (Meadowlark's lesson).
9. **Files are friends.** Diffable text project format; open formats (CLAP, VST3, SFZ, DAWproject).

## 3. Architecture

```
┌──────────────────────────── UI thread (egui) ────────────────────────────┐
│  Areas/Editors: Playlist · Channel Rack · Piano Roll · Mixer · Devices · │
│                 Browser · Info · Command palette · Assistant (opt.)      │
│        │ user input / keymap / Lua / MCP                                 │
│        ▼                                                                 │
│  Operator registry ──► execute(&mut Project, props) ──► Undo history     │
│        │                                   (snapshot of Arc-shared model)│
│        ▼                                                                 │
│  Project model (pure data, no audio, no UI)                              │
│        │ "compile" on change (worker thread)                             │
└────────┼─────────────────────────────────────────────────────────────────┘
         ▼  rtrb SPSC queue (new EngineSnapshot / small param+transport msgs)
┌──────────────────────── Audio thread (cpal callback) ────────────────────┐
│  Schedule: clips → channels → device chains → mixer tracks → master      │
│  sample-accurate events · latency compensation · modulators              │
│  old snapshots retired via rtgc/basedrop · meters → triple_buffer → UI   │
└──────────────────────────────────────────────────────────────────────────┘
   side workers: disk streaming (creek) · waveform peaks · plugin hosts
   (out-of-process) · AI jobs (ort) · MCP server (rmcp, localhost)
```

### 3.1 Operators (the Blender idea)
```rust
pub trait Operator {
    const ID: &'static str;            // "pattern.add_note", "mixer.route"
    type Props: Serialize + DeserializeOwned + JsonSchema;
    fn label() -> &'static str;
    fn description() -> &'static str; // feeds Info View, tooltips, MCP docs
    fn poll(project: &Project) -> bool { true }
    fn execute(project: &mut Project, props: Self::Props) -> Result<()>;
}
```
One registry powers: command palette (Ctrl+K / F3), keymaps, menus, Info View, undo labels, Lua API, MCP tools, macro recording. Undo = snapshots of an `Arc`-shared model (like Blender's memfile undo): simplest correct approach; revisit if memory grows.

### 3.2 Project model
- **Time:** musical ticks (960 PPQ) + tempo map → sample positions. Global key/scale.
- **Channels** (FL Channel Rack): instrument/sampler/plugin + device chain + target mixer track.
- **Patterns:** multi-channel note data. Steps are a *view* of notes. Notes carry velocity, release, pan, fine pitch, and per-note expression curves (MPE-ready from day one).
- **Clips:** pattern, audio, automation. Live on free **Playlist lanes**; optional **linked track** mode (lane ↔ channel ↔ mixer track). Clip aliases.
- **Arrangements:** many per project, sharing channels and mixer.
- **Mixer:** tracks with device chain, any→any sends (cycles rejected), master.
- **Devices:** built-in, CLAP, VST3, Lua/WASM. Each exposes params; any param accepts modulators and automation.
- **Format:** `MySong.ravune/` folder → `project.ron` + `samples/` + `backups/`. Single-file `.ravune` zip for sharing. DAWproject import/export in M10.

### 3.3 Workspace layout (Cargo)
Start with three crates, add more only when a milestone needs them:
- `ravune-core`: project model, operators, undo, serialization. Pure, fully unit-tested.
- `ravune-engine`: audio graph, devices, transport, offline render.
- `ravune` (app): egui UI, editors, theme, input, MIDI.
- Later: `ravune-plugins` (CLAP/VST3 host), `ravune-mcp`, `ravune-script` (Lua), `ravune-ai`.

## 4. Tech stack (verified 2026-10)

| Area | Choice | Notes |
|---|---|---|
| GUI | **egui 0.37 + eframe/wgpu + egui_tiles**: ✅ decided 2026-10-24, never switch | Blender-like immediate UI with custom theme layer; wgpu callbacks for heavy canvases (piano roll, playlist, waveforms); AccessKit built in; `egui_kittest`/`egui_mcp` for UI tests. Rejected: iced (no a11y, API churn), Tauri 2 (two languages, IPC, weak Linux webview). |
| Audio I/O | cpal 0.18 (WASAPI, ASIO opt-in, CoreAudio, ALSA/JACK/PipeWire) | + audio_thread_priority |
| MIDI | midir · wmidi · midly · midi2 (later) | Windows MIDI Services / MIDI 2.0 in a later phase |
| RT plumbing | rtrb · rtgc/basedrop · triple_buffer · arc-swap · no_denormals · assert_no_alloc | |
| DSP | own engine + fundsp inside devices | Firewheel studied, not used (game-oriented) |
| Media | symphonia · creek · hound · flacenc · rubato · signalsmith-stretch | all permissive |
| Plugins | clack-host (CLAP first) · vst3 (coupler-rs) · livi (LV2, Linux) | out-of-process sandbox; VST3 SDK is MIT since 3.8 |
| Scripting | mlua (Luau, sandboxed) | wasmtime components later for sandboxed DSP |
| MCP | rmcp 3.x (official SDK) | localhost streamable HTTP + stdio shim |
| AI | ort (ONNX Runtime) · burn fallback | worker threads only |
| Instruments | own SFZ-subset sampler | sfizz lib archived, no maintained Rust SFZ player |

**License: MIT OR Apache-2.0** (✅ decided 2026-10-24, the Rust ecosystem convention). Consequences:
- **No GPL dependencies**: Rubber Band, nih-plug's VST3 bindings, Slint-GPL are out. Enforced with `cargo-deny` in CI.
- **ASIO is an opt-in build feature** (Steinberg ASIO SDK is GPLv3/proprietary): default Windows builds use WASAPI (shared + exclusive); ASIO builds are either built by users or distributed under Steinberg's proprietary ASIO agreement.
- VST3 hosting is fine (VST3 SDK is MIT since 3.8).

## 5. Milestones

Every milestone = a playable build + exit criteria. Ordered by "what a beginner needs next". What each one still leaves open is in [AUDIT.md](AUDIT.md).

### M0: Foundations ✅
Research, plan, repo, Rust toolchain. **Exit:** GUI toolkit (egui) and license (MIT OR Apache-2.0) decided.

### M1: "First Beep" ✅ (2026-10-24)
> Done: everything below. Measured on the dev machine: 48 kHz, 10.0 ms buffer (WASAPI shared mode's floor: going under 10 ms needs the opt-in ASIO build, not wired yet). Synth is hand-written (polyBLEP + ZDF SVF) instead of fundsp: smaller and allocation-free. CI runs Linux + Windows on every push; macOS on manual runs.

- Cargo workspace (core / engine / app), CI on Windows/macOS/Linux.
- cpal output with device picker, buffer size, latency display; recover from device changes.
- Transport (play/stop/tempo), metronome.
- One built-in synth ("VA-1": 2 osc + filter + ADSR, fundsp).
- Play notes from computer keyboard and any MIDI keyboard (midir, auto-detect, hot-plug).
- Operator registry + undo/redo + command palette from day one.
- App shell with theme tokens (no hardcoded colors), default theme v0 (Ableton-like "Ravune Dark").
- **Exit:** launch → press a key → sound in under 10 s; < 10 ms output latency on ASIO/WASAPI; zero allocations in the callback (asserted).

### M2: "First Beat" ✅ (2026-10-24)
> Done: channel rack with paint-to-toggle steps (16–64), sample-accurate sequencer, global swing, mute, per-channel volume/pitch, choke groups; polyphonic sampler (any format via symphonia, mono for now); browser (built-in library, user folders, search, favorites, click-preview, double-click/drag to add or replace); synthesized CC0 kit (11 sounds) + embedded demo project; `.ravune` RON files with atomic save, autosave every 30 s, restore-on-next-launch; open from the command line.
> Deviations: swing is global (per-channel later); step velocity has no editor yet (set via operator, shown as brightness: velocity lane comes with the M3 piano roll); sampler has no ADSR yet (M4 devices); preview isn't tempo-synced (only matters for loops, M3); external CC0 packs (Cowsynth, VCSL…) not bundled yet; project is a single `.ravune` file referencing samples by path ("collect samples" later).

- Channel Rack + step sequencer (16→64 steps, per-channel swing, velocity).
- Sampler (one-shot, pitch, ADSR, choke groups).
- Browser v1: folder tree, user folders, click-to-preview (tempo-synced), drag to rack, favorites, search.
- **Demo sound pack v1** (see §7). Starter templates.
- Save/load (`project.ron`), autosave, crash recovery.
- **Exit:** fresh install → 4-bar beat in < 60 s, saved and reopened.

### M3: "First Song" ✅ v1 (2026-10-24)
> Done: tick timeline (96 PPQ); multiple FL-style patterns (steps + piano-roll notes per channel; new/clone/rename/delete); PAT/SONG transport with start marker; playlist with free lanes, pattern clips (loop when longer than the pattern) and audio clips (waveforms, drop from browser, chase when starting mid-clip), click-to-place, drag, resize, split, duplicate, delete; piano roll v1 (draw/move/resize/delete with audition, velocity lane, snap 1/4–1/32, song key + scale highlight, fold-to-scale, ghost notes, quantize); sample-accurate tick sequencer; offline WAV export (16-bit, 44.1 kHz, 2 s tail) on a worker thread; demo song (60 bars ≈ 2 min, A minor, bundled sounds only): the exit criterion.
> Deferred: automation clips, loop region, markers, clip aliases, multiple arrangements, song-structure helper, piano-roll multi-select and chord stamp/strum/arp/randomize, tempo-sync stretch for audio clips (they play at their own speed), FLAC/stem export, stereo (engine is mono until M4).

- Playlist: free lanes, pattern/audio/automation clips, drag/resize/split/duplicate, loop region, markers, clip aliases.
- Piano roll v1: draw/select/move/resize, velocity lane, scale highlight + snap, ghost notes, quantize, chord stamp, strum, arp, randomize.
- Audio clips: import any format, waveform peaks, tempo-sync stretch (signalsmith-stretch).
- Multiple arrangements. Song structure helper (Intro/Verse/Chorus blocks) to escape the loop trap.
- Export WAV/FLAC/stems (offline render).
- **Exit:** make and export a full 2-minute song using only bundled sounds.

### M4: "Sound Design" ✅ v1 (2026-10-24, v0.4.0)
> Done: stereo engine. FL-style mixer (up to 32 tracks, master, route to any bus, post-fader sends, loops rejected, fader/pan/mute with click-free ramps, post-fader peak meters). Effect chains on mixer tracks with 8 built-in devices (EQ Three, Filter, Compressor, Saturator, Chorus, tempo-synced ping-pong Delay, Freeverb Reverb, Limiter), built on the UI thread and handed to the audio thread by stable id, so tails survive edits. Ableton-style Device view: the instrument, then its track's chain. 8 named macros and 4 LFOs (sine/triangle/saw/square, free Hz or locked to the bar) mapped onto any synth knob, effect knob, fader or pan by right-click, amounts ±100% of the knob's travel, applied every 256 samples. Waveform overview with 3-band analysis (lightrb's bands and colors) and a Mono / RGB / 3-Band setting. Drag & drop from the OS (audio → playlist at the cursor or rack, folders → browser, projects → open). Hold-Space hand tool in the playlist. Stereo WAV export. Demo song mixed on buses with FX, sends, an LFO and two macros. Exit criterion: wobble bass = right-click VA-1 Cutoff → New LFO → Sync, Length ½ beat.
> Deferred: insert chains on channels (effects live on mixer tracks, FL-style), routing cables view, solo, 16 macros + variations + Rand, envelope / random / keytrack / velocity modulators and per-voice modulation, drag-to-assign, automation (recording, drawing, clips), plugin delay compensation (no latent devices yet), limiter look-ahead, stereo samples (still summed to mono), audio clips routed to their own track (they play into the master).

- Mixer: tracks, sends, any→any routing with cables, master, meters.
- Device chains on channels and mixer tracks; built-in FX: EQ, compressor, limiter, reverb, delay, filter, distortion, chorus.
- Racks with up to 16 macros, macro variations + Rand.
- Modulators on any parameter (LFO, envelope, random, macro, keytrack/velocity), per-voice where it applies; drag to assign.
- Automation recording/drawing; reusable automation clips; plugin delay compensation.
- **Exit:** build a wobble bass from the stock synth + modulators without reading a manual.

### M5: "Make It Yours" (UX pass + customization)
> Asked for on 2026-10-24: tighter, consistent spacing: padding inside elements, never outer margins, every region separated by dividers, scrollbars flush against the dividers; keyboard shortcuts and full keyboard access; good-looking icons (iconify-style); floating windows and modals (e.g. a floating pattern editor); React Bits-style micro-interactions, ported to Rust; an app that feels instant and stays light; sidechain that is trivially easy; a browser with folders, subfolders and tabs for instruments.

- **UX foundation:** spacing tokens on a 4-px grid, padding-only layout, 1-px dividers between regions, edge-to-edge scroll areas with thin scrollbars on the dividers, fixed-height header bars, Phosphor icon font (MIT, subset to the glyphs used).
- **Keyboard first:** every editor and panel one key away, focusable custom widgets (knobs, faders, steps, browser rows) with a visible focus ring, arrows / Enter / Esc everywhere, a shortcut sheet (F1), AccessKit names and values for screen readers.
- **Floating windows and modals:** any editor pops out into a floating window (FL-style) and docks back; double-clicking a pattern clip opens it in a floating editor; one modal component (title, Esc / Enter, focus) for rename, confirm and preferences.
- **Micro-interactions** (ported from React Bits): pointer spotlight and border glow on cards, a sliding active-tab pill, animated list selection with edge fades, elastic knob and fader limits, a glowing focus ring. Cheap to draw, repaint only while animating, off with "reduce motion".
- **Sidechain in one click:** "Duck to…" on a mixer track inserts a compressor keyed by the trigger track, tuned for pumping; any compressor can pick a sidechain source.
- **Browser 2.0 (local):** folder tree with subfolders, tabs for Samples · Instruments (built-ins and presets) · Plugins (installed CLAP / VST3 listed; loading arrives in M7), keyboard navigation.
- **Customization:** theme editor with built-in themes, keymap presets ("Ravune", "FL-style", "Ableton-style"), Blender-style areas and workspaces (Beat, Compose, Sound Design, Mix, Learn), UI scale, reduce motion.
- **Light and instant:** cold start in well under a second, ~0 % CPU when idle, small release binary (LTO, strip, subset fonts).
- **Exit:** a user can make Ravune look and lay out like their old DAW in 5 minutes, and drive a whole session from the keyboard.

> Progress (2026-10-24), done: the layout kit (padding-only, dividers, edge scroll bars), Phosphor icons, keyboard (F1 sheet, focus ring and arrow keys on every control, AccessKit names), floating editor windows and the modal kit (palette, shortcuts, rename, Preferences window), one-click sidechain ("Duck to…" + a sidechain picker on every compressor, keyed post-effects / pre-fader), browser tabs with a folder tree and VA-1 presets, foldable browser / editors / keys / effect cards, beat and 16th labels on the zoomed ruler, React Bits-style motion (spotlight, tab pill, click sparks, step pop, cascading lists, playhead glow, clip lift, toasts) with reduce motion, a second demo ("Hardgroove", 145 BPM), and a lighter build (fonts 1.1 MB → 130 KB, no egui fallback fonts or system-font scan; release 19 MB, window in ~0.2 s, ~0 % CPU idle).
> Done (2026-10-25): four built-in themes and a live theme editor (every color token and size; `.ravune-theme` files in RON, colors as hex, missing tokens fall back); keymaps (Ravune, FL Studio, Ableton layouts; rebind any command; only changes are stored; the F1 sheet, tooltips and palette read the keymap); workspaces (Beat, Compose, Sound, Mix + your own, Ctrl+PageUp/Down) and a split editor area. After review: the mixer's effects open in a right-hand panel with full-height strips, LED meters, see-through playlist clips.
> Deviation: workspaces remember the panel layout, docked / split / floating editors and the browser tab instead of a free tree of areas (egui_tiles): the panel layout kept its animations and stayed simple; a full area tree can come later if the fixed regions feel limiting.

### M6: "Find Sounds" (online browser)
- An **Online** tab in the browser: search free sounds without leaving Ravune: [Openverse](https://openverse.org) (Creative Commons audio, no account needed) and [Freesound](https://freesound.org) (with a free API key). Filters: one-shot / loop, BPM, key, duration, license.
- Streaming preview; drag a result onto the rack or the playlist and it is downloaded into the project's `samples/` folder with its license and author saved; "Export credits" writes the attribution list.
- Free instruments too: CC0 / CC-BY sample libraries (SFZ, Decent Sampler) and VA-1 preset packs; the same tab later lists the community gallery (M10).
- License safety: the license is on every result, the default filter is CC0 + CC-BY, and a project can forbid "non-commercial" sounds.
- **Exit:** find a kick, a vocal chop and a pad online and build a loop with them in under two minutes, credits included.

> Done (2026-10-25): the Online tab (Openverse anonymous, Freesound with a key; length and license filters, CC0 + CC BY by default, SA / NC opt-in, never ND), preview from a cache, keeping a used sound in Documents/Ravune/Online Sounds with its credit in `credits.json`, File → Export credits; "search online" when the local search finds nothing; SFZ and Decent Sampler multi-sample instruments (regions, round robins, loops, release) and a list of free libraries to download.
> Deviations: credits live beside the kept sounds (per folder) rather than inside the project file; non-commercial is a filter of the tab, not a per-project rule; BPM / key filters are not there yet (Openverse has no such fields); Openverse allows 20 anonymous searches a minute, 200 a day.

### M7: "Plug It In"
- CLAP hosting, then VST3; plugin scanner in a separate process; out-of-process sandbox so a crash never kills the session.
- Plugin database in browser (thumbnails, presets, tags).
- MIDI learn on any parameter (right-click → Learn); Lua controller scripts; MPE input.
- **Exit:** 20 popular free CLAP/VST3 plugins load, play, save, reload; killing a plugin process doesn't crash Ravune.

> Done (2026-10-25): `ravune-bridge` (shared memory + named events per block, JSON-line control, late / dead plugins go silent instead of stalling the audio thread) and `ravune-host` (CLAP via clack-host, VST3 via the vst3 crate: scan, load, process with sample-accurate notes, parameters, state, transport, MIDI, editors in a Win32 window). Plugins tab with a cached scan database, plugin instruments and effects in projects with their state saved, device cards with editor, parameters, crash state and Reload. MIDI learn on any knob or fader (saved with the project) and MPE (per-note bend, pressure, slide for VA-1 and plugins). Tested end to end with a CLAP test synth and the vst3 crate's gain example; the installed Elektron Overbridge VST3s scan, load and open their editors.
> Exit test (2026-10-25, `cargo test -p ravune-host --release --test plugins -- --ignored --nocapture`): every installed plugin is scanned, loaded in its own process, played (a note, or noise into effects), given a parameter change that lands in its state, saved and loaded again from that state. **25 plugins pass, 41 CLAP / VST3 builds, none fail:** Surge XT, Surge XT Effects, Dexed, OB-Xf, Monique, Six Sines (+ Seven Outs), Shortcircuit XT, RipplerX, Dragonfly Hall / Room / Plate / Early Reflections, FILT-R, GATE-12, TIME-12, REEV-R and Conduit's Polysynth, Polymetric Delay, Ring Modulator, MultiOut Clock Synth, Chord Memory, MTS adapter, Event Monitor, MIDI2 SawSynth. Silent on a note by design: Shortcircuit XT (no sample loaded), Conduit's Event Monitor and its MIDI2 SawSynth (a demo that never writes audio). The Elektron Overbridge plugins are skipped (they stream from the hardware). The test found and fixed three host bugs: CLAP plugins with sidechain inputs or extra outputs crashed (only the main ports had buffers), notes ignored a plugin's preferred dialect (MIDI 1 / MIDI 2 now sent when asked), and CLAP processing was stopped off the audio thread; VST3 teardown now releases interfaces before the objects behind them. Vital (needs an account) and Airwindows (installer only) weren't installed.
> Not yet: Lua controller scripts move to M8 with the rest of Lua; (Plugin editors were Windows-only here and macOS / Linux polled for the audio handoff; editors now open in a Cocoa window on macOS and an X11 one on Linux, and the handoff sleeps on named pipes there.)

### M8: "Talk To It" (scripting + MCP)
- Lua API = operator registry + project queries. Piano-roll scripts and MIDI generators/transformations as Lua extensions.
- Built-in MCP server (rmcp): small tool set: `search_operators`, `run_operator`, `query_project`, `browse_sounds`, `render_and_analyze` (loudness, spectrum summary, key, onsets) so agents can "hear".
- Every agent call = one labeled undo group; visible command log; permission tiers (read / edit / files); off until enabled.
- **Exit:** Claude (or any MCP client) builds a 4-bar beat + bassline in a running Ravune, and one Ctrl+Z reverts it.

> Done (2026-10-26): `ravune-mcp`, a stdio MCP server that speaks to a running Ravune over localhost with a token (search_operators, run_operator, query_project, browse_sounds, render_and_analyze, checkpoint, undo, save_project); Preferences → Agents (off by default, edit / files permissions, a log of every call); an agent's edits are one "Agent edits" undo step. Luau scripts (sandboxed, 2 s limit, one undo step) in the piano roll's Tools and the palette, nine built in. Exit test passed against a running Ravune: an agent built a 4-bar beat and bassline, one Ctrl+Z reverted it, a client without the token was turned away.
> Deviations: a hand-written JSON-RPC server instead of rmcp (one small dependency-free binary); Luau (mlua) instead of plain Lua; Lua controller scripts aren't there yet.

### M9: "Smart Helpers" (optional AI extension)
- Local stem separation (Demucs ONNX) → stems to tracks.
- Audio → MIDI (Basic Pitch ONNX).
- Neural device host: user-loaded models (e.g. RAVE) on a worker thread with reported latency; AI synth plugins via CLAP also welcome.
- "Explain this" assistant and loop→song suggestions (works with local or user-provided models).
- **Exit:** everything works offline; uninstalling the extension leaves no trace.

> Done (2026-10-26), without neural models: "Split into drums and tones" (harmonic / percussive separation) and "Notes from this sound" (YIN, one voice) on any audio clip, Create → Song from the patterns, and the MCP server as the assistant. Demucs, Basic Pitch and the neural device host wait for an ONNX runtime decision.

### M10: "Hello World" (public beta)
- Interactive in-app lessons ("make your first beat"), more templates and sounds.
- Finish the in-app Manual (Help → Manual): a chapter for every editor and workflow (it starts with two: a first beat, automation).
- DAWproject import/export.
- macOS (Apple Silicon) + Linux (PipeWire) builds, installers, auto-update.
- Extension package format (themes, keymaps, scripts, devices, sound packs) + gallery v1.
- Performance pass, crash reporting (opt-in), docs site.
- **Exit:** 10 non-producer testers make a song in their first session.

> Done (2026-10-26): four interactive lessons; seven genre templates and five CC0 genre packs (77 sounds); the Manual's eleven chapters; DAWproject import / export; `.ravunext` extensions with a gallery and an update check; opt-in crash reports. Not yet: installers and a self-updater, macOS / Linux testing, the docs site, the testers.

### Later (post-beta backlog)
Session/clip launcher view · Patcher/Grid modular environment · audio recording comping · WASM DSP devices · Windows MIDI Services / MIDI 2.0 · tablet sketchpad companion · collaboration · Link.

## 6. Quality gates (every milestone)
- `ravune-core`: unit tests for every operator (execute + undo round-trip).
- `ravune-engine`: offline-render golden tests (bit-exact or tolerance), `assert_no_alloc` in debug.
- UI: `egui_kittest` smoke tests for critical flows (first beat, save/load).
- CI: build + test on Windows, macOS, Linux; clippy; a check that no color literals exist outside the theme module.

## 7. Demo sound pack
Goal: small, legally clean, instantly fun. Licenses verified per source; attribution in `assets/sounds/CREDITS.md`.

| Content | Source | License |
|---|---|---|
| 808/909-style drum kit (kick, snare, clap, hats, toms, perc) | **Synthesized in-house** with an offline fundsp script | CC0 by construction |
| Synth/bass one-shots | Karoryfer *Cowsynth* (14 MB), VCSL *TX81Z* subset | CC0 |
| Keys | Greg Sullivan E-Pianos (21.5 MB) | CC-BY 3.0 |
| Piano (optional download) | Salamander Grand, trimmed to ~4 velocity layers | CC-BY 3.0 |
| Loops | Hand-picked Freesound CC0 loops (license file checked per sound) | CC0 |

In repo: ≤ 30 MB (FLAC). Larger "Starter Pack" as a GitHub release asset, downloaded on first run. **Avoid:** Dirt-Samples (no license), "royalty-free" packs (no redistribution), NC-licensed sets, RAVE checkpoints.

## 8. Risks & mitigations
| Risk | Mitigation |
|---|---|
| GUI toolkit churn (killed Meadowlark) | Decide in M0, wrap custom canvases behind our own widgets, never switch |
| Scope creep / burnout | Vertical milestones with hard exit criteria; "Later" list is allowed to stay later |
| Plugin crashes destroy trust | Out-of-process hosting, autosave, crash recovery |
| Real-time glitches | Lock-free design, `assert_no_alloc`, stress tests with tiny buffers |
| Thin content | Bundled pack + templates from M2; synthesize our own drums |
| AI backlash | AI is an optional extension, off by default, no-training pledge in README |
| GPL code sneaking in (ASIO, Rubber Band, bindings) | `cargo-deny` license allowlist in CI; ASIO behind an opt-in feature |

## 9. Open questions
1. Visual identity: mascot? logo? default theme palette (see THEMING.md).
2. Theming questions listed at the end of THEMING.md.
