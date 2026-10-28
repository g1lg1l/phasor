# Scripts

Scripts are [Luau](https://luau.org), and they drive Ravune through the same
[commands](commands.md) as everything else. They run on the selected channel, from the
piano roll's Tools → Scripts or the command palette (Ctrl K).

- Put your own `.luau` (or `.lua`) files in `Documents/Ravune/Scripts`. The first line, a
  `--` comment, says what the script does in the menu.
- A run is one undo step. A script that fails halfway leaves nothing behind.
- Scripts are sandboxed (no files, no system) and stopped after two seconds.

## The `ravune` table

| Call | What it gives |
|---|---|
| `ravune.run(id, props)` | runs a command; an error stops the script |
| `ravune.notes([channel])` | the selected (or given) channel's notes in the current pattern: `{ index, start, length, key, velocity }`, in ticks |
| `ravune.pattern()` | `{ index, name, steps }` |
| `ravune.selected()` | `{ channel, slot, pattern }` |
| `ravune.scale()` | `{ root, name, keys }`: root 0 is C, keys are the scale's pitch classes from the root |
| `ravune.project()` | the project summary agents get |
| `ravune.print(text)` | shows `text` when the script ends |
| `ravune.args` | the script's arguments (built-in scripts use them) |
| `ravune.STEP`, `ravune.BEAT`, `ravune.BAR` | a 16th, a beat and a bar, in ticks |

## An example

```lua
-- Euclidean rhythm: hits spread as evenly as they go over the steps
local hits, steps = 5, 16
local channel = ravune.selected().channel
for s = 0, ravune.pattern().steps - 1 do
	local on = (s % steps) * hits % steps < hits
	ravune.run("step.set", { channel = channel, step = s, velocity = on and 100 or 0 })
end
```

Nine scripts are built in (Euclidean rhythms, arpeggios, a random bassline in key, reverse,
half and double time); the Groove Tools extension adds ghost notes, three against four and
rolling hats.
