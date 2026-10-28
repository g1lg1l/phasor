"""Writes the docs site's generated pages: the Manual's chapters, straight from the app
(crates/ravune/src/help.rs, so the site and Help → Manual never disagree), every command
(crates/ravune-core/src/ops.rs, what scripts and agents run), the Manual's sidebar and the
screenshots and icon.

    python scripts/site.py
    cd site && npm ci && npm run build    # VitePress → site/.vitepress/dist, what GitHub Pages serves
"""
import json
import re
import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "site" / "src"

STRING = r'"((?:[^"\\]|\\.)*)"'


def unescape(s):
    return s.replace('\\"', '"').replace("\\\\", "\\")


def chapters():
    """The Manual: (title, what it's for, [(step, how)])."""
    help_rs = (ROOT / "crates" / "ravune" / "src" / "help.rs").read_text(encoding="utf-8")
    block = help_rs[help_rs.index("const MANUAL"):help_rs.index("impl App {")]
    found = []
    for chunk in re.split(r"\n    \(\n", block)[1:]:
        title, about, *steps = [unescape(s) for s in re.findall(STRING, chunk)]
        found.append((title, about, list(zip(steps[0::2], steps[1::2]))))
    return found


def commands():
    """Every operator: (id, label, description, example props)."""
    ops = (ROOT / "crates" / "ravune-core" / "src" / "ops.rs").read_text(encoding="utf-8")
    pattern = r'id: ' + STRING + r',\s*label: ' + STRING + r',\s*description: ' + STRING + r',\s*example: (?:r#"(.*?)"#|' + STRING + r')'
    return [(m[0], unescape(m[1]), unescape(m[2]), m[3] or unescape(m[4])) for m in re.findall(pattern, ops, re.S)]


def slug(title):
    return re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")


def main():
    manual = SRC / "manual"
    shutil.rmtree(manual, ignore_errors=True)
    manual.mkdir(parents=True)
    pages = []
    for title, about, steps in chapters():
        body = f"# {title}\n\n{about}\n"
        for k, (step, how) in enumerate(steps, 1):
            body += f"\n## {k}. {step}\n\n{how}\n"
        name = f"{slug(title)}.md"
        (manual / name).write_text(body, encoding="utf-8", newline="\n")
        pages.append((title, name))

    groups = {}
    for op in commands():
        groups.setdefault(op[0].split(".")[0], []).append(op)
    body = ("# Commands\n\nEvery edit in Ravune is one of these. The command palette (Ctrl K) runs them by name, "
            "scripts with `ravune.run(id, props)`, agents with `run_operator`; each is one undo step.\n")
    for group, ops in groups.items():
        body += f"\n## {group}\n\n| Command | What it does | Example |\n|---|---|---|\n"
        for i, label, about, example in ops:
            body += f"| `{i}`<br>{label} | {about.replace('|', '\\|')} | `{example}` |\n"
    (SRC / "commands.md").write_text(body, encoding="utf-8", newline="\n")

    sidebar = "// Written by scripts/site.py: the Manual's chapters, in the app's order.\nexport default [\n"
    sidebar += "".join(f"  {{ text: {json.dumps(t)}, link: '/manual/{f[:-3]}' }},\n" for t, f in pages)
    (ROOT / "site" / ".vitepress" / "manual.mts").write_text(sidebar + "]\n", encoding="utf-8", newline="\n")

    public = SRC / "public"
    (public / "images").mkdir(parents=True, exist_ok=True)
    for shot in (ROOT / "docs" / "screenshots").glob("*.png"):
        shutil.copyfile(shot, public / "images" / shot.name)
    shutil.copyfile(ROOT / "assets" / "icon.png", public / "icon.png")
    print(f"{len(pages)} manual chapters, {sum(map(len, groups.values()))} commands")


if __name__ == "__main__":
    main()
