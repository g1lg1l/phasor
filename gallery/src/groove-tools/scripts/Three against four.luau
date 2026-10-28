-- A hit every three 16ths on the selected channel: three against the kick's four.
local channel = ravune.selected().channel
for s = 0, ravune.pattern().steps - 1 do
	ravune.run("step.set", { channel = channel, step = s, velocity = s % 3 == 0 and (s % 12 == 0 and 110 or 90) or 0 })
end
