"""Rebuilds Ravune's three fonts in crates/ravune/assets/fonts (needs `pip install fonttools`).

    python scripts/fonts.py <egui-phosphor crate dir> [<Inter variable .ttf> <JetBrainsMono variable .ttf>]
    (with only the first argument, just the icons)

- Phosphor (MIT): the regular weight, subset to the icons listed below; also writes
  crates/ravune/src/icons.rs. Add a name here, rerun, and use `icons::NAME`.
  The crate dir is egui-phosphor's source (e.g. from ~/.cargo/registry/src/*/egui-phosphor-*).
- Inter and JetBrains Mono (SIL OFL): pinned to the one weight Ravune draws (480 / 500)
  and subset to Latin, punctuation, arrows and a few symbol blocks.
"""
import os
import re
import sys

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

ICONS = """PLAY STOP METRONOME ARROW_U_UP_LEFT ARROW_U_UP_RIGHT GEAR_SIX MAGNIFYING_GLASS FILE FILE_PLUS
FOLDER FOLDER_OPEN FOLDER_PLUS FLOPPY_DISK EXPORT CARET_RIGHT CARET_DOWN CARET_LEFT CARET_UP STAR WAVEFORM
MUSIC_NOTES MUSIC_NOTE PIANO_KEYS FADERS FADERS_HORIZONTAL PLUGS SQUARES_FOUR WAVE_SINE WAVE_TRIANGLE
WAVE_SAWTOOTH WAVE_SQUARE POWER X PLUS MINUS COPY TRASH PENCIL_SIMPLE ARROW_SQUARE_OUT ARROW_SQUARE_IN
KEYBOARD QUESTION INFO SPEAKER_HIGH SPEAKER_X HEADPHONES LIGHTNING GLOBE CUBE CPU LIST DOTS_THREE CHECK
MAGIC_WAND ARROW_COUNTER_CLOCKWISE CORNERS_OUT SIDEBAR_SIMPLE SPARKLE TIMER SHUFFLE HAND_GRABBING GRID_FOUR
ARROW_RIGHT ARROWS_LEFT_RIGHT SNOWFLAKE COMMAND DOTS_SIX_VERTICAL MAGNIFYING_GLASS_PLUS MAGNIFYING_GLASS_MINUS
SCISSORS SPEAKER_SLASH ARROW_BEND_DOWN_RIGHT CIRCLE MAGNET ARROWS_IN_LINE_VERTICAL LINK ARROW_DOWN_RIGHT
SQUARE_SPLIT_HORIZONTAL LAYOUT CLOUD CLOUD_ARROW_DOWN DOWNLOAD_SIMPLE CIRCLE_NOTCH WARNING PLUG APP_WINDOW KEY
CROSSHAIR FILE_AUDIO PAUSE FUNNEL ARROW_CLOCKWISE CURSOR ERASER PAINT_BRUSH CHART_LINE
ARROWS_MERGE SPEAKER_HIFI PLUGS_CONNECTED EAR PLUS_CIRCLE CLOCK_COUNTER_CLOCKWISE REPEAT RECORD
GUITAR MICROPHONE MICROPHONE_STAGE VINYL_RECORD FIRE DROP PLANET HEART GHOST ROBOT WIND BELL SPIRAL MOUNTAINS PALETTE
CODE FLAG GRADUATION_CAP PUZZLE_PIECE STACK DICE_FIVE BUG ARROW_CIRCLE_UP FILE_ARROW_DOWN FILE_ARROW_UP BOOK_OPEN
LIGHTBULB ARROWS_SPLIT CHAT_CIRCLE_TEXT MUSIC_NOTES_PLUS SLIDERS PATH TREE_STRUCTURE""".split()

TEXT = [(0x20, 0x7E), (0xA0, 0xFF), (0x2000, 0x206F), (0x20AC, 0x20AC), (0x2190, 0x21FF), (0x2200, 0x22FF),
        (0x25A0, 0x25FF), (0x2600, 0x26FF)]

ROOT = os.path.join(os.path.dirname(__file__), "..")
FONTS = os.path.join(ROOT, "crates/ravune/assets/fonts")


def save_subset(font, unicodes, out, features=()):
    opts = subset.Options()
    opts.layout_features = list(features)
    opts.hinting = False
    opts.notdef_outline = True
    opts.name_IDs = ["*"]
    sub = subset.Subsetter(opts)
    sub.populate(unicodes=unicodes)
    sub.subset(font)
    font.save(out)
    print(f"{os.path.basename(out)}: {os.path.getsize(out) // 1024} KB")


def main(phosphor_dir, inter=None, jetbrains=None):
    table = dict(re.findall(r'pub const (\w+): &str = "\\u\{([0-9A-F]+)\}";',
                            open(os.path.join(phosphor_dir, "src/variants/codepoints.rs"), encoding="utf-8").read()))
    names = list(dict.fromkeys(ICONS))
    missing = [n for n in names if n not in table]
    if missing:
        sys.exit(f"not in Phosphor: {missing}")
    lines = ["//! Phosphor icons (MIT, phosphoricons.com), the regular weight, subset to the glyphs below.",
             "//! Regenerate with scripts/fonts.py after adding a name there (docs/THEMING.md §8).",
             "#![allow(dead_code)]", ""]
    lines += [f'pub const {n}: &str = "\\u{{{table[n]}}}";' for n in sorted(names)]
    open(os.path.join(ROOT, "crates/ravune/src/icons.rs"), "w", encoding="utf-8", newline="\n").write("\n".join(lines) + "\n")
    save_subset(TTFont(os.path.join(phosphor_dir, "res/Phosphor.ttf")), [int(table[n], 16) for n in names],
                os.path.join(FONTS, "Phosphor.ttf"))

    if not (inter and jetbrains):
        return  # icons only
    text = [c for a, b in TEXT for c in range(a, b + 1)]
    for src, out, pin in [(inter, "Inter.ttf", {"wght": 480, "opsz": 14}), (jetbrains, "JetBrainsMono.ttf", {"wght": 500})]:
        font = TTFont(src)
        axes = [a.axisTag for a in font["fvar"].axes] if "fvar" in font else []
        if axes:
            font = instancer.instantiateVariableFont(font, {k: v for k, v in pin.items() if k in axes})
        save_subset(font, text, os.path.join(FONTS, out), ["kern", "liga", "calt", "tnum"])


if __name__ == "__main__":
    if len(sys.argv) not in (2, 4):
        sys.exit(__doc__)
    main(*sys.argv[1:])
