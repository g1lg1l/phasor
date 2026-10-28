-- A new bassline on the selected channel: notes of the song's scale, mostly off the beat
-- (where the kick isn't), around the root an octave or two down. Run it again for another.
local channel = ravune.selected().channel
local scale = ravune.scale()
local steps = ravune.pattern().steps
math.randomseed(math.floor(os.clock() * 1e6))
ravune.run("notes.clear", { channel = channel })
local root = 36 + scale.root
if root > 41 then root -= 12 end
for s = 0, steps - 1 do
	if s % 4 ~= 0 and math.random() < 0.42 then
		local degree = scale.keys[math.random(1, #scale.keys)]
		local key = root + degree + (math.random() < 0.15 and 12 or 0)
		local length = math.random() < 0.25 and 2 * ravune.STEP or ravune.STEP * 3 // 4
		ravune.run("note.add", { channel = channel, key = key, start = s * ravune.STEP, length = length, velocity = math.random(85, 120) })
	end
end
