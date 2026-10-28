# Extensions

An extension adds themes, scripts, templates and sounds. Install one from Preferences →
Extensions, from a file or from the gallery; remove it there too.

- Its **themes** join the list in Preferences → Look.
- Its **scripts** show up in the palette and the piano roll's Tools → Scripts.
- Its **templates** in File → New from Template.
- Its **sounds** in the browser, in a folder named after it.

## Make one

A `.ravunext` file is a zip:

```text
extension.json
themes/Night.ravune-theme
scripts/Ghost notes.luau
templates/Dub Techno 118.ravune
sounds/Hits/clank.wav
```

```json
{
  "name": "Night Pack",
  "version": "1.0.0",
  "author": "you",
  "about": "One line on what it brings."
}
```

Only those four folders are unpacked, and only their own kinds of files (`.ravune-theme`,
`.luau` or `.lua`, `.ravune`, `.wav` or `.flac`): nothing in an extension runs outside the
script sandbox. A theme file is what Preferences → Look → Edit theme… → Export writes; a
template is any saved project.

## The gallery

The gallery lives in the repository: `gallery/src` holds one folder per extension, and
`python scripts/gallery.py` packs them and writes `gallery/index.json`. To share yours,
open a pull request that adds its folder.
