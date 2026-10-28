"""Retakes the README and docs screenshots (needs `pip install pillow` and a release build).

    cargo build --release -p ravune -p ravune-mcp
    python scripts/screenshots.py

Each shot runs Ravune once on the Techno template, with a throwaway profile that sets the
layout and lets agents in; ravune-mcp selects a channel and pattern the way an agent would,
and RAVUNE_CAPTURE_TO saves the window. They land in docs/screenshots at 1440 x 900, with
waveforms.png: the Loop lane in each waveform style
(scripts/site.py copies them to the docs site).
"""
import json
import os
import subprocess
import sys
import tempfile
import time

from PIL import Image

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
BIN = os.path.join(ROOT, "target", "release")
EXE = ".exe" if sys.platform == "win32" else ""
SONG = os.path.join(ROOT, "assets", "templates", "techno.ravune")

# file, workspace (the layout), (channel, pattern) to select
SHOTS = [
    ("ravune", '(name:"Beat",browser:true,browser_tab:Sounds,editor:true,keys:false,tab:Mixer,split:None,floating:[])', None),
    ("pianoroll", '(name:"Compose",browser:false,browser_tab:Sounds,editor:true,keys:false,tab:PianoRoll,split:None,floating:[])', (7, 2)),
    ("device", '(name:"Sound",browser:true,browser_tab:Instruments,editor:true,keys:false,tab:Rack,split:None,floating:[])', (6, 1)),
]


def select(data, channel, pattern):
    """channel.select and pattern.select through ravune-mcp."""
    mcp = subprocess.Popen([os.path.join(BIN, "ravune-mcp" + EXE)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env={**os.environ, "RAVUNE_DATA": data}, text=True)
    calls = [("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "screenshots", "version": "1"}})]
    calls += [("tools/call", {"name": "run_operator", "arguments": {"id": op, "props": props}}) for op, props in (("channel.select", {"channel": channel}), ("pattern.select", {"pattern": pattern}))]
    for i, (method, params) in enumerate(calls):
        mcp.stdin.write(json.dumps({"jsonrpc": "2.0", "id": i, "method": method, "params": params}) + "\n")
        mcp.stdin.flush()
        reply = json.loads(mcp.stdout.readline())
        if "error" in reply or reply.get("result", {}).get("isError"):
            sys.exit(f"{method}: {reply}")
    mcp.stdin.close()
    mcp.wait(5)


def shoot(workspace, selection=None, extra=""):
    """One run of Ravune: what its window showed, at the screen's own pixels."""
    data = tempfile.mkdtemp(prefix="ravune-shot-")
    settings = f"(check_updates:false,agent:(enabled:true,port:7318),workspaces:[{workspace}],workspace:0{extra})"
    with open(os.path.join(data, "app.ron"), "w", encoding="utf-8") as f:
        json.dump({"settings": settings}, f)
    raw = os.path.join(data, "shot.png")
    env = {**os.environ, "RAVUNE_DATA": data, "RAVUNE_CAPTURE": "1", "RAVUNE_CAPTURE_TO": raw}
    app = subprocess.Popen([os.path.join(BIN, "ravune" + EXE), SONG], env=env)
    if selection:
        time.sleep(2.5)  # the agent server is up; the capture waits 5 s
        select(data, *selection)
    app.wait(30)
    return Image.open(raw).convert("RGB")


def save(image, name):
    out = os.path.join(ROOT, "docs", "screenshots", f"{name}.png")
    image.save(out, optimize=True)
    print("wrote", os.path.relpath(out, ROOT))


def main():
    for name, workspace, selection in SHOTS:
        save(shoot(workspace, selection).resize((1440, 900), Image.LANCZOS), name)
    # the Loop lane's audio clips in each waveform style (Preferences → Look), one above the other
    lane = (376, 168, 952, 222)  # at 1440 x 900
    shots = [shoot(SHOTS[0][1], extra=f",wave:{style}") for style in ("Mono", "Rgb", "ThreeBand")]
    k = shots[0].width / 1440
    strips = [s.crop(tuple(round(v * k) for v in lane)) for s in shots]
    w, h = strips[0].size
    waves = Image.new("RGB", (w, 3 * h + 2 * 6), (23, 23, 23))
    for i, strip in enumerate(strips):
        waves.paste(strip, (0, i * (h + 6)))
    save(waves, "waveforms")


if __name__ == "__main__":
    main()
