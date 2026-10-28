# Ravune: Themes & Layout (design draft)

> Draft for discussion: we'll iterate on this calmly before M5. The *foundation* (no hardcoded look) starts in M1.
>
> **Direction (decided 2026-10-24):** serious and precise, Ableton-like: neutral greys, one amber accent, near-square corners, monospace readouts, no bounce. The first "playful candy" look was rejected as too much.

## Goals
1. **Calm, precise default.** An instrument, not a toy: dense, flat, readable, with motion only where it carries information (meters, playhead, held keys).
2. **Theme every pixel** (Blender-grade granularity) without making simple themes hard: change one accent color and get a coherent theme.
3. **Rearrange every part** (areas, editors, panels, toolbars) and save it as a workspace.
4. **Share in one click**: themes, workspaces and keymaps are plain files in one package format.
5. **Accessible**: contrast checks, UI scale, color-blind-safe palettes, reduced motion.

## What we learned
| App | Good | Bad |
|---|---|---|
| Blender | Per-editor, per-widget colors; live preview; XML presets; 230 community themes | Hundreds of flat fields, no semantic layer → most themes are half-finished |
| Reaper | Huge theme culture (Stash), WALTER layout language, themes can even move controls | Ugly default; theme authoring is arcane |
| FL Studio | Theme editor in-app, hue/saturation shifting | Thin docs; limited granularity |
| VS Code / Zed | Semantic tokens + per-component overrides, JSON themes, huge galleries | (the model to copy for the token layer) |

**Ravune's answer:** a **4-layer token cascade** (semantic first, granular when you want) + **shape & motion tokens** + **layout files**.

## 1. Token cascade

```
Palette  →  Semantic tokens  →  Component tokens  →  Editor overrides
(raw)       (meaning)           (widget parts)       (per area type)
```

1. **Palette**: named raw colors. A theme can be just a palette + "derive everything".
2. **Semantic**: `bg.base`, `bg.raised`, `bg.sunken`, `fg.text`, `fg.muted`, `accent`, `accent.alt`, `ok`, `warn`, `danger`, `selection`, `focus`, `playhead`, `grid.bar`, `grid.beat`, `grid.step`. Defaults derive from the palette (OKLCH lightness steps).
3. **Component**: `knob.track`, `knob.fill`, `knob.cap`, `button.bg.hover`, `meter.low/mid/high/peak`, `clip.header`, `note.body`, `note.selected`, `piano.key.white/black`, `step.on/off/accent`, `cable.audio/midi/mod`, … Each defaults to a semantic token.
4. **Editor overrides**: `[editor.piano_roll]`, `[editor.mixer]`, … override any token only inside that editor (Blender's model).

Unset tokens fall through to the layer above, so a 5-line theme is complete and a 500-line theme is precise.

### Clip/track colors
16-color clip palette per theme, auto-assigned round-robin, with a color-blind-safe alternative set.

## 2. Shape, type & motion tokens
| Group | Tokens |
|---|---|
| Shape | `radius.sm/md/lg`, `stroke.thin/thick`, `knob.style` (`arc`, `flat`, `skeuo`, `dot`), `clip.style` (`rounded`, `square`, `pill`) |
| Spacing | `space.unit` (4 px base) and scale, `density` (`compact`, `cozy`, `comfy`) |
| Type | `font.ui` (Inter), `font.mono` (JetBrains Mono, for every value and readout), sizes; bundled OFL fonts |
| Motion | `duration.fast/normal` for fades (held keys, hover), `reduce_motion`. No springs or overshoot by default. |
| Effects | `shadow.popup` only; optional `glow.accent` for meters/playhead |

## 3. Theme file format
> Shipped in M5: `.ravune-theme` files in RON with every token flat (`accent: "#ffa545"`, sizes as numbers); a file with only some tokens takes the rest from Ravune Dark. The TOML design below (palettes, `extends`, per-editor overrides, hot reload) is the direction for when themes grow.

TOML, human-editable, hot-reloaded on save (live preview while you edit in any text editor).

```toml
[meta]
name = "Cold Steel"
author = "you"
extends = "ravune-dark"   # inherit everything, override what you want
version = 1

[palette]
graphite = "#16181c"
ice      = "#7fd4ff"
signal   = "#ff5c5c"

[semantic]
accent  = "ice"
danger  = "signal"
bg.base = "graphite"

[shape]
knob.style = "arc"
radius.md  = 2

[editor.piano_roll]
note.body = "ice"
```

## 4. Theme editor (M5)
> Shipped: Preferences → Look lists the themes with swatches; the editor window edits every color and size live, saves a copy (built-ins are never overwritten), imports and exports. The generators, contrast checker and eyedropper below are still ideas.
- Live preview on a **specimen board** (every widget in every state) plus the real UI.
- **One-color generator:** pick an accent → full theme (OKLCH harmonies).
- **From image:** drop a picture (album art!) → palette extraction → theme.
- **Contrast checker:** WCAG warnings per token pair (text on bg, notes on grid).
- **Eyedropper inspector:** Alt-click any pixel in the UI → shows which token painted it → edit it there.
- Export as a package; import by drag & drop.

## 5. Layout customization (Blender areas)
> Shipped in M5: workspaces as tabs in the top bar (Beat, Compose, Sound, Mix + your own), each remembering its panels, docked / split / floating editors and browser tab; a split editor area. The free area tree below is the longer-term plan.
- The window is a **tree of areas** (egui_tiles): split by dragging a corner or via menu, join, swap, maximize (Ctrl+Space), tear off into a new OS window (multi-monitor).
- Every area has a header with an **editor-type switcher**: Playlist, Channel Rack, Piano Roll, Mixer, Devices, Browser, Automation, Info, Command Log, Assistant, Patcher (later).
- Per-area state (zoom, snap, scroll, filters) is saved with the layout.
- **Workspaces** = named layouts in tabs on top: *Beat*, *Compose*, *Sound Design*, *Mix*, *Learn*. Users add their own.
- Toolbars and panels are themselves configurable (show/hide, reorder, add operator buttons: any operator can become a button).
- Files: `*.ravune-workspace` (RON). Stored in user prefs and optionally inside the project.

## 6. Packages
A package is a folder (or zip) with a `ravune-package.toml` manifest; it can contain any mix of:
`themes/`, `workspaces/`, `keymaps/`, `icons/` (SVG), `fonts/` (OFL only), `scripts/` (Lua), `devices/`, `sounds/`.
Same format later feeds the extension gallery (M10).

## 7. Built-in themes (first ideas)
| Name | Vibe |
|---|---|
| **Ravune Dark** (default, shipped in M1) | Neutral greys, amber accent, 2–3 px corners, mono readouts |
| **Ravune Light** | Ableton "Mid Light"-style grey for bright rooms |
| **Scope** (shipped as "Scope") | Near-black with phosphor-green accent |
| **High Contrast** | Accessibility first |

## 8. Engineering rules (from M1)
- All drawing goes through a `Theme` handle; **no color/size literals outside `theme/`** (CI grep check).
- Tokens are resolved once per theme change into a flat struct (zero cost per frame).
- Custom widgets (knob, fader, step, note, clip) take tokens, never raw colors.
- egui `Visuals`/`Style` are *generated* from our tokens, not edited directly.
- Layout (M5): a region is fixed-height bars (`kit::bar`) over one edge-to-edge scroll area (`kit::scroll`), split by 1-px dividers. Space is padding inside elements, never outer margins, so scroll bars sit on the dividers. Spacing tokens: `pad` 8, `gap` 4, `bar_h` 32, `row_h` 22.
- Motion (M5): one `motion` token (0.16 s) drives every transition; "Reduce motion" sets it to 0 and the sparks, pops and cascades switch off. Animations repaint only while they run.
- Fonts and icons: Inter (UI, weight 480), JetBrains Mono (values, 500) and Phosphor (icons, MIT), each pinned to one weight and subset to the glyphs used (~130 KB together); egui's fallback fonts aren't shipped. To add an icon: put its name in `scripts/fonts.py`, run `python scripts/fonts.py <egui-phosphor dir> <Inter variable ttf> <JetBrains Mono variable ttf>`, use `icons::NAME`.

## Open questions (to discuss)
1. Logo: the wordmark + rotating ravune glyph is a placeholder; a proper mark comes later.
2. Do themes get to move controls (Reaper WALTER-style) or only restyle them? Proposal: restyle only; layout belongs to workspaces.
3. Animated background shaders (proposal): not in built-in themes; allowed for community themes.
4. Should a project be able to carry its own theme (e.g. a template that looks different)? Proposal: yes, optional and overridable by the user.
