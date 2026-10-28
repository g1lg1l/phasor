"""Packs the extension gallery: each folder in gallery/src becomes gallery/<folder>.ravunext,
and gallery/index.json lists them (what Preferences → Extensions → Gallery shows).

    python scripts/gallery.py

An extension folder holds extension.json (name, version, author, about) and any of themes/,
scripts/, templates/ and sounds/. The zips are byte-for-byte the same on every run.
"""
import json
import os
import zipfile

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "gallery")
URL = "https://raw.githubusercontent.com/g1lg1l/ravune/main/gallery/"

index = []
for folder in sorted(os.listdir(os.path.join(ROOT, "src"))):
    src = os.path.join(ROOT, "src", folder)
    manifest = json.load(open(os.path.join(src, "extension.json"), encoding="utf-8"))
    package = f"{folder}.ravunext"
    with zipfile.ZipFile(os.path.join(ROOT, package), "w", zipfile.ZIP_DEFLATED) as z:
        for base, dirs, files in os.walk(src):
            dirs.sort()
            for name in sorted(files):
                path = os.path.join(base, name)
                info = zipfile.ZipInfo(os.path.relpath(path, src).replace(os.sep, "/"), (2026, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                z.writestr(info, open(path, "rb").read())
    index.append({**manifest, "url": URL + package})
    print("packed", package)
with open(os.path.join(ROOT, "index.json"), "w", encoding="utf-8", newline="\n") as f:
    json.dump(index, f, indent=2, ensure_ascii=False)
    f.write("\n")
