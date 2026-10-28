# Scripts and agents

Let code or an AI assistant do the repetitive part: every change undoes in one step.

## 1. Scripts

Luau scripts run on the selected channel: from the piano roll's Tools → Scripts or the command palette (Ctrl K). Put your own .luau files in Documents/Ravune/Scripts.

## 2. Write one

A script calls ravune.run(command, props) with any command the palette lists, and reads ravune.notes(), ravune.pattern(), ravune.scale(). It stops after 2 seconds; a failed run leaves nothing behind.

## 3. Agents

Preferences → Agents lets Claude and other MCP clients search commands, edit the project, find sounds and listen to a render. It's off until you turn it on.

## 4. Chat

Ctrl I opens the agent chat on the right, full height. Write what you want and Claude Code does it in your project: it runs your own Claude Code (installed, and signed in once in a terminal) with nothing but Ravune's commands, and each reply undoes in one step.

## 5. Undo what it did

Everything an agent changes between your own edits is one undo step: Ctrl Z, and it's gone. The log in Preferences → Agents shows every call.
