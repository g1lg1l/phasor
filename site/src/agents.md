# Agents (MCP)

Ravune speaks the [Model Context Protocol](https://modelcontextprotocol.io), so Claude or
any other MCP client can make music in it with you. It's off until you turn it on.

## The chat inside Ravune

`Ctrl I` (or the robot at the top right) opens a chat on the right, the full height of the
window. Write what you want ("make the kick punchier", "add a reverb bus for the snare") and
Claude does it in the open project while you listen.

The chat runs the [Claude Code](https://claude.com/claude-code) you have installed, in the
background, with nothing but Ravune's commands: no files, no shell, none of your own
settings or MCP servers. Sign in once by running `claude` in a terminal. Each reply undoes
in one step, and the chat turns agents on by itself the first time you send something.

## Use it from your own client

1. In Ravune, Preferences → Agents → *Let agents use Ravune*. Choose whether agents may
   change the project and save it.
2. Add the bridge that ships next to the program to your client. In Claude Code:

```bash
claude mcp add ravune -- /path/to/ravune-mcp
```

The bridge talks to the running Ravune on `127.0.0.1` (port 7317 by default) with a token
Ravune writes into its data folder, so nothing else on the machine gets in.

## What an agent can do

| Tool | What it does |
|---|---|
| `search_operators` | finds commands by words in their id, label or description |
| `run_operator` | runs one command with its props |
| `query_project` | reads the project: a summary, a pattern's notes, the playlist, or all of it |
| `browse_sounds` | searches the sound library |
| `render_and_analyze` | renders some bars offline and describes them: levels, energy per band, stereo width, onsets, key, tempo |
| `checkpoint` | closes the current undo step |
| `undo` | takes back the last one |
| `save_project` | saves (only when you allowed it) |

Everything an agent changes between your own edits is one undo step: Ctrl Z, and it's
gone. Preferences → Agents keeps a log of every call.
