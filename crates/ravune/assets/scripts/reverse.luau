-- The selected channel's notes, back to front.
local channel = ravune.selected().channel
local length = ravune.pattern().steps * ravune.STEP
local notes = ravune.notes()
ravune.run("notes.clear", { channel = channel })
for _, n in ipairs(notes) do
	ravune.run("note.add", { channel = channel, key = n.key, start = math.max(0, length - n.start - n.length), length = n.length, velocity = n.velocity })
end
