-- Hats on the selected channel: 16ths accented off the beat, a 32nd roll into the loop.
local channel = ravune.selected().channel
local steps = ravune.pattern().steps
ravune.run("notes.clear", { channel = channel })
for s = 0, steps - 1 do
	local v = ({ 70, 45, 100, 50 })[s % 4 + 1]
	ravune.run("note.add", { channel = channel, key = 60, start = s * ravune.STEP, length = ravune.STEP, velocity = v })
	if s >= steps - 4 then
		ravune.run("note.add", { channel = channel, key = 60, start = s * ravune.STEP + ravune.STEP // 2, length = ravune.STEP // 2, velocity = v - 15 })
	end
end
