-- Double or half time: every note's start and length times `factor` (2 = half time, the
-- pattern grows to fit; 0.5 = double time).
local factor = ravune.args.factor or 2
local channel = ravune.selected().channel
local steps = ravune.pattern().steps
local notes = ravune.notes()
if factor > 1 then
	ravune.run("pattern.set_length", { steps = math.min(512, math.ceil(steps * factor)) })
end
ravune.run("notes.clear", { channel = channel })
for _, n in ipairs(notes) do
	local start = math.floor(n.start * factor)
	if start < ravune.pattern().steps * ravune.STEP then
		ravune.run("note.add", { channel = channel, key = n.key, start = start, length = math.max(1, math.floor(n.length * factor)), velocity = n.velocity })
	end
end
