# Ravune: Research

> Snapshot as of **2026-10-24**. Versions: FL Studio 2026.1.7 · Ableton Live 12.4.7 · Bitwig Studio 6.1 · Blender 5.2 LTS.
> Reddit could not be scraped directly; sentiment comes from KVR, Gearspace, vendor forums, Lemmy, G2/Capterra, trade press and surveys. Treat single-forum anecdotes as signals, not facts.

## TL;DR

1. **No one combines FL's ease with Ableton/Bitwig's sound-design depth without tier walls.** LMMS stalled (1.3 still alpha), Ardour is audio-first, Zrythm is mid-rewrite, Meadowlark (Rust) is on hiatus. No Rust DAW has real traction.
2. **The Blender lesson is architectural, not visual:** every action is a typed *operator*. That single decision gives undo, keymaps, command search, scripting and an MCP server for free.
3. **Beginners quit because they don't know where to start and get stuck in 8-bar loops**, not because of missing features. 61 % of quitters quit in year one.
4. **AI: producers want assistive, local, opt-in AI** (stems, mastering help, tedious edits, "explain this") and reject song generation, cloud upload and training on their sessions. Blender's 2026 Anthropic backlash shows AI positioning can split a creative community: Ravune's AI must be **off by default, local-first, and fully removable**.
5. **Every existing DAW↔LLM integration is a fragile bolt-on** (remote scripts, OSC, file exchange; no grouped undo; can't read automation or audio). A *native* MCP server on top of the operator layer is a genuine first.
6. **Tailwinds:** VST3 SDK is MIT since Oct 2025 (ASIO GPLv3), CLAP is open, DAWproject is an open exchange format, PipeWire made Linux audio sane.

---

## 1. What to take from each DAW

### FL Studio: the timeline and the browser
| Feature | Why it matters |
|---|---|
| **Playlist with free clips**: pattern, audio and automation clips go on *any* track and can overlap; tracks aren't tied to instruments. Optional "linked" mode (drop an instrument on a track header → channel+track+mixer stay in sync). | Drag anything anywhere, zero setup. Linked mode keeps classic-DAW users happy. |
| **Patterns span all channels**; Channel Rack step sequencer is a *view* of pattern data (16→512 steps, per-channel swing, graph editor for per-step velocity/pan/pitch). | First beat in 30 seconds. |
| **Piano roll tools**: chord stamp + voice leading, strum, arp, chop, glue, flip, flam, limit, randomize, LFO shaping, riff machine, slide/porta notes, ghost notes from other channels, scale highlight/snap, per-note velocity/pan/release/mod X-Y/fine pitch, **Python scripts that transform notes**. | One-click non-destructive transforms next to fast hand editing = why it's "best-in-class". |
| **Browser**: folder tree + user tabs (All, Current Project, Plugin DB, Starred), 30 extra search folders, click-to-preview synced to tempo, drag to playlist/channel/mixer/effect slot, tags with Any/All, boolean search, saved searches, "find similar", inline waveforms (2026), "Current Project" tab (undo history, patterns, used samples). | The thing users say they miss most when switching. |
| **Picker panel** (every clip in project, drag/rename/recolor/delete unused), **multiple arrangements** sharing one rack/mixer, sub-track groups, Performance mode. | Cheap song versions, project hygiene. |
| **Mixer**: any→any routing with drawn cables, feedback blocked. **Patcher**: node graph with color-coded wires (audio/param/MIDI), custom control-surface panels, saved as preset. | Modular without leaving the DAW. |
| **Lifetime free updates.** | The #1 loyalty driver; any later upsell (FL Cloud) is watched suspiciously. |
| 2025–26 additions: Loop Starter, Gopher assistant that *acts* in the project (experimental, no training on user data), stem separation → mixer tracks, cloud backup on every save. | Validates "assistant that acts" + "blank-page helpers". |

**Avoid:** clumsy live audio/MIDI recording, mixing so weak users export to another DAW, non-customizable shortcuts, slide notes only on native instruments, no recording in the cheapest edition.

### Ableton Live 12: devices, racks, learning
| Feature | Why it matters |
|---|---|
| **Racks** (Instrument / Drum / Audio FX / MIDI FX): parallel chains, nesting, key/velocity/chain-select zones, **16 macros** with per-mapping min/max (inverted ranges), **Macro Variations** + **Rand**. | Complex instruments behind a one-knob face. |
| **Drum Rack**: 128 pads, choke groups, return chains, drag-to-layer. | The best drum workflow there is. |
| **MIDI Tools** (12.0): Transformations (Arpeggiate, Connect, Ornament, Quantize, Recombine, Span, Strum, Time Warp) and Generators (Rhythm, Seed, Shape, Stacks), user-extensible. | Generators/transforms as a *plugin API*. |
| **Scale awareness** everywhere (scale mode, fold to scale, scale-aware devices), microtonal `.ascl`. | No wrong notes for beginners. |
| **Tag-based browser** with `#tag` search, labels, **Similarity Search** (find/swap similar sounds). | Large libraries stay manageable. |
| **Info View / Learn View**: hover any control for an explanation. | The gold standard of learning-by-doing. |
| **Session View** clip launcher alongside Arrangement; Warping (Beats/Tones/Texture/Re-Pitch/Complex). | Jam first, arrange later. |
| 12.3–12.4: local stem separation, device A/B compare, Bounce in Place, Splice in browser. | Table stakes are rising. |

**Avoid:** no universal modulation (needs Max for Live → Suite only), Lite's 8-track wall, tier-gated features, awkward piano roll for beginners, single rigid window, no Linux.

### Bitwig Studio 6: modulation
- **Modulator slots on every device**; a modulator can target any parameter (including other modulators); many go **per-voice/polyphonic**. Assigned by dragging.
- **The Grid**: 190+ modules, Poly/FX/Note grids, first-class devices with remote knobs and nested chains.
- v6: **clip-based reusable automation**, curved segments, **clip aliases** (linked clips), **global key** followed by note devices, MPE on clips, auto backups. 6.1: rebuilt sampler (slicing, spectral, granular).
- **Avoid:** dense shortcut-heavy UI, thin stock sounds, 12-month upgrade plans, Grid gated to the top edition.

### Blender: the model for a modular app
- **Areas/editors**: split by dragging a corner, change any area's editor type from its header, swap, maximize, tear off into a window.
- **Operators**: every action has an id, `poll` (can it run now?), `execute` (scripts), `invoke` (UI/keymap, may open a dialog), `modal` (interactive drags). Typed properties auto-generate UI, undo/redo and the "Adjust Last Operation" panel, and register in **F3 search**.
- **Keymaps** bind operator + properties to input per editor; presets (Blender / Industry Compatible); export/import.
- **Themes**: per editor, per widget, live preview, shareable presets; 230 themes on extensions.blender.org.
- **Workspaces**: saved layouts that can also switch mode and filter add-ons.
- **Warning:** pre-2.8 "everything is configurable" produced a hostile first run. Ship opinionated defaults and beginner workspaces; make power discoverable, not mandatory.

### MIDI controllers
- FL: pick controller type, "Link to controller"/multilink, event-driven Python MIDI scripting.
- Ableton: auto-detected Control Surface slots, MIDI/Key map modes, undocumented Python remote scripts, deepest hardware = Push; MPE since Live 11 (Note Expression tab).
- Bitwig: JS/Java controller extensions, huge community (DrivenByMoss).
- **Pattern for Ravune:** auto-detect profiles + right-click "Learn" on any parameter + controller scripts over the *same* operator API. MPE/per-note expression in the note model from day one.

---

## 2. Market research

### Love / hate / opportunity
| Area | Users love | Users hate | Ravune opportunity |
|---|---|---|---|
| Pattern / step sequencer | FL: instant, "like a video game" | Weak audio + mixing | Patterns *and* a strong audio timeline in one Playlist |
| Device chains / racks | Macros, Bitwig modulators | Lite/Intro caps, Suite-only features | Full chains + modulators for everyone, no tiers |
| Pricing | FL lifetime updates, Reaper $60 honor license | Subscriptions, paid add-ons, free→paid (Cakewalk→Sonar) | Free core forever |
| AI | Local stems, mastering assist, help | Song generation, training on sessions, cloud upload | Local, opt-in, removable AI; written no-training pledge |
| Scripting / MCP | Max for Live (6k+ devices), ReaScript, community MCP servers | Fragile bridges, no undo, blind to automation/audio | Native MCP over operators with grouped undo + visible command log |
| Themes / layout | Reaper Stash, Blender themes | FL theme docs, Reaper's default look | Theme + workspace format with a one-click gallery |
| Onboarding | GarageBand, BandLab, Learning Music | "Tutorials assume expertise" | Interactive lessons *inside* the DAW |
| Arrangement | Clip launching, loops | Stuck at 8-bar loops | Loop→song structure helpers |
| Platforms | Bitwig/Reaper/Ardour on Linux | Ableton refuses Linux; Logic Mac-only | First-class Win/macOS/Linux, CLAP-first |
| Interop | DAWproject | Lock-in | DAWproject import/export + diffable text project format |

### Per-DAW sentiment (condensed)
- **FL Studio**: loved for patterns, piano roll, lifetime updates, easiest setup (G2). Hated for recording/mixing; Fruity edition has no audio recording; FL Cloud seen as eroding the lifetime promise.
- **Ableton Live**: loved for Session View, racks, Max for Live. Hated for price ($99/$439/$749), Lite's 8 tracks with no upgrade path, Suite-only stems, messy MIDI tools, no Linux; newcomers call it overbearing.
- **Bitwig**: loved for modulators, Grid, Linux, CLAP. Hated for learning curve, thin content; trust dented by the 2022 Spectral Suite paid-add-on attempt (reversed).
- **Reaper**: loved for $60 license, never-locking trial, speed, extreme customizability. Hated for ugly defaults that need lots of tweaking.
- **Logic**: loved for value, Session Players, Stem Splitter. Hated for Mac-only and the Jan 2026 Creator Studio subscription push.
- **Fender Studio Pro** (ex-Studio One, renamed Jan 2026): rename lost 20 years of tutorial SEO; mixed pricing reactions.
- **GarageBand / BandLab**: loved for free, instant, Smart Instruments, SongStarter, 100M+ BandLab users. Limited mixing/plugins; BandLab's Cakewalk→Sonar move caused backlash.
- **Open source**: Ardour 9 (Feb 2026) strong audio, weak MIDI; LMMS 1.3 alpha 2 (Sep 2026) no audio recording, flaky VST; Zrythm v2 alpha breaks v1 projects; Meadowlark on hiatus (engine split into Firewheel); small Rust DAWs ≤170★; openDAW (browser) moved its engine to Rust→WASM.

### Beginners
- EDMProd survey (1,000+): of those who quit, **61 % quit in year one**; top struggle = **"not knowing where to start"**.
- The **loop trap**: projects stall at 8–16 bars because beginners lack song structure.
- What feels easy: instant sound with no setup (FL step seq, GarageBand Smart Instruments), zero cost/install (BandLab), blank-page helpers (SongStarter, Loop Starter), guided learning (Ableton Learning Music, but it lives *outside* the DAW).
- Cost walls (Fruity no audio, Lite 8 tracks, Suite-only stems) push beginners away exactly when they get hooked.

### Pricing reactions
Lifetime free updates (FL) ≫ cheap honor license (Reaper) > tiered perpetual (Ableton) > upgrade plans (Bitwig) > subscription-only (tolerated only alongside perpetual) ≫ free-turned-paid (backlash).

### AI attitudes
- Tracklib 2024 (1,107 producers): 25 % use AI: 74 % of those for stem separation, 46 % mastering/EQ, 21 % generating elements, **3 % whole songs**. 82 % of non-users cite artistic reasons.
- SOS/Sonarworks 2026 (~1,200, 70 % pros): ~20 % regular AI users; ~60 % want tedious work automated; >33 % fear losing creative intent; 66 % rate ethical sourcing very important.
- Labels settled with Udio/Suno (late 2025) toward licensed models; Sony still suing. Suno bought WavTool and launched "Suno Studio".
- **Welcome:** local stems, mastering assist, cleanup, editing chores, sound search, in-app help, privacy guarantees.
- **Reject:** whole-song generation, opaque training data, session upload, AI replacing the producer.

### Existing MCP / LLM ↔ DAW integrations
| Project | Stars | How | Limits |
|---|---|---|---|
| ahujasid/ableton-mcp | ~3.2k | Live remote script | Default devices only, timeouts, "save first", big arrangements must be split |
| uisato/ableton-mcp-extended | ~280 | Remote script | Buggy automation, no Arrangement View |
| shiehn/total-reaper-mcp | ~87 | File exchange | 600+ tools split into profiles (LLM tool caps ~128), macOS-first |
| Bitwig (WeModulate, WigAI) | ~50–65 | OSC | Partial |
| FL Studio MCP | ~32 | MIDI script | Piano roll only: FL's API can't load plugins or create patterns |

**Shared gaps:** all external bolt-ons; can't read automation or audio; the model can't "hear"; no grouped undo; no vendor ships an official server. FL's Gopher is closed and in-app.
**Design implication:** expose a *small, composable* tool set (operator search + execute + project query) rather than 600 tools; one undo group per agent action; render/analysis tools so the agent can "hear" (loudness, spectrum, onset, key).

### Customization & community
Reaper Stash (hundreds of one-click themes, even clones of other DAWs), FL theme editor (poorly documented), Blender 230 themes, Max for Live 6k+ devices, ReaPack. Themes and devices create identity and sunk investment → retention. The community fills the long tail.

### Platforms
Linux demand is real (Ableton absent; PipeWire matured; plugin scarcity is the blocker). Apple Silicon native is table stakes. Tablets: sketchpad→desktop handoff (Ableton Note). Web DAWs are rising (BandLab, openDAW) but can't host plugins.

---

## 3. Opportunity gaps (ranked)
1. **Free, beginner-first, full-featured, cross-platform DAW** with no tier walls.
2. **Native MCP + scripting API** over operators, grouped undo per AI action, visible command log.
3. **Local, opt-in assistive AI only** (stems, mastering assist, sound search, "explain this") with a global off switch and a no-training pledge.
4. **Loop→song tools**: attack the #1 beginner quit reason.
5. **In-app interactive lessons + contextual help** (Info View × open-source Gopher).
6. **Blender-grade themes and workspaces** with an extension gallery.
7. **Device SDK** in the spirit of Max for Live/JSFX (sandboxed WASM/scripts), CLAP-first hosting; VST3 now unblocked by MIT.
8. **First-class Linux/PipeWire** + native Apple Silicon.
9. **DAWproject import/export** + diffable text project format (great for git and AI agents).
10. **Tablet/web sketchpad companion** (later).

## 4. Risks (why open DAWs fail)
1. **Engine/plugin reliability**: crashes and hosting bugs destroy trust. → sandboxed plugin process, autosave, crash recovery from day one.
2. **Rewrites and burnout** (Meadowlark, Zrythm v2, LMMS alpha). → small vertical slices, ship usable milestones, don't build a GUI toolkit or an engine framework before the app.
3. **Polish and content gap.** → bundled demo sounds, presets, templates, a polished UI as a first-class goal.
4. **Ecosystem network effects** (tutorials, packs target FL/Ableton). → FL/Ableton keymap presets, DAWproject, familiar vocabulary.
5. **AI backlash and funding.** → AI as optional extension, never core; clear stance in README.

---

## Sources (selection)
- FL manual: [Playlist](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/playlist.htm) · [Channel Rack](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/channelrack.htm) · [Piano Roll tools](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/pianoroll_menu.htm) · [Browser](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/browser.htm) · [Mixer routing](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/mixer_iorouting.htm) · [Patcher](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/plugins/Patcher.htm) · [MIDI scripting](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/midi_scripting.htm) · [What's new](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/basics_new.htm)
- Ableton: [Racks](https://www.ableton.com/en/live-manual/12/instrument-drum-and-effect-racks/) · [Live 12 release notes](https://www.ableton.com/en/release-notes/live-12/) · [Lite limits](https://www.ableton.com/products/live-lite/features) · [MPE FAQ](https://help.ableton.com/hc/en-us/articles/360019144999-MPE-in-Live-FAQ)
- Bitwig: [Cross-modulation](https://www.bitwig.com/learnings/cross-modulation-76/) · [The Grid](https://www.bitwig.com/the-grid/) · [What's new](https://bitwig.com/whats-new/) · [Studio 6 review](https://musictech.com/reviews/digital-audio-workstations/bitwig-studio-6-review-logic-pro-user)
- Blender: [Areas](https://docs.blender.org/manual/en/latest/interface/window_system/areas.html) · [Operator API](https://docs.blender.org/api/current/bpy.types.Operator.html) · [Keymap](https://docs.blender.org/manual/en/latest/editors/preferences/keymap.html) · [Themes](https://docs.blender.org/manual/en/latest/editors/preferences/themes.html) · [Workspaces](https://docs.blender.org/manual/en/latest/interface/window_system/workspaces.html) · [Theme extensions](https://extensions.blender.org/themes/)
- Market: [Production Expert 2025 DAW survey](https://production-expert.com/production-expert-1/2025-daw-survey-the-results) · [Why producers quit](https://weraveyou.com/2026/08/why-most-music-producers-quit/) · [Tracklib AI survey](https://www.musicbusinessworldwide.com/25-of-music-producers-are-now-using-ai-survey-says-but-a-majority-shows-strong-resistance/) · [SOS AI 2026](https://www.soundonsound.com/music-business/ai-music-tech-2026) · [Blender × Anthropic fallout](https://gamefromscratch.com/blender-x-anthropic-the-fallout/) · [VST3 MIT](https://sonicstate.com/news/2025/10/30/vst-3-now-available-under-mit-license) · [DAWproject](https://www.production-expert.com/production-expert-1/steinberg-joins-dawproject-support-can-avid-ignore-it) · [MCP DAW roundup](https://chatforest.com/reviews/music-audio-production-mcp-servers/) · [ableton-mcp](https://github.com/ahujasid/ableton-mcp) · [total-reaper-mcp](https://github.com/shiehn/total-reaper-mcp) · [Meadowlark](https://github.com/MeadowlarkDAW/Meadowlark) · [Ardour 9](https://ardour.org/whatsnew-9.html) · [Studio One → Fender Studio Pro](https://www.soundonsound.com/news/studio-one-becomes-fender-studio-pro) · [BandLab 100M](https://musicbusinessworldwide.com/music-making-app-bandlab-surpasses-100-million-users)
