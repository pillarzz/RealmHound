# Research documents

## Combat History accounting design

- `combat-history-design.md` — how fight totals, percentages, the boss HP bar,
  rescaling/healing, overkill, invulnerability, solo diagnostics, and the
  Marble Colossus / Moonlight Village special cases are defined.

## Player damage sources: implementation status

- `damage-sources-status.md` — source of truth; GitHub also renders it as Markdown.
- `damage-sources-status.html` — self-contained interactive view served by GitHub Pages:
  <https://pillarzz.github.io/RealmHound/research/damage-sources-status.html>

### Regenerating the HTML

The HTML is generated from the Markdown by `build_damage_sources.py` in the local
`RealmHound-docs/research` workspace. It reads `damage-sources-status.md`, the optional
`sprites/manifest.json` and the sprite PNGs, then writes `damage-sources-status.html`.

```powershell
cd <RealmHound-docs>\research
python build_damage_sources.py
```

Copy the refreshed Markdown and HTML into this directory and commit both. The full build
embeds the item sprites as data URIs, so the page is self-contained and works offline; do
not edit the generated HTML by hand. Without `sprites/manifest.json` the script still
builds a working page (about 826 KB) that keeps every table and item name but omits the
icons.

Item sprites are extracted from the game client. If the repository should not carry those
assets, publish the no-sprite build instead of the full one.
