-- Chords become arpeggios: each chord on the selected channel (notes starting together)
-- plays its keys one after another, every `rate` ticks, for as long as the chord lasted.
local rate = ravune.args.rate or ravune.STEP
local updown = ravune.args.direction == "updown"
local channel = ravune.selected().channel
local notes = ravune.notes()
local chords, starts = {}, {}
for _, n in ipairs(notes) do
	if not chords[n.start] then
		chords[n.start] = {}
		table.insert(starts, n.start)
	end
	table.insert(chords[n.start], n)
end
ravune.run("notes.clear", { channel = channel })
local arpeggiated = 0
for _, start in ipairs(starts) do
	local chord = chords[start]
	table.sort(chord, function(a, b) return a.key < b.key end)
	if #chord == 1 then
		local n = chord[1]
		ravune.run("note.add", { channel = channel, key = n.key, start = n.start, length = n.length, velocity = n.velocity })
	else
		local order = table.clone(chord)
		if updown then
			for i = #chord - 1, 2, -1 do table.insert(order, chord[i]) end
		end
		local length = chord[1].length
		for _, n in ipairs(chord) do length = math.max(length, n.length) end
		local t, i = 0, 0
		while t < length do
			local n = order[i % #order + 1]
			ravune.run("note.add", { channel = channel, key = n.key, start = start + t, length = math.min(rate, length - t), velocity = n.velocity })
			t, i = t + rate, i + 1
		end
		arpeggiated += 1
	end
end
ravune.print(`{arpeggiated} chord(s) arpeggiated`)
