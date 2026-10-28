-- Soft ghost hits a 16th before each hit on the selected channel, where nothing plays yet.
local channel = ravune.selected().channel
local notes = ravune.notes()
local length = ravune.pattern().steps * ravune.STEP
local taken = {}
for _, n in ipairs(notes) do
	taken[n.start] = true
end
for _, n in ipairs(notes) do
	local at = (n.start - ravune.STEP) % length
	if not taken[at] and n.velocity > 60 then
		taken[at] = true
		ravune.run("note.add", { channel = channel, key = n.key, start = at, length = n.length, velocity = math.max(20, n.velocity // 3) })
	end
end
