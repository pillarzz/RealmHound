//! XML parser to generate ObjectID.list and TileID.list from extracted XML files.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

/// Parsed object asset from XML.
#[derive(Debug, Default, Clone)]
pub struct XmlObject {
    pub id: i32,
    pub id_name: String,
    pub display_name: String,
    pub class: String,
    pub group: String,
    pub labels: String,
    pub textures: Vec<XmlTexture>,
    pub projectiles: Vec<XmlProjectile>,
    /// Mask texture for dye/cloth rendering
    pub mask: Option<XmlTexture>,
    /// Tex1 color value (clothing dye/cloth pattern)
    pub tex1: u32,
    /// Tex2 color value (accessory dye/cloth pattern)
    pub tex2: u32,
    /// SlotType from game XML (determines equipment category)
    pub slot_type: i32,
    /// Base defense from game XML (enemy DEF; reduces our self-computed hits)
    pub defense: i32,
    /// Enemy collision hitbox scale (`CustomHitbox scale` in the object XML).
    /// 0.0 means absent; serialized as 1.0 (the game default).
    pub hitbox_scale: f32,
}

/// Parsed tile asset from XML.
#[derive(Debug, Default, Clone)]
pub struct XmlTile {
    pub id: i32,
    pub id_name: String,
    pub damage: i32,
    pub texture: Option<XmlTexture>,
}

/// Texture data from XML.
#[derive(Debug, Default, Clone)]
pub struct XmlTexture {
    pub file: String,
    pub index: String,
}

/// Projectile data from XML.
#[derive(Debug, Default, Clone)]
pub struct XmlProjectile {
    pub min_damage: i32,
    pub max_damage: i32,
    pub armor_piercing: bool,
}

impl XmlObject {
    /// Format as ObjectID.list line.
    pub fn to_list_line(&self) -> String {
        let projectile_str = self
            .projectiles
            .iter()
            .map(|p| {
                format!(
                    "{}:{}:{}",
                    p.min_damage,
                    p.max_damage,
                    if p.armor_piercing { 1 } else { 0 }
                )
            })
            .collect::<Vec<_>>()
            .join(",");

        let texture_str = self
            .textures
            .iter()
            .map(|t| format!("{}:{}", t.file, t.index))
            .collect::<Vec<_>>()
            .join(",");

        let mask_str = self
            .mask
            .as_ref()
            .map(|m| format!("{}:{}", m.file, m.index))
            .unwrap_or_default();
        let tex1_str = if self.tex1 != 0 {
            format!("0x{:x}", self.tex1)
        } else {
            String::new()
        };
        let tex2_str = if self.tex2 != 0 {
            format!("0x{:x}", self.tex2)
        } else {
            String::new()
        };

        format!(
            "{};{};{};{};{};{};{};{};{};{};{};{};{};{}",
            self.id,
            self.display_name,
            self.class,
            self.group,
            projectile_str,
            texture_str,
            self.labels,
            self.id_name,
            mask_str,
            tex1_str,
            tex2_str,
            self.slot_type,
            self.defense,
            if self.hitbox_scale > 0.0 {
                self.hitbox_scale
            } else {
                1.0
            },
        )
    }
}

impl XmlTile {
    /// Format as TileID.list line.
    /// Format: `id;textureData;damage;idName`
    pub fn to_list_line(&self) -> String {
        let texture_str = self
            .texture
            .as_ref()
            .map(|t| format!("{}:{}", t.file, t.index))
            .unwrap_or_default();

        format!(
            "{};{};{};{}",
            self.id, texture_str, self.damage, self.id_name,
        )
    }
}

/// Parse all XML files in a directory and generate ObjectID.list and TileID.list.
pub fn generate_asset_lists<P: AsRef<Path>, Q: AsRef<Path>>(
    xml_dir: P,
    output_dir: Q,
) -> io::Result<(usize, usize)> {
    let xml_dir = xml_dir.as_ref();
    let output_dir = output_dir.as_ref();

    let mut objects: BTreeMap<i32, XmlObject> = BTreeMap::new();
    let mut tiles: BTreeMap<i32, XmlTile> = BTreeMap::new();

    // Walk all XML files
    for entry in fs::read_dir(xml_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("xml") {
            if let Err(e) = parse_xml_file(&path, &mut objects, &mut tiles) {
                tracing::warn!("Failed to parse {:?}: {}", path, e);
            }
        }
    }

    // Write ObjectID.list
    let object_lines: Vec<_> = objects.values().map(|o| o.to_list_line()).collect();
    fs::write(output_dir.join("ObjectID.list"), object_lines.join("\n"))?;

    // Write TileID.list
    let tile_lines: Vec<_> = tiles.values().map(|t| t.to_list_line()).collect();
    fs::write(output_dir.join("TileID.list"), tile_lines.join("\n"))?;

    Ok((objects.len(), tiles.len()))
}

/// Parse a single XML file and extract objects and tiles.
fn parse_xml_file<P: AsRef<Path>>(
    path: P,
    objects: &mut BTreeMap<i32, XmlObject>,
    tiles: &mut BTreeMap<i32, XmlTile>,
) -> io::Result<()> {
    let content = fs::read_to_string(path)?;
    let mut reader = Reader::from_str(&content);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut current_object: Option<XmlObject> = None;
    let mut current_tile: Option<XmlTile> = None;
    let mut current_projectile: Option<XmlProjectile> = None;
    let mut current_texture: Option<XmlTexture> = None;
    let mut in_texture = false;
    let mut in_animated_texture = false;
    let mut in_mask = false;
    let mut current_text_tag = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                match tag_name.as_str() {
                    "Object" => {
                        let mut obj = XmlObject::default();
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"type" => {
                                    let val = String::from_utf8_lossy(&attr.value);
                                    obj.id = parse_hex_or_dec(&val);
                                }
                                b"id" => {
                                    obj.id_name = String::from_utf8_lossy(&attr.value).to_string();
                                }
                                _ => {}
                            }
                        }
                        current_object = Some(obj);
                    }
                    "Ground" => {
                        let mut tile = XmlTile::default();
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"type" => {
                                    let val = String::from_utf8_lossy(&attr.value);
                                    tile.id = parse_hex_or_dec(&val);
                                }
                                b"id" => {
                                    tile.id_name = String::from_utf8_lossy(&attr.value).to_string();
                                }
                                _ => {}
                            }
                        }
                        current_tile = Some(tile);
                    }
                    "Projectile" => {
                        current_projectile = Some(XmlProjectile::default());
                    }
                    "Texture" => {
                        current_texture = Some(XmlTexture::default());
                        in_texture = true;
                    }
                    "AnimatedTexture" => {
                        current_texture = Some(XmlTexture::default());
                        in_animated_texture = true;
                    }
                    "Mask" => {
                        current_texture = Some(XmlTexture::default());
                        in_mask = true;
                    }
                    _ => {
                        current_text_tag = tag_name;
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                // Handle self-closing tags like <ArmorPiercing/>
                if tag_name == "ArmorPiercing" {
                    if let Some(ref mut proj) = current_projectile {
                        proj.armor_piercing = true;
                    }
                } else if tag_name == "CustomHitbox" {
                    if let Some(ref mut obj) = current_object {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"scale" {
                                obj.hitbox_scale =
                                    String::from_utf8_lossy(&attr.value).parse().unwrap_or(1.0);
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(ref e)) => {
                let text = e.unescape().unwrap_or_default().to_string();

                if in_texture || in_animated_texture || in_mask {
                    if let Some(ref mut tex) = current_texture {
                        match current_text_tag.as_str() {
                            "File" => tex.file = text,
                            "Index" => tex.index = text,
                            _ => {}
                        }
                    }
                } else if let Some(ref mut proj) = current_projectile {
                    match current_text_tag.as_str() {
                        "MinDamage" => proj.min_damage = text.parse().unwrap_or(0),
                        "MaxDamage" => proj.max_damage = text.parse().unwrap_or(0),
                        "Damage" => {
                            let dmg = text.parse().unwrap_or(0);
                            proj.min_damage = dmg;
                            proj.max_damage = dmg;
                        }
                        _ => {}
                    }
                } else if let Some(ref mut obj) = current_object {
                    match current_text_tag.as_str() {
                        "DisplayId" => obj.display_name = text,
                        "Class" => obj.class = text,
                        "Group" => obj.group = text,
                        "SlotType" => obj.slot_type = text.parse().unwrap_or(0),
                        "Defense" => obj.defense = text.parse().unwrap_or(0),
                        "Labels" => obj.labels = text,
                        "Tex1" => obj.tex1 = parse_hex_u32(&text),
                        "Tex2" => obj.tex2 = parse_hex_u32(&text),
                        _ => {}
                    }
                } else if let Some(ref mut tile) = current_tile {
                    match current_text_tag.as_str() {
                        "MinDamage" | "MaxDamage" | "Damage" => {
                            tile.damage = text.parse().unwrap_or(0);
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                match tag_name.as_str() {
                    "Object" => {
                        if let Some(obj) = current_object.take() {
                            if obj.id != 0 {
                                objects.insert(obj.id, obj);
                            }
                        }
                    }
                    "Ground" => {
                        if let Some(tile) = current_tile.take() {
                            if tile.id != 0 {
                                tiles.insert(tile.id, tile);
                            }
                        }
                    }
                    "Projectile" => {
                        if let Some(proj) = current_projectile.take() {
                            if let Some(ref mut obj) = current_object {
                                obj.projectiles.push(proj);
                            }
                        }
                    }
                    "Texture" => {
                        if let Some(tex) = current_texture.take() {
                            if in_texture {
                                if let Some(ref mut obj) = current_object {
                                    obj.textures.push(tex);
                                } else if let Some(ref mut tile) = current_tile {
                                    tile.texture = Some(tex);
                                }
                            }
                        }
                        in_texture = false;
                    }
                    "AnimatedTexture" => {
                        if let Some(tex) = current_texture.take() {
                            if in_animated_texture {
                                if let Some(ref mut obj) = current_object {
                                    obj.textures.push(tex);
                                } else if let Some(ref mut tile) = current_tile {
                                    tile.texture = Some(tex);
                                }
                            }
                        }
                        in_animated_texture = false;
                    }
                    "Mask" => {
                        if let Some(tex) = current_texture.take() {
                            if in_mask {
                                if let Some(ref mut obj) = current_object {
                                    obj.mask = Some(tex);
                                }
                            }
                        }
                        in_mask = false;
                    }
                    _ => {}
                }
                current_text_tag.clear();
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(())
}

/// Parse a hex (0x...) or decimal string to i32.
fn parse_hex_or_dec(s: &str) -> i32 {
    if s.starts_with("0x") || s.starts_with("0X") {
        i32::from_str_radix(&s[2..], 16).unwrap_or(0)
    } else {
        s.parse().unwrap_or(0)
    }
}

/// Parse a hex (0x...) or decimal string to u32 (for Tex1/Tex2 color values).
fn parse_hex_u32(s: &str) -> u32 {
    if s.starts_with("0x") || s.starts_with("0X") {
        u32::from_str_radix(&s[2..], 16).unwrap_or(0)
    } else {
        s.parse().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex() {
        assert_eq!(parse_hex_or_dec("0x0b25"), 0x0b25);
        assert_eq!(parse_hex_or_dec("123"), 123);
        assert_eq!(parse_hex_or_dec("0X1A"), 0x1A);
    }
}
