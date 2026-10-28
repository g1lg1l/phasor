-- Euclidean rhythm: `hits` spread as evenly as they go over `steps`, repeated across the
-- pattern on the selected channel (first hit on the one).
local hits, steps = ravune.args.hits or 5, ravune.args.steps or 16
local channel = ravune.selected().channel
for s = 0, ravune.pattern().steps - 1 do
	local on = (s % steps) * hits % steps < hits
	ravune.run("step.set", { channel = channel, step = s, velocity = on and (s % steps == 0 and 120 or 100) or 0 })
end
