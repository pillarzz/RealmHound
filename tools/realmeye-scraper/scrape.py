"""
RealmEye Wiki Scraper v2 - builds item drop data for RealmHound.

Algorithm:
1. Load ObjectID.list for item ID lookup and UT/ST filtering
2. Scrape dungeon pages -> extract "Drops of Interest" tables
3. Scrape biome pages -> extract "Drops of Interest" tables
4. Write compact JSON output
"""

import hashlib
import json
import os
import re
import sys
import time
import unicodedata
from datetime import date
from pathlib import Path

import requests
from bs4 import BeautifulSoup, NavigableString

REALMEYE_BASE = "https://www.realmeye.com/wiki"
CACHE_DIR = Path(__file__).parent / ".cache"
OUTPUT_PATH = Path(__file__).parent / "../../realmhound/assets/data/realmeye_drops.json"
REQUEST_DELAY = 1.0


class RealmEyeScraper:
    def __init__(self, object_list_path: str):
        self.object_list_path = Path(object_list_path)
        self.session = requests.Session()
        self.session.headers.update({
            "User-Agent": "RealmHound-DropScraper/2.0 (community tool; 1req/s)",
        })
        self.last_request_time = 0.0

        # ObjectID.list data
        self.name_to_ids: dict[str, list[int]] = {}
        self.id_to_labels: dict[int, set] = {}

        # Dungeon metadata
        self.dungeon_slugs: list[tuple[str, str]] = []
        self.portals: dict[str, int] = {}

        # Output
        self.output_path = OUTPUT_PATH
        self.dungeons: dict[str, dict] = {}
        self.biomes: dict[str, dict] = {}

    def run(self):
        print("=== RealmEye Scraper v2 ===\n")
        print("[1/4] Loading ObjectID.list...")
        self.load_object_list()
        print(f"  Loaded {len(self.name_to_ids)} item names\n")

        print("[2/4] Scraping dungeon pages...")
        self.scrape_dungeon_list()
        self.scrape_dungeon_drops()
        dungeon_items = sum(
            len(ids)
            for d in self.dungeons.values()
            for ids in d["drops"].values()
        )
        print(f"\n  {len(self.dungeons)} dungeons, {dungeon_items} item entries\n")

        print("[3/4] Scraping biome pages...")
        self.scrape_biome_drops()
        biome_items = sum(
            len(ids)
            for b in self.biomes.values()
            for ids in b["drops"].values()
        )
        print(f"\n  {len(self.biomes)} biomes, {biome_items} item entries\n")

        print("[4/4] Writing output...")
        self.write_output()
        print("  Done!\n")

        self.print_summary()

    def load_object_list(self):
        """Load ObjectID.list and index item names for ID lookup."""
        with open(self.object_list_path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#"):
                    continue
                fields = line.split(";")
                if len(fields) < 8:
                    continue
                try:
                    obj_id = int(fields[0])
                except ValueError:
                    continue

                display = fields[1].strip()
                id_name = fields[7].strip() if len(fields) > 7 else ""
                name = display or id_name
                if not name:
                    continue

                labels = set()
                if len(fields) > 6 and fields[6]:
                    labels = {l.strip() for l in fields[6].split(",") if l.strip()}

                self.name_to_ids.setdefault(name, []).append(obj_id)
                self.id_to_labels[obj_id] = labels

    def is_worthy_item(self, item_id: int) -> bool:
        """Check if an item is a UT/ST equipment piece (not reskin, unless shiny)."""
        labels = self.id_to_labels.get(item_id, set())
        if "RESKIN" in labels and "SHINY" not in labels:
            return False
        if not ({"UT", "ST"} & labels):
            return False
        if not ({"WEAPON", "ABILITY", "ARMOR", "RING", "AMULET"} & labels):
            return False
        return True

    @staticmethod
    def normalize_text(text: str) -> str:
        """Normalize unicode from HTML to ASCII-compatible equivalents."""
        text = unicodedata.normalize("NFKC", text)
        text = text.replace("\u2019", "'").replace("\u2018", "'")
        text = text.replace("\u201c", '"').replace("\u201d", '"')
        text = text.replace("\u2013", "-").replace("\u2014", "-")
        return text

    def fetch_page(self, slug: str) -> BeautifulSoup | None:
        """Fetch a wiki page with caching and rate limiting."""
        cache_file = CACHE_DIR / f"{hashlib.md5(slug.encode()).hexdigest()}.html"

        if cache_file.exists():
            html = cache_file.read_text(encoding="utf-8")
            return BeautifulSoup(html, "lxml")

        elapsed = time.time() - self.last_request_time
        if elapsed < REQUEST_DELAY:
            time.sleep(REQUEST_DELAY - elapsed)

        url = f"{REALMEYE_BASE}/{slug}"
        try:
            resp = self.session.get(url, timeout=15)
            self.last_request_time = time.time()
            if resp.status_code != 200:
                return None
            CACHE_DIR.mkdir(parents=True, exist_ok=True)
            cache_file.write_text(resp.text, encoding="utf-8")
            return BeautifulSoup(resp.text, "lxml")
        except requests.RequestException:
            return None

    def scrape_dungeon_list(self):
        """Fetch /wiki/dungeons and extract dungeon names + slugs + portal IDs."""
        soup = self.fetch_page("dungeons")
        if not soup:
            print("  ERROR: Could not fetch dungeons page")
            return

        seen = set()
        for link in soup.find_all("a", href=re.compile(r"^/wiki/")):
            href = link.get("href", "")
            slug = href.replace("/wiki/", "")
            parent_td = link.find_parent("td")
            if not parent_td:
                continue
            name = link.get_text(strip=True)
            if name and slug and slug != "dungeons" and slug not in seen:
                seen.add(slug)
                self.dungeon_slugs.append((name, slug))
                # Portal ID = object with same name as dungeon
                portal_id = self.name_to_ids.get(name, [None])[0]
                if portal_id:
                    self.portals[name] = portal_id

        print(f"  Found {len(self.dungeon_slugs)} dungeons ({len(self.portals)} with portal IDs)")

    def scrape_dungeon_drops(self):
        """For each dungeon, extract its Drops of Interest table."""
        for name, slug in self.dungeon_slugs:
            sys.stdout.write(f"\r  Processing: {name:<50}")
            sys.stdout.flush()

            soup = self.fetch_page(slug)
            if not soup:
                continue

            drops = self._extract_drops_of_interest(soup)
            if drops:
                self.dungeons[name] = {
                    "portal_id": self.portals.get(name),
                    "drops": drops,
                }

    def scrape_biome_drops(self):
        """Scrape biome pages for Drops of Interest."""
        biomes = {
            "beach": "rookie", "undead-forest": "rookie",
            "low-forest": "rookie", "mid-plains": "rookie",
            "nature-ruins": "rookie", "mid-desert": "rookie",
            "high-plains": "rookie", "high-forest": "rookie",
            "high-desert": "rookie",
            "coral-reefs": "adept", "sprite-forest": "adept",
            "haunted-hallows": "adept", "shipwreck-cove": "adept",
            "dead-church": "adept", "risen-hell": "adept",
            "abandoned-city": "adept",
            "deep-sea-abyss": "veteran", "carboniferous": "veteran",
            "floral-escape": "veteran", "sanguine-forest": "veteran",
            "runic-tundra": "veteran",
            "eternal-frost": "seasonal", "relentless-springs": "seasonal",
        }

        for slug, tier in biomes.items():
            soup = self.fetch_page(slug)
            if not soup:
                continue

            biome_name = slug.replace("-", " ").title()
            h1 = soup.find("h1")
            if h1:
                biome_name = h1.get_text(strip=True)

            sys.stdout.write(f"\r  Processing: {biome_name:<50}")
            sys.stdout.flush()

            drops = self._extract_drops_of_interest(soup)
            if drops:
                self.biomes[biome_name] = {
                    "tier": tier,
                    "drops": drops,
                }

    def _extract_drops_of_interest(self, soup: BeautifulSoup, location_name: str = "") -> dict[str, list[int]]:
        """
        Extract items from a "Drops of Interest" table.
        Returns: {source_label: [item_ids]}
        Handles two formats:
        - Normal: "Item | Drops From" columns
        - Bag-based: rows with bag icons (seasonal biomes)
        """
        drops: dict[str, list[int]] = {}

        heading = None
        for h in soup.find_all(["h2", "h3"]):
            text = h.get_text(strip=True).lower()
            if "drop" in text and "interest" in text:
                heading = h
                break
        if not heading:
            return drops

        table = heading.find_next("table")
        if not table:
            return drops

        # Detect format: check if first row has "Item" + "Drops From" headers
        first_row = table.find("tr")
        if first_row:
            headers = [th.get_text(strip=True).lower() for th in first_row.find_all("th")]
            if "item" in headers and any("drop" in h for h in headers):
                return self._parse_normal_drops_table(table)

        # Bag-based format: look for white/ST bag rows
        return self._parse_bag_based_table(table, location_name)

    def _parse_normal_drops_table(self, table) -> dict[str, list[int]]:
        """Parse standard 'Item | Drops From' table."""
        drops: dict[str, list[int]] = {}
        rows = table.find_all("tr")

        for row in rows[1:]:
            h4 = row.find("h4")
            if h4 and "historical" in h4.get_text(strip=True).lower():
                break

            cells = row.find_all("td")
            if len(cells) < 2:
                continue

            item_ids = self._extract_item_ids(cells[0])
            if not item_ids:
                continue

            item_slugs = set()
            for link in cells[0].find_all("a", href=re.compile(r"^/wiki/")):
                item_slugs.add(link.get("href", "").replace("/wiki/", ""))

            sources = self._extract_sources(cells[1], item_slugs)
            if not sources:
                continue

            for source in sources:
                if source not in drops:
                    drops[source] = []
                for item_id in item_ids:
                    if item_id not in drops[source]:
                        drops[source].append(item_id)

        return drops

    def _parse_bag_based_table(self, table, location_name: str) -> dict[str, list[int]]:
        """Parse bag-based table (seasonal biomes). Only process white/ST bag rows."""
        drops: dict[str, list[int]] = {}
        TARGET_BAGS = {"White Bag", "Cyan Bag", "Gold Bag", "ST Bag"}

        tbody = table.find("tbody") or table
        rows = tbody.find_all("tr", recursive=False)

        current_bag = None
        remaining_span = 0

        for row in rows:
            cells = row.find_all("td", recursive=False)
            if not cells:
                continue

            # Check if first cell has a bag icon (new bag section)
            first_imgs = cells[0].find_all("img", recursive=False)
            bag_names = {img.get("title", "") for img in first_imgs}
            detected_bag = bag_names & TARGET_BAGS

            if detected_bag or bag_names - {""}:
                # New bag section
                rowspan = cells[0].get("rowspan", "")
                remaining_span = int(rowspan) if rowspan else 1
                current_bag = next(iter(detected_bag)) if detected_bag else None
            else:
                remaining_span -= 1

            if not current_bag:
                continue
            if remaining_span <= 0:
                current_bag = None
                continue

            # Collect all worthy items from this row
            items_to_resolve: list[tuple[int, str]] = []
            for cell in cells:
                for link in cell.find_all("a", href=re.compile(r"^/wiki/")):
                    img = link.find("img")
                    if img:
                        name = img.get("title", "") or img.get("alt", "")
                    else:
                        name = link.get_text(strip=True)
                    if not name:
                        continue
                    name = self.normalize_text(name)
                    item_ids = self.name_to_ids.get(name, [])
                    for item_id in item_ids:
                        if item_id and self.is_worthy_item(item_id):
                            slug = link.get("href", "").replace("/wiki/", "")
                            items_to_resolve.append((item_id, slug))

            # Fetch each item's page to get specific mob sources
            for item_id, slug in items_to_resolve:
                sys.stdout.write(f"\r    Resolving: {slug:<45}")
                sys.stdout.flush()
                mobs = self._fetch_item_drops_from(slug)
                if mobs:
                    for mob_name in mobs:
                        if mob_name not in drops:
                            drops[mob_name] = []
                        if item_id not in drops[mob_name]:
                            drops[mob_name].append(item_id)
                else:
                    if "All Enemies" not in drops:
                        drops["All Enemies"] = []
                    if item_id not in drops["All Enemies"]:
                        drops["All Enemies"].append(item_id)

        return drops

    def _fetch_item_drops_from(self, slug: str) -> list[str]:
        """Fetch an item's wiki page and extract 'Drops From' mob names.
        Stops at seasonal event separators."""
        soup = self.fetch_page(slug)
        if not soup:
            return []

        for th in soup.find_all("th"):
            if "drops from" in th.get_text(strip=True).lower():
                td = th.find_next_sibling("td")
                if not td:
                    tr = th.find_parent("tr")
                    if tr:
                        td = tr.find("td")
                if not td:
                    break

                mobs = []
                for child in td.children:
                    if hasattr(child, 'name') and child.name == 'b':
                        text = child.get_text(strip=True).lower()
                        if 'during' in text:
                            break
                    if hasattr(child, 'name') and child.name == 'a':
                        href = child.get("href", "")
                        if href.startswith("/wiki/"):
                            mob_name = child.get_text(strip=True)
                            if mob_name and "chest" not in mob_name.lower():
                                mobs.append(self.normalize_text(mob_name))
                return mobs

        return []

        return drops

    def _extract_item_ids(self, cell) -> list[int]:
        """Extract worthy item IDs from an item table cell."""
        ids = []
        for link in cell.find_all("a", href=re.compile(r"^/wiki/")):
            img = link.find("img")
            if img:
                name = img.get("title", "") or img.get("alt", "")
            else:
                name = link.get_text(strip=True)
            if not name:
                continue
            name = self.normalize_text(name)
            for item_id in self.name_to_ids.get(name, []):
                if item_id and self.is_worthy_item(item_id):
                    ids.append(item_id)
        return ids

    def _extract_sources(self, cell, item_slugs: set = None) -> list[str]:
        """
        Parse the "Drops From" cell to extract source labels.
        Handles:
        - Specific mobs: <a href="/wiki/mob-slug"><img/>Mob Name</a>
        - Categories: "All" text + <a href="/wiki/page#section">Category Name</a>
        - Self-referencing: link points back to the item itself -> "All Enemies"
        """
        if item_slugs is None:
            item_slugs = set()
        sources = []
        children = list(cell.children)

        i = 0
        while i < len(children):
            child = children[i]

            if isinstance(child, NavigableString):
                text = child.strip().rstrip(",").strip()
                if text.lower().startswith("all"):
                    # Look ahead for category link
                    j = i + 1
                    while j < len(children) and isinstance(children[j], NavigableString):
                        j += 1
                    if j < len(children) and hasattr(children[j], 'name') and children[j].name == 'a':
                        cat_link = children[j]
                        cat_name = self.normalize_text(cat_link.get_text(strip=True))
                        sources.append(f"All {cat_name}")
                        i = j + 1
                        continue
                i += 1
                continue

            if hasattr(child, 'name') and child.name == 'a':
                href = child.get("href", "")
                if href.startswith("/wiki/"):
                    slug = href.replace("/wiki/", "").split("#")[0]
                    # Skip if this links back to the item itself
                    if slug in item_slugs:
                        if not sources:
                            sources.append("All Enemies")
                        i += 1
                        continue
                    img = child.find("img")
                    if img:
                        mob_name = child.get_text(strip=True)
                        if not mob_name:
                            mob_name = img.get("title", "") or img.get("alt", "")
                        if mob_name:
                            sources.append(self.normalize_text(mob_name))
                    else:
                        text_content = child.get_text(strip=True)
                        if text_content:
                            sources.append(self.normalize_text(text_content))

            i += 1

        return sources

    def write_output(self):
        """Write compact JSON output."""
        all_sources = set()
        for d in self.dungeons.values():
            all_sources.update(d["drops"].keys())
        for b in self.biomes.values():
            all_sources.update(b["drops"].keys())

        source_list = sorted(all_sources)
        source_to_idx = {name: idx for idx, name in enumerate(source_list)}

        dungeon_entries = []
        for name, data in sorted(self.dungeons.items()):
            portal_id = data["portal_id"]
            drop_tuples = []
            for source, item_ids in sorted(data["drops"].items(), key=lambda x: x[0]):
                drop_tuples.append([source_to_idx[source], sorted(item_ids)])
            dungeon_entries.append([name, portal_id, drop_tuples])

        biome_entries = []
        for name, data in sorted(self.biomes.items()):
            tier = data["tier"]
            drop_tuples = []
            for source, item_ids in sorted(data["drops"].items(), key=lambda x: x[0]):
                drop_tuples.append([source_to_idx[source], sorted(item_ids)])
            biome_entries.append([name, tier, drop_tuples])

        output = {
            "v": str(date.today()),
            "sources": source_list,
            "dungeons": dungeon_entries,
            "biomes": biome_entries,
        }

        self.output_path.parent.mkdir(parents=True, exist_ok=True)
        with open(self.output_path, "w", encoding="utf-8") as f:
            json.dump(output, f, separators=(",", ":"))

        size_kb = os.path.getsize(self.output_path) / 1024
        print(f"  Written to: {self.output_path}")
        print(f"  File size: {size_kb:.1f} KB")

    def print_summary(self):
        total_dungeon_items = set()
        for d in self.dungeons.values():
            for ids in d["drops"].values():
                total_dungeon_items.update(ids)

        total_biome_items = set()
        for b in self.biomes.values():
            for ids in b["drops"].values():
                total_biome_items.update(ids)

        all_items = total_dungeon_items | total_biome_items
        print("=== Summary ===")
        print(f"  Dungeons: {len(self.dungeons)}")
        print(f"  Biomes: {len(self.biomes)}")
        print(f"  Unique items (dungeons): {len(total_dungeon_items)}")
        print(f"  Unique items (biomes): {len(total_biome_items)}")
        print(f"  Total unique items: {len(all_items)}")


def main():
    if len(sys.argv) != 2:
        print("Usage: python scrape.py <path-to-ObjectID.list>", file=sys.stderr)
        sys.exit(2)

    scraper = RealmEyeScraper(sys.argv[1])
    scraper.run()


if __name__ == "__main__":
    main()
