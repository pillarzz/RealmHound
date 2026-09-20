# RealmEye Drop Scraper

Scrapes RealmEye wiki to build an item drop mapping (item → mob → dungeon/biome/encounter).

## Setup

```powershell
cd tools/realmeye-scraper
pip install -r requirements.txt
```

## Usage

```powershell
python scrape.py "$env:LOCALAPPDATA\RealmHound\assets\ObjectID.list"
```

Output: `../../realmhound/assets/data/realmeye_drops.json`

## How it works

1. Loads ObjectID.list to get all UT/ST equipment item IDs and display names
2. Scrapes each dungeon/biome wiki page to build mob → location mapping
3. Scrapes each item's RealmEye page for "Drops From" field
4. Cross-references mobs to locations
5. Outputs structured JSON

## Rate limiting

- 1 request per second (respectful to RealmEye)
- Cached responses in `.cache/` to avoid re-fetching on subsequent runs

## Notes

- Skips potions, consumables, dyes, keys
- Only processes items with UT or ST labels
- Portal IDs come from ObjectID.list (class "Portal")
