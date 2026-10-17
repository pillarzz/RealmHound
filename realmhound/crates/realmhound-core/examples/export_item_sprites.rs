//! Export one PNG per equippable RotMG item, for the HTML item-icon reference.
//!
//! For every `<Object>` in `equip.xml` that is equippable (has a `<SlotType>`)
//! and carries a `<Texture><File>…</File><Index>…</Index></Texture>` -- or, when
//! it has no `<Texture>`, an equivalently shaped `<AnimatedTexture>` (player-skin
//! consumables use that form) -- the sprite is resolved through the game's sprite
//! atlas (`flatbuffer/spritesheetf`), cropped out of the matching atlas PNG,
//! upscaled 4x with nearest-neighbour (8x8 item art -> 32x32 icons) and written
//! as `<type>.png`, where `<type>` is the lowercase `0x…` object type
//! ("0x9c0.png").
//!
//! Animated textures are resolved the way the renderer resolves them for a
//! static icon: [`SpriteAtlas::get_sprite_with_direction`] with direction 0
//! (down / facing the screen), which prefers the animated entry for that
//! direction and falls back to the plain sheet lookup.
//!
//! A pretty-printed `manifest.json` next to the PNGs maps each file stem to the
//! item's display name: `{"0x9c0": "Wand of Retribution", ...}`.
//!
//! Items that cannot be resolved are reported (first 10, with the reason) and
//! skipped -- a bad entry never aborts the run.
//!
//! Usage:
//!   cargo run -p realmhound-core --example export_item_sprites -- [--limit N] [--out DIR]
//!
//! `--limit N` exports at most N items (quick smoke test); `--out DIR` overrides
//! the output directory (default: the docs repo's `research/sprites`).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use quick_xml::events::Event;
use quick_xml::Reader;
use realmhound_core::assets::{find_assets_dir, SpriteAtlas};

/// Output directory consumed by the HTML reference document.
const DEFAULT_OUT_DIR: &str = r"E:\git\RealmHound-docs\research\sprites";

/// Nearest-neighbour upscale factor: 8x8 item art -> 32x32 icons.
const SCALE: u32 = 4;

/// Atlas id -> PNG file stem. Mirrors the sprite renderer's atlas list
/// (`crates/realmhound/src/rendering/sprite_renderer.rs`).
const ATLAS_FILES: [(u8, &str); 4] = [
    (1, "groundTiles"),
    (2, "characters"),
    (3, "characters_masks"),
    (4, "mapObjects"),
];

/// An equippable item with a resolvable sprite reference.
struct ItemEntry {
    /// Normalised lowercase `0x…` type, used as the PNG file stem.
    key: String,
    /// Display name (`<DisplayId>`, else the object's `id` attribute).
    name: String,
    /// Sheet name from `<Texture><File>` / `<AnimatedTexture><File>`.
    sheet: String,
    /// Sprite index from `<Texture><Index>` / `<AnimatedTexture><Index>`.
    index: i32,
    /// True when the sprite came from `<AnimatedTexture>`, so the animated
    /// (sheet, index, direction) lookup is used instead of the plain one.
    animated: bool,
}

/// Which object child's text is currently being collected.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    DisplayId,
    File,
    Index,
}

/// Accumulates the `<Object>` fields we care about while it is being parsed.
#[derive(Default)]
struct ObjectBuilder {
    type_attr: String,
    id_attr: String,
    display_id: String,
    slot_type: bool,
    file: String,
    index: String,
    animated: bool,
}

fn main() {
    let mut limit: Option<usize> = None;
    let mut out_dir = PathBuf::from(DEFAULT_OUT_DIR);

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--limit" => match args.next().and_then(|v| v.parse::<usize>().ok()) {
                Some(n) => limit = Some(n),
                None => eprintln!("--limit expects a number, ignoring"),
            },
            "--out" => match args.next() {
                Some(v) => out_dir = PathBuf::from(v),
                None => eprintln!("--out expects a directory, ignoring"),
            },
            "-h" | "--help" => {
                println!("usage: export_item_sprites [--limit N] [--out DIR]");
                return;
            }
            other => eprintln!("ignoring unknown argument: {other}"),
        }
    }

    let Some(assets_dir) = find_assets_dir() else {
        eprintln!("could not find the RealmHound assets dir (no ObjectID.list)");
        return;
    };
    println!("assets: {}", assets_dir.display());

    let atlas =
        match SpriteAtlas::load_from_file(assets_dir.join("flatbuffer").join("spritesheetf")) {
            Ok(atlas) => atlas,
            Err(e) => {
                eprintln!("failed to load sprite atlas: {e}");
                return;
            }
        };
    println!(
        "atlas: {} sheets, {} sprites",
        atlas.sheet_count(),
        atlas.sprite_count()
    );

    let equip_xml = assets_dir.join("xml").join("equip.xml");
    let (items, skipped_no_texture) = match parse_equip_items(&equip_xml) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("failed to parse {}: {e}", equip_xml.display());
            return;
        }
    };
    println!(
        "equippable items with a sprite source: {} ({} equippable without <Texture>/<AnimatedTexture> File+Index skipped)",
        items.len(),
        skipped_no_texture
    );

    if let Err(e) = fs::create_dir_all(&out_dir) {
        eprintln!("failed to create {}: {e}", out_dir.display());
        return;
    }

    let sprites_dir = assets_dir.join("sprites");
    let mut atlas_images: HashMap<u8, Result<image::RgbaImage, String>> = HashMap::new();
    let mut manifest: BTreeMap<String, String> = BTreeMap::new();
    let mut atlas_use: BTreeMap<u8, usize> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    let mut failed = 0usize;

    let selected = items.iter().take(limit.unwrap_or(usize::MAX));
    for item in selected {
        let mut fail = |reason: String| {
            failed += 1;
            if failures.len() < 10 {
                failures.push(format!("{} ({}): {reason}", item.key, item.name));
            }
        };

        // Animated textures (player-skin consumables) are keyed by
        // (sheet, index, direction) in the atlas; direction 0 is the default
        // frame and the lookup falls back to the plain sheet lookup.
        let found = if item.animated {
            atlas
                .get_sprite_with_direction(&item.sheet, item.index, 0)
                .copied()
        } else {
            atlas.get_sprite(&item.sheet, item.index).copied()
        };
        let Some(sprite) = found else {
            fail(format!(
                "sprite {}:{} not present in the atlas",
                item.sheet, item.index
            ));
            continue;
        };

        let Some(png_name) = atlas_png_name(sprite.atlas_id) else {
            fail(format!("unknown atlas id {}", sprite.atlas_id));
            continue;
        };
        let entry = atlas_images.entry(sprite.atlas_id).or_insert_with(|| {
            let path = sprites_dir.join(format!("{png_name}.png"));
            image::open(&path)
                .map(|img| img.to_rgba8())
                .map_err(|e| format!("{}: {e}", path.display()))
        });
        let atlas_image = match entry {
            Ok(img) => img,
            Err(e) => {
                fail(format!("atlas {} unreadable: {e}", sprite.atlas_id));
                continue;
            }
        };

        if sprite.width <= 0 || sprite.height <= 0 {
            fail(format!(
                "sprite {}:{} has invalid size {}x{}",
                item.sheet, item.index, sprite.width, sprite.height
            ));
            continue;
        }
        let (x, y, w, h) = (
            sprite.x as u32,
            sprite.y as u32,
            sprite.width as u32,
            sprite.height as u32,
        );
        if x + w > atlas_image.width() || y + h > atlas_image.height() {
            fail(format!(
                "region {x},{y} {w}x{h} outside atlas {png_name} ({}x{})",
                atlas_image.width(),
                atlas_image.height()
            ));
            continue;
        }

        let crop = image::imageops::crop_imm(atlas_image, x, y, w, h).to_image();
        let scaled = image::imageops::resize(
            &crop,
            w * SCALE,
            h * SCALE,
            image::imageops::FilterType::Nearest,
        );
        let out_path = out_dir.join(format!("{}.png", item.key));
        if let Err(e) = scaled.save(&out_path) {
            fail(format!("failed to write {}: {e}", out_path.display()));
            continue;
        }

        *atlas_use.entry(sprite.atlas_id).or_insert(0) += 1;
        manifest.insert(item.key.clone(), item.name.clone());
    }

    let manifest_path = out_dir.join("manifest.json");
    let manifest_json = match serde_json::to_string_pretty(&manifest) {
        Ok(json) => json,
        Err(e) => {
            eprintln!("failed to serialise manifest: {e}");
            return;
        }
    };
    if let Err(e) = fs::write(&manifest_path, manifest_json) {
        eprintln!("failed to write {}: {e}", manifest_path.display());
        return;
    }

    println!("\nsummary");
    println!("  items found:     {}", items.len());
    println!("  sprites written: {}", manifest.len());
    println!("  failures:        {failed}");
    for (atlas_id, count) in &atlas_use {
        let name = atlas_png_name(*atlas_id).unwrap_or("<unknown>");
        println!("  atlas {atlas_id} ({name}): {count}");
    }
    if !failures.is_empty() {
        println!("  first failures:");
        for failure in &failures {
            println!("    - {failure}");
        }
    }
    println!("  output:          {}", out_dir.display());
    println!("  manifest:        {} entries", manifest.len());
}

/// Atlas PNG stem for a sprite's atlas id, mirroring the renderer's mapping.
fn atlas_png_name(atlas_id: u8) -> Option<&'static str> {
    ATLAS_FILES
        .iter()
        .find(|(id, _)| *id == atlas_id)
        .map(|(_, name)| *name)
}

/// Parse every equippable item out of `equip.xml`.
///
/// Returns the items (in document order) plus the number of equippable objects
/// that had no usable `<Texture><File>/<Index>`.
fn parse_equip_items(equip_xml: &Path) -> Result<(Vec<ItemEntry>, usize), String> {
    let file = fs::File::open(equip_xml).map_err(|e| format!("open: {e}"))?;
    let mut reader = Reader::from_reader(BufReader::new(file));
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut items = Vec::new();
    let mut skipped_no_texture = 0usize;

    // Depth of the currently open <Object>, if any. `rel` below is the element's
    // depth relative to it, so direct children are rel 1 and <File>/<Index>
    // inside <Texture> are rel 2.
    let mut object_base: Option<usize> = None;
    let mut depth = 0usize;
    let mut builder = ObjectBuilder::default();
    let mut in_texture = false;
    let mut field: Option<Field> = None;
    // `File`/`Index` of the `<Texture>`/`<AnimatedTexture>` currently open.
    let mut source_file = String::new();
    let mut source_index = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Err(e) => return Err(format!("at byte {}: {e}", reader.buffer_position())),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let tag = e.name().as_ref().to_vec();
                if object_base.is_none() {
                    if tag == b"Object" {
                        builder = ObjectBuilder::default();
                        for attr in e.attributes().flatten() {
                            let value = attr
                                .unescape_value()
                                .map(|v| v.into_owned())
                                .unwrap_or_default();
                            match attr.key.as_ref() {
                                b"type" => builder.type_attr = value,
                                b"id" => builder.id_attr = value,
                                _ => {}
                            }
                        }
                        object_base = Some(depth);
                    }
                } else if let Some(base) = object_base {
                    match depth - base {
                        1 => match tag.as_slice() {
                            b"Texture" | b"AnimatedTexture" => {
                                in_texture = true;
                                source_file.clear();
                                source_index.clear();
                            }
                            b"SlotType" => builder.slot_type = true,
                            b"DisplayId" => field = Some(Field::DisplayId),
                            _ => {}
                        },
                        2 if in_texture => match tag.as_slice() {
                            b"File" => field = Some(Field::File),
                            b"Index" => field = Some(Field::Index),
                            _ => {}
                        },
                        _ => {}
                    }
                }
                depth += 1;
            }
            Ok(Event::End(e)) => {
                depth = depth.saturating_sub(1);
                let tag = e.name().as_ref().to_vec();
                match tag.as_slice() {
                    b"File" | b"Index" | b"DisplayId" => field = None,
                    b"Texture" | b"AnimatedTexture" => {
                        in_texture = false;
                        commit_sprite_source(
                            &mut builder,
                            &source_file,
                            &source_index,
                            tag == b"AnimatedTexture",
                        );
                    }
                    _ => {}
                }
                if tag == b"Object" {
                    if let Some(base) = object_base {
                        if depth == base {
                            object_base = None;
                            if let Some(item) = finish_object(&builder, &mut skipped_no_texture) {
                                items.push(item);
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(field) = field {
                    let text = e.unescape().unwrap_or_default();
                    match field {
                        Field::DisplayId => builder.display_id.push_str(&text),
                        Field::File => source_file.push_str(&text),
                        Field::Index => source_index.push_str(&text),
                    }
                }
            }
            Ok(_) => {}
        }
    }

    Ok((items, skipped_no_texture))
}

/// Merge a just-closed `<Texture>`/`<AnimatedTexture>` into the builder.
///
/// `<Texture>` wins over `<AnimatedTexture>`; among sources of the same kind the
/// first complete one wins, so later elements cannot silently replace it.
fn commit_sprite_source(builder: &mut ObjectBuilder, file: &str, index: &str, animated: bool) {
    let (file, index) = (file.trim(), index.trim());
    if file.is_empty() || index.is_empty() {
        return;
    }
    if !builder.file.is_empty() && (animated || !builder.animated) {
        return;
    }
    builder.file = file.to_string();
    builder.index = index.to_string();
    builder.animated = animated;
}

/// Convert a finished `<Object>` into an [`ItemEntry`], when it qualifies.
fn finish_object(builder: &ObjectBuilder, skipped_no_texture: &mut usize) -> Option<ItemEntry> {
    if !builder.slot_type {
        return None;
    }
    let file = builder.file.trim();
    let index = match parse_number(builder.index.trim()) {
        Some(index) => index,
        None => {
            *skipped_no_texture += 1;
            return None;
        }
    };
    if file.is_empty() {
        *skipped_no_texture += 1;
        return None;
    }
    let key = format!("0x{:x}", parse_type(builder.type_attr.trim())?);
    let name = if !builder.display_id.trim().is_empty() {
        builder.display_id.trim().to_string()
    } else if !builder.id_attr.trim().is_empty() {
        builder.id_attr.trim().to_string()
    } else {
        key.clone()
    };
    Some(ItemEntry {
        key,
        name,
        sheet: file.to_string(),
        index,
        animated: builder.animated,
    })
}

/// Parse a decimal or `0x`-prefixed hex number.
fn parse_number(raw: &str) -> Option<i32> {
    let raw = raw.trim();
    match raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        Some(hex) => i32::from_str_radix(hex, 16).ok(),
        None => raw.parse::<i32>().ok(),
    }
}

/// Parse an object `type` attribute: hex, with or without the `0x` prefix.
fn parse_type(raw: &str) -> Option<u32> {
    let digits = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .unwrap_or(raw);
    u32::from_str_radix(digits, 16).ok()
}
