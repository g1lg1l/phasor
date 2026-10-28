# Commands

Every edit in Ravune is one of these. The command palette (Ctrl K) runs them by name, scripts with `ravune.run(id, props)`, agents with `run_operator`; each is one undo step.

## transport

| Command | What it does | Example |
|---|---|---|
| `transport.play_toggle`<br>Play / Stop | Start or stop playback (the pattern in PAT mode, the playlist in SONG mode). | `{}` |
| `transport.mode_toggle`<br>Pattern / Song Mode | Switch between looping the current pattern and playing the playlist. | `{}` |
| `transport.set_position`<br>Set Song Position | Where song playback starts, in ticks (96 per beat). | `{"tick": 384}` |
| `transport.loop`<br>Loop Region | Play only part of the song, over and over: from and to in ticks (96 per beat) set the region and turn it on; on switches it on or off (the region stays); clear drops it. | `{"from": 0, "to": 1536}` |
| `transport.jump`<br>Jump Here | Move the song position (ticks) and, if the song is playing, the playhead with it. | `{"tick": 384}` |
| `transport.set_tempo`<br>Set Tempo | Set the song tempo in beats per minute (20–300). | `{"bpm": 120}` |
| `transport.metronome_toggle`<br>Toggle Metronome | Turn the click track on or off while playing. | `{}` |
| `transport.set_swing`<br>Set Swing | Delay every second 16th step for a shuffled groove (0–100%). | `{"amount": 0.3}` |

## song

| Command | What it does | Example |
|---|---|---|
| `song.set_key`<br>Set Key | Song key: root 0–11 (0 = C) and scale (major, minor, dorian, …). | `{"root": 9, "scale": "minor"}` |
| `song.structure`<br>Build a Song From the Patterns | Lays the patterns out as a song after the end of the playlist: intro, build, drop, break, drop 2, outro, in 8-bar phrases, each with a marker. Kick and drums start, the bass joins, the break drops the kick, the outro thins out. | `{}` |

## pattern

| Command | What it does | Example |
|---|---|---|
| `pattern.add`<br>New Pattern | Add an empty pattern and select it. | `{}` |
| `pattern.duplicate`<br>Clone Pattern | Copy the current pattern (its notes and steps) and select the copy. | `{}` |
| `pattern.select`<br>Select Pattern | Show a pattern in the channel rack and piano roll. | `{"pattern": 1}` |
| `pattern.remove`<br>Delete Pattern | Delete a pattern and its clips from the playlist (there is always one left). | `{}` |
| `pattern.rename`<br>Rename Pattern | Give a pattern a name, e.g. "Chorus drums". | `{"name": "Intro"}` |
| `pattern.set_color`<br>Set Pattern Color | Color a pattern (RGB, 0–255 each); null goes back to its palette color. | `{"color": [232, 106, 60]}` |
| `pattern.set_length`<br>Set Pattern Length | Pattern length in 16th steps (1–512). Its clips on the playlist that were exactly one pattern long follow. | `{"steps": 32}` |
| `pattern.automation`<br>Pattern Automation | A knob's line inside a pattern (default: the current one); it plays wherever the pattern does. Points are [tick from the pattern's start, 0–1 along the knob]; curves bend each stretch between two points (-1..1, 0 straight). remove: true drops the line. | `{"target": {"type": "track_gain", "track": 1}, "points": [[0, 0.2], [384, 0.8]]}` |
| `pattern.clear`<br>Clear Pattern | Remove every step and note from the current pattern. | `{}` |

## step

| Command | What it does | Example |
|---|---|---|
| `step.toggle`<br>Toggle Step | Turn a step on or off. Channel and step are 0-based; the current pattern unless given. | `{"channel": 0, "step": 4}` |
| `step.set`<br>Set Step | Set a step's velocity (0 = off, 1–127 = on). | `{"channel": 0, "step": 4, "velocity": 100}` |

## note

| Command | What it does | Example |
|---|---|---|
| `note.add`<br>Add Note | Draw a note in the piano roll: key (60 = C4), start and length in ticks (96 per beat). | `{"key": 60, "start": 0, "length": 24}` |
| `note.set`<br>Edit Note | Move, resize or change a note (by its index in the channel's notes): key, start, length, velocity, fine pitch (-2..2 semitones), brightness (0..1). | `{"index": 0, "start": 48, "key": 62}` |
| `note.remove`<br>Delete Note | Delete a note by its index in the channel's notes. | `{"index": 0}` |

## notes

| Command | What it does | Example |
|---|---|---|
| `notes.quantize`<br>Quantize Notes | Snap note starts to a grid in ticks (default 24 = 1/16). | `{}` |
| `notes.chord`<br>Stamp Chord | Draw a chord (Major, Minor, 7, Maj7, Min7, Sus2, Sus4, Dim, Aug, Power) from a root key. | `{"key": 60, "start": 0, "length": 96, "chord": "Minor"}` |
| `notes.clear`<br>Clear Notes | Remove a channel's steps and notes from a pattern. | `{"channel": 0}` |
| `notes.legato`<br>Legato | Stretch each note of a channel up to the next one (the last to the pattern's end). | `{}` |
| `notes.humanize`<br>Humanize | Nudge a channel's notes a little off the grid and vary their velocity (amount 0–1). | `{"amount": 0.3}` |
| `notes.strum`<br>Strum | Spread the notes of each chord out in time, low to high, like a strummed guitar. | `{"ticks": 6}` |

## clip

| Command | What it does | Example |
|---|---|---|
| `clip.add`<br>Add Clip | Place a pattern (default: the current one), an audio sample or an automation clip (a knob's target and its points) on the playlist. | `{"lane": 0, "start": 0}` |
| `clip.set`<br>Move Clip | Move, resize or trim a playlist clip (by index), or give an audio clip another sound (sample: a file). | `{"index": 0, "start": 384}` |
| `clip.remove`<br>Delete Clip | Remove a clip from the playlist. | `{"index": 0}` |
| `clip.duplicate`<br>Duplicate Clip | Put a copy of a clip right after it on the same lane. | `{"index": 0}` |
| `clip.split`<br>Split Clip | Cut a clip in two at a song position (in ticks). | `{"index": 0, "at": 384}` |
| `clip.set_fx`<br>Clip Properties | A clip's gain (dB, -60 to +12), pitch (semitones, -24 to 24), fade in / out (ticks), mute, the mixer track it plays into, and how it follows the tempo: stretch (off, resample, stretch) from the sound's own bpm (20 to 400; 0 = not known), and looped (the sound repeats under the clip). | `{"index": 0, "gain": -3, "fade_in": 96}` |
| `clip.envelope`<br>Clip Envelope | Add, change or remove a clip envelope (volume, pan, filter). Points are [tick from where the clip's content starts, value 0–1]; curves bend each stretch between two points (-1..1, 0 straight). | `{"index": 0, "target": "filter", "points": [[0, 0.1], [384, 0.5]]}` |
| `clip.automation`<br>Automation Points | An automation clip's points: [tick from where the clip's content starts, 0–1 along the knob's travel]; curves bend each stretch between two points (-1..1, 0 straight). | `{"index": 0, "points": [[0, 0.2], [1536, 0.9]]}` |
| `clip.make_unique`<br>Make Unique | Give a pattern clip its own copy of the pattern, so editing it changes only this clip. | `{"index": 0}` |

## lane

| Command | What it does | Example |
|---|---|---|
| `lane.set`<br>Lane Settings | Name, color, mute or solo a playlist lane (0-based). | `{"lane": 0, "name": "Drums", "solo": true}` |
| `lane.insert`<br>Insert Lane | Open an empty lane: clips on it and below move down one. | `{"lane": 2}` |
| `lane.remove`<br>Delete Lane | Delete a lane with its clips; the lanes below move up one. | `{"lane": 2}` |

## marker

| Command | What it does | Example |
|---|---|---|
| `marker.add`<br>Add Marker | Put a named marker on the timeline (tick, default: the song position; name, default: Marker N). The ruler shows it; a click there jumps to it. | `{}` |
| `marker.set`<br>Edit Marker | Move (tick) or rename (name) a marker, by its index in time order. | `{"index": 0, "name": "Drop"}` |
| `marker.remove`<br>Delete Marker | Take a marker off the timeline, by its index in time order. | `{"index": 0}` |

## steps

| Command | What it does | Example |
|---|---|---|
| `steps.fill`<br>Fill Each N Steps | Replace a channel's steps with one every N steps (4 = each beat). | `{"channel": 0, "every": 4}` |
| `steps.rotate`<br>Rotate | Shift a channel's notes by some steps, wrapping around the pattern (negative = left). | `{"channel": 0, "by": 1}` |

## channel

| Command | What it does | Example |
|---|---|---|
| `channel.solo`<br>Solo Channel | Mute every other channel; again to unmute them all. | `{"channel": 0}` |
| `channel.clone`<br>Clone Channel | A copy of a channel (instrument, settings and its notes in every pattern) right below it. | `{"channel": 0}` |
| `channel.move`<br>Move Channel | Move a channel up (negative) or down in the rack. | `{"channel": 2, "by": -1}` |
| `channel.add`<br>Add Channel | Add a channel: a sampler for a sample path, or a VA-1 synth (optionally from a preset, e.g. "Acid Bass"). | `{}` |
| `channel.replace`<br>Replace Instrument | Swap a channel's instrument (a sample, a VA-1 preset or a plugin) and keep its notes, mixer track and level. | `{"channel": 0, "preset": "Acid Bass"}` |
| `channel.remove`<br>Remove Channel | Delete a channel with its steps and notes in every pattern (the selected one by default). | `{}` |
| `channel.select`<br>Select Channel | Choose which channel the keyboard plays and the device and piano roll show. | `{"channel": 1}` |
| `channel.set`<br>Edit Channel | Change a channel's name, gain (0–1), mute, pitch (semitones), choke group, sample, attack and release (seconds, samplers), start and end (the part of the sound a sampler plays, 0–1 of its length), looped (true: that part repeats while the note is held), or swing (0–1; negative follows the song's). | `{"channel": 0, "gain": 0.7}` |

## mixer

| Command | What it does | Example |
|---|---|---|
| `mixer.add_track`<br>Add Mixer Track | Add a mixer track (routed to the master); hidden: true makes it a ghost, out of the mixer unless ghosts are shown. | `{}` |
| `mixer.remove_track`<br>Remove Mixer Track | Delete a mixer track; whatever played into it goes to the master. | `{"track": 1}` |
| `mixer.set`<br>Edit Mixer Track | Change a track's name, gain (0–1.25), pan (-1–1), EQ (low, mid, high: dB, -15–15; low shelf 120 Hz, mid 1 kHz, high shelf 6 kHz), mute, solo, route (the track it feeds), color ([r, g, b]; auto_color: true for the palette's), icon (a name; "" for automatic) or hidden (a ghost track, shown only with the mixer's ghosts). | `{"track": 1, "gain": 0.8}` |
| `mixer.set_send`<br>Set Send | Send part of a track to another track (e.g. a reverb bus). Amount 0 removes the send. | `{"track": 1, "to": 3, "amount": 0.3}` |
| `mixer.duck`<br>Duck To (Sidechain) | Make a track duck under another one (e.g. the bass under the kick): adds a compressor keyed by the source track. | `{"track": 2, "source": 1}` |

## device

| Command | What it does | Example |
|---|---|---|
| `device.add`<br>Add Effect | Add an effect to a mixer track: eq, filter, compressor, distortion, chorus, delay, reverb or limiter. | `{"track": 0, "kind": "reverb"}` |
| `device.remove`<br>Remove Effect | Delete an effect (by id) and its modulation. | `{"id": 1}` |
| `device.move`<br>Move Effect | Move an effect to another position in its chain. | `{"id": 1, "index": 0}` |
| `device.set`<br>Set Effect Parameter | Set one effect parameter (by effect id and parameter index). | `{"id": 1, "param": 0, "value": 0.5}` |
| `device.sidechain`<br>Set Sidechain | Let another track drive a compressor (e.g. the kick ducking the bass). Leave out "source" to switch it off. | `{"id": 3, "source": 1}` |
| `device.style`<br>Rename Effect | An effect's name ("" for its kind's), color ([r, g, b]; auto_color: true for none) and icon (a name; "" for its kind's). | `{"id": 1, "name": "Space"}` |
| `device.bypass`<br>Bypass Effect | Switch an effect off or on without removing it. | `{"id": 1}` |

## midi

| Command | What it does | Example |
|---|---|---|
| `midi.learn`<br>Map MIDI Controller | Make a controller knob (MIDI CC, channel 0-15) move a parameter. Replaces that controller's and that parameter's old mapping. | `{"channel": 0, "cc": 74, "target": {"type": "track_gain", "track": 1}}` |
| `midi.forget`<br>Forget MIDI Mapping | Stop a controller from moving this parameter. | `{"target": {"type": "track_gain", "track": 1}}` |

## plugin

| Command | What it does | Example |
|---|---|---|
| `plugin.store`<br>Store Plugin State | Keep a plugin's state (base64) in the project: on a channel, or on a plugin effect by id. Not an undo step. | `{"channel": 0, "state": "AAAA"}` |

## macro

| Command | What it does | Example |
|---|---|---|
| `macro.randomize`<br>Randomize Macros | Turn every macro that moves something to a random position (try again for another). | `{}` |
| `macro.store`<br>Store Macro Variation | Keep where the macros are now as a variation, to come back to with macro.recall. | `{}` |
| `macro.recall`<br>Recall Macro Variation | Put the macros back where a stored variation had them. | `{"index": 0}` |
| `macro.forget`<br>Forget Macro Variation | Delete a stored variation. | `{"index": 0}` |
| `macro.set`<br>Set Macro | Turn a macro knob (0–1) or rename it. | `{"index": 0, "value": 0.5}` |

## lfo

| Command | What it does | Example |
|---|---|---|
| `lfo.add`<br>Add LFO | Add a low-frequency oscillator you can map onto any knob. | `{}` |
| `lfo.set`<br>Edit LFO | Change an LFO's shape (0 sine, 1 triangle, 2 saw, 3 square, 4 random), rate in Hz, tempo sync and length in beats. | `{"index": 0, "sync": true, "beats": 2}` |
| `lfo.remove`<br>Remove LFO | Delete an LFO and what it was mapped to. | `{"index": 0}` |

## mod

| Command | What it does | Example |
|---|---|---|
| `mod.map`<br>Map Modulation | Let a macro or LFO move a knob. Amount -1–1 is a share of the knob's range. | `{"source": {"type": "macro", "index": 0}, "target": {"type": "track_gain", "track": 1}, "amount": 0.5}` |
| `mod.set`<br>Set Modulation Amount | Change how far a mapping moves its knob (-1–1). | `{"index": 0, "amount": 0.25}` |
| `mod.remove`<br>Remove Modulation | Delete a mapping. | `{"index": 0}` |

## synth

| Command | What it does | Example |
|---|---|---|
| `synth.set_param`<br>Set Synth Parameter | Set one VA-1 parameter (on the selected channel unless `channel` is given). | `{"param": "cutoff", "value": 1200}` |
| `synth.reset`<br>Init Synth | Put every VA-1 parameter back to its default. | `{}` |
| `synth.surprise`<br>Randomize Synth | A random but musical VA-1 patch. | `{}` |

## edit

| Command | What it does | Example |
|---|---|---|
| `edit.undo`<br>Undo | Take back the last change. | `{}` |
| `edit.redo`<br>Redo | Bring back the change you just undid. | `{}` |
