//! XML parser for char/list API responses.

use quick_xml::events::Event;
use quick_xml::Reader;

use super::character::{CharacterClass, Pet, PetAbility, PetInventoryItem, RealmCharacter};
use super::pcstats::decode_pcstats;
use super::storage::{AccountData, GiftStorage, PotionStorage, VaultChest};

/// Parse pet inventory from the inv attribute format.
/// Format: "start,count;X;item1,item2#uniqueid,item3,..."
/// Example: "0,8;X;31128,31128,10023#6032401060888576,2799"
/// Note: Stack counts are not available in API data, only from live NewTick packets.
fn parse_pet_inventory(inv: &str) -> (i32, Vec<PetInventoryItem>) {
    // Split by semicolons: "0,8" ; "X" ; "items..."
    let parts: Vec<&str> = inv.split(';').collect();
    if parts.len() < 3 {
        return (0, Vec::new());
    }

    // Parse slot info from first part "start,count"
    let slot_info: Vec<&str> = parts[0].split(',').collect();
    let slot_count = if slot_info.len() >= 2 {
        slot_info[1].parse().unwrap_or(0)
    } else {
        0
    };

    // Parse items from third part (after X separator)
    let items_str = parts[2];
    let mut items = Vec::new();

    if !items_str.is_empty() {
        for item_str in items_str.split(',') {
            if item_str.is_empty() {
                continue;
            }
            // Check for unique ID format: "item_id#unique_id"
            if let Some(hash_pos) = item_str.find('#') {
                let item_id: i32 = item_str[..hash_pos].parse().unwrap_or(-1);
                let unique_id: Option<u64> = item_str[hash_pos + 1..].parse().ok();
                items.push(PetInventoryItem {
                    item_id,
                    unique_id,
                    stack_count: 0,
                });
            } else {
                let item_id: i32 = item_str.parse().unwrap_or(-1);
                items.push(PetInventoryItem {
                    item_id,
                    unique_id: None,
                    stack_count: 0,
                });
            }
        }
    }

    (slot_count, items)
}

/// Parse Pet element attributes into a Pet struct.
fn parse_pet_attributes(e: &quick_xml::events::BytesStart<'_>) -> Pet {
    let mut pet = Pet::default();

    for attr in e.attributes().flatten() {
        let key = String::from_utf8_lossy(attr.key.as_ref());
        let value = String::from_utf8_lossy(&attr.value);

        match key.as_ref() {
            "name" => pet.name = value.to_string(),
            "createdOn" => pet.created_on = value.to_string(),
            "skin" => pet.skin = value.parse().unwrap_or(0),
            "type" => pet.pet_type = value.parse().unwrap_or(0),
            "instanceId" => pet.instance_id = value.parse().unwrap_or(0),
            "maxAbilityPower" => pet.max_ability_power = value.parse().unwrap_or(0),
            "rarity" => pet.rarity = value.parse().unwrap_or(0),
            // Note: incInv is ignored - we rely solely on the inv attribute format
            // inv="0,8;X;items..." where 8 is the slot count
            "inv" => {
                let (slots, items) = parse_pet_inventory(&value);
                pet.inventory_slots = slots;
                pet.inventory = items;
            }
            _ => {}
        }
    }

    pet
}

/// Parse Ability element attributes.
fn parse_ability_attributes(e: &quick_xml::events::BytesStart<'_>) -> PetAbility {
    let mut ability = PetAbility::default();

    for attr in e.attributes().flatten() {
        let key = String::from_utf8_lossy(attr.key.as_ref());
        let value = String::from_utf8_lossy(&attr.value);

        match key.as_ref() {
            "type" => ability.ability_type = value.parse().unwrap_or(0),
            "power" => ability.power = value.parse().unwrap_or(0),
            _ => {}
        }
    }

    ability
}

/// Parse the char/list XML response into a list of characters.
/// This is a simplified parser that only returns character data.
/// Use `parse_account_data` for full account data including vault/storage.
pub fn parse_char_list(xml: &str) -> Result<Vec<RealmCharacter>, ParseError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut characters = Vec::new();
    let mut current_char: Option<RealmCharacter> = None;
    let mut current_pet: Option<Pet> = None;
    let mut current_element = String::new();
    let mut element_path: Vec<String> = Vec::new();
    let mut in_pet = false;
    let mut current_item_data_type: Option<i32> = None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                element_path.push(name.clone());
                current_element = name.clone();

                if name == "Char" {
                    let mut char = RealmCharacter::default();
                    // Try to get the id attribute
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"id" {
                            if let Ok(id_str) = std::str::from_utf8(&attr.value) {
                                char.char_id = id_str.parse().unwrap_or(0);
                            }
                        }
                    }
                    current_char = Some(char);
                } else if name == "Pet" {
                    in_pet = true;
                    current_pet = Some(parse_pet_attributes(&e));
                } else if name == "Ability" && in_pet {
                    // Parse ability from start tag (may have content)
                    if let Some(ref mut pet) = current_pet {
                        pet.abilities.push(parse_ability_attributes(&e));
                    }
                } else if name == "ItemData" {
                    // Parse ItemData type attribute for enchantment data
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"type" {
                            if let Ok(type_str) = std::str::from_utf8(&attr.value) {
                                current_item_data_type = type_str.parse().ok();
                            }
                        }
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                // Handle self-closing tags like <Ability type="407" power="100"/>
                if name == "Ability" && in_pet {
                    if let Some(ref mut pet) = current_pet {
                        pet.abilities.push(parse_ability_attributes(&e));
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                element_path.pop();

                if name == "Char" {
                    if let Some(mut char) = current_char.take() {
                        // Decode PCStats if present
                        if !char.pc_stats_raw.is_empty() {
                            char.stats = decode_pcstats(&char.pc_stats_raw);
                        }
                        characters.push(char);
                    }
                } else if name == "Pet" {
                    if let (Some(ref mut char), Some(pet)) = (&mut current_char, current_pet.take())
                    {
                        char.pet = Some(pet);
                    }
                    in_pet = false;
                } else if name == "ItemData" {
                    current_item_data_type = None;
                }
                current_element.clear();
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if text.is_empty() {
                    continue;
                }

                // Skip text content when inside Pet - all pet data comes from attributes
                if in_pet {
                    continue;
                }

                if let Some(ref mut char) = current_char {
                    // Check if we're inside UniqueItemInfo for ItemData
                    let in_unique_item_info = element_path.iter().any(|e| e == "UniqueItemInfo");

                    if in_unique_item_info && current_element == "ItemData" {
                        // Store unique item data for character's equipment enchantments.
                        // Multiple entries for the same item type are preserved in order.
                        if let Some(item_id) = current_item_data_type {
                            char.unique_item_info
                                .entry(item_id)
                                .or_default()
                                .push(text.clone());
                        }
                    } else {
                        match current_element.as_str() {
                            "ObjectType" => {
                                char.class_id = text.parse().unwrap_or(0);
                                char.class = CharacterClass::from_id(char.class_id);
                            }
                            "Level" => char.level = text.parse().unwrap_or(1),
                            "Texture" => char.skin = text.parse().unwrap_or(0),
                            "Tex1" => char.tex1 = text.parse().unwrap_or(0),
                            "Tex2" => char.tex2 = text.parse().unwrap_or(0),
                            "Exp" => char.exp = text.parse().unwrap_or(0),
                            "CurrentFame" => char.fame = text.parse().unwrap_or(0),
                            "Seasonal" => char.seasonal = text == "True",
                            "CrucibleActive" => char.crucible_active = !text.is_empty(),
                            "HasBackpack" => char.has_backpack = text == "1",
                            "BackpackSlots" => char.backpack_slots = text.parse().unwrap_or(0),
                            "Has3Quickslots" => char.has_3_quickslots = text == "1",
                            "Equipment" => {
                                char.equipment = text
                                    .split(',')
                                    .filter_map(|s| {
                                        // Handle format like "123#abc" - take number before #
                                        s.split('#').next()?.parse().ok()
                                    })
                                    .collect();
                            }
                            "EquipQS" => {
                                char.equip_qs = text.split(',').map(|s| s.to_string()).collect();
                            }
                            "CreationDate" => char.creation_date = text,
                            "MaxHitPoints" => char.max_hp = text.parse().unwrap_or(0),
                            "MaxMagicPoints" => char.max_mp = text.parse().unwrap_or(0),
                            "Attack" => char.attack = text.parse().unwrap_or(0),
                            "Defense" => char.defense = text.parse().unwrap_or(0),
                            "Speed" => char.speed = text.parse().unwrap_or(0),
                            "Dexterity" => char.dexterity = text.parse().unwrap_or(0),
                            "HpRegen" => char.vitality = text.parse().unwrap_or(0),
                            "MpRegen" => char.wisdom = text.parse().unwrap_or(0),
                            "PCStats" => char.pc_stats_raw = text,
                            "LDTimer" => {
                                char.loot_boost_secs = text
                                    .trim()
                                    .parse::<f64>()
                                    .ok()
                                    .map(|v| v.max(0.0) as u32)
                                    .filter(|s| *s > 0);
                            }
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(ParseError::Xml(format!(
                    "Error at position {}: {:?}",
                    reader.buffer_position(),
                    e
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(characters)
}

/// Account stars contributed by one class's best base fame. RotMG grants up to
/// 5 stars per class as base fame crosses 20 / 500 / 1500 / 5000 / 15000; the
/// account star total (shown by the Rank widget) is the sum across all classes.
fn class_fame_stars(base_fame: i32) -> i32 {
    const THRESHOLDS: [i32; 5] = [20, 500, 1500, 5000, 15000];
    THRESHOLDS.iter().filter(|&&t| base_fame >= t).count() as i32
}

/// Parse the char/list XML response into full account data including storage.
/// Returns characters, vault, gifts, potions, and materials.
pub fn parse_account_data(xml: &str) -> Result<AccountData, ParseError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut account_data = AccountData::default();
    let mut current_char: Option<RealmCharacter> = None;
    let mut current_pet: Option<Pet> = None;
    let mut current_element = String::new();
    let mut element_path: Vec<String> = Vec::new();
    let mut in_pet = false;
    let mut buf = Vec::new();

    // Track current ItemData attributes for UniqueItemInfo parsing
    let mut current_item_data_id: Option<String> = None;
    // Track ClassStats class attribute for exaltation parsing
    let mut current_class_stats_class: Option<i32> = None;
    // Root-scope (`<Chars>`) account-wide accelerator state. `<Accelerators>`
    // precedes `<Timestamp>` in the response, so collect the raw string and the
    // anchor separately and fold them into absolute expiries after the loop.
    let mut accelerators_raw: Option<String> = None;
    let mut server_timestamp: Option<i64> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                element_path.push(name.clone());
                current_element = name.clone();

                if name == "Char" {
                    let mut char = RealmCharacter::default();
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"id" {
                            if let Ok(id_str) = std::str::from_utf8(&attr.value) {
                                char.char_id = id_str.parse().unwrap_or(0);
                            }
                        }
                    }
                    current_char = Some(char);
                } else if name == "Pet" {
                    in_pet = true;
                    current_pet = Some(parse_pet_attributes(&e));
                } else if name == "Ability" && in_pet {
                    // Parse ability from start tag (may have content)
                    if let Some(ref mut pet) = current_pet {
                        pet.abilities.push(parse_ability_attributes(&e));
                    }
                } else if name == "ItemData" {
                    // Parse ItemData attributes for UniqueItemInfo (can be "type" or "id" attribute)
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"type" || attr.key.as_ref() == b"id" {
                            if let Ok(id_str) = std::str::from_utf8(&attr.value) {
                                current_item_data_id = Some(id_str.to_string());
                            }
                        }
                    }
                } else if name == "ClassStats" {
                    // Parse ClassStats class attribute for exaltation data
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"class" {
                            if let Ok(class_str) = std::str::from_utf8(&attr.value) {
                                current_class_stats_class = class_str.parse::<i32>().ok();
                            }
                        }
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                // Handle self-closing tags like <Ability type="407" power="100"/>
                if name == "Ability" && in_pet {
                    if let Some(ref mut pet) = current_pet {
                        pet.abilities.push(parse_ability_attributes(&e));
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                element_path.pop();

                if name == "Char" {
                    if let Some(mut char) = current_char.take() {
                        if !char.pc_stats_raw.is_empty() {
                            char.stats = decode_pcstats(&char.pc_stats_raw);
                        }
                        account_data.characters.push(char);
                    }
                } else if name == "Pet" {
                    if let (Some(ref mut char), Some(pet)) = (&mut current_char, current_pet.take())
                    {
                        char.pet = Some(pet);
                    }
                    in_pet = false;
                } else if name == "ItemData" {
                    current_item_data_id = None;
                } else if name == "ClassStats" {
                    current_class_stats_class = None;
                }
                current_element.clear();
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if text.is_empty() {
                    continue;
                }

                // Skip text content when inside Pet - all pet data comes from attributes
                if in_pet {
                    continue;
                }

                // Check if we're inside Account section for storage
                let in_account = element_path.iter().any(|e| e == "Account");
                let in_vault = element_path.iter().any(|e| e == "Vault");
                let in_material_storage = element_path.iter().any(|e| e == "MaterialStorage");
                let in_unique_item_info = element_path.iter().any(|e| {
                    e == "UniqueItemInfo"
                        || e == "UniqueGiftItemInfo"
                        || e == "UniqueTemporaryGiftItemInfo"
                });
                // PowerUpStats is at root level (under Chars), not inside Account
                let in_power_up_stats = element_path.iter().any(|e| e == "PowerUpStats");

                // Parse ClassStats exaltation data (inside PowerUpStats at root level)
                if in_power_up_stats && current_element == "ClassStats" {
                    if let Some(class_id) = current_class_stats_class {
                        if let Some(exaltation) = super::ClassExaltation::from_csv(class_id, &text)
                        {
                            account_data.exaltation_stats.insert(class_id, exaltation);
                        }
                    }
                    continue; // Skip further processing for ClassStats
                }

                if let Some(ref mut char) = current_char {
                    // Check if we're in UniqueItemInfo for this character
                    let char_unique_item_info = element_path.iter().any(|e| e == "UniqueItemInfo");

                    if char_unique_item_info && current_element == "ItemData" {
                        // Store unique item data for character's equipment.
                        // Multiple entries for the same item type are preserved in order.
                        if let Some(ref type_id) = current_item_data_id {
                            if let Ok(item_id) = type_id.parse::<i32>() {
                                char.unique_item_info
                                    .entry(item_id)
                                    .or_default()
                                    .push(text.clone());
                            }
                        }
                    } else {
                        match current_element.as_str() {
                            "ObjectType" => {
                                char.class_id = text.parse().unwrap_or(0);
                                char.class = CharacterClass::from_id(char.class_id);
                            }
                            "Level" => char.level = text.parse().unwrap_or(1),
                            "Texture" => char.skin = text.parse().unwrap_or(0),
                            "Tex1" => char.tex1 = text.parse().unwrap_or(0),
                            "Tex2" => char.tex2 = text.parse().unwrap_or(0),
                            "Exp" => char.exp = text.parse().unwrap_or(0),
                            "CurrentFame" => char.fame = text.parse().unwrap_or(0),
                            "Seasonal" => char.seasonal = text == "True",
                            "CrucibleActive" => char.crucible_active = !text.is_empty(),
                            "HasBackpack" => char.has_backpack = text == "1",
                            "BackpackSlots" => char.backpack_slots = text.parse().unwrap_or(0),
                            "Has3Quickslots" => char.has_3_quickslots = text == "1",
                            "Equipment" => {
                                char.equipment = text
                                    .split(',')
                                    .filter_map(|s| s.split('#').next()?.parse().ok())
                                    .collect();
                            }
                            "EquipQS" => {
                                char.equip_qs = text.split(',').map(|s| s.to_string()).collect();
                            }
                            "CreationDate" => char.creation_date = text,
                            "MaxHitPoints" => char.max_hp = text.parse().unwrap_or(0),
                            "MaxMagicPoints" => char.max_mp = text.parse().unwrap_or(0),
                            "Attack" => char.attack = text.parse().unwrap_or(0),
                            "Defense" => char.defense = text.parse().unwrap_or(0),
                            "Speed" => char.speed = text.parse().unwrap_or(0),
                            "Dexterity" => char.dexterity = text.parse().unwrap_or(0),
                            "HpRegen" => char.vitality = text.parse().unwrap_or(0),
                            "MpRegen" => char.wisdom = text.parse().unwrap_or(0),
                            "PCStats" => char.pc_stats_raw = text,
                            "LDTimer" => {
                                char.loot_boost_secs = text
                                    .trim()
                                    .parse::<f64>()
                                    .ok()
                                    .map(|v| v.max(0.0) as u32)
                                    .filter(|s| *s > 0);
                            }
                            _ => {}
                        }
                    }
                } else if in_account {
                    // Parse account-level storage
                    match current_element.as_str() {
                        "AccountId" => {
                            if account_data.account_id.is_none() && !text.trim().is_empty() {
                                account_data.account_id = Some(text.trim().to_string());
                            }
                        }
                        "Name" if !element_path.iter().any(|e| e == "Guild") => {
                            if account_data.account_name.is_none() && !text.trim().is_empty() {
                                account_data.account_name = Some(text.trim().to_string());
                            }
                        }
                        "Chest" if in_vault => {
                            account_data.vault.chests.push(VaultChest::parse(&text));
                        }
                        "Chest" if in_material_storage => {
                            account_data.materials.chests.push(VaultChest::parse(&text));
                        }
                        "Gifts" => {
                            account_data.gifts = GiftStorage::parse(&text);
                        }
                        "Potions" => {
                            account_data.potions = PotionStorage::parse(&text);
                        }
                        "ItemData" if in_unique_item_info => {
                            // Store the enchant data keyed by instance ID
                            if let Some(ref id) = current_item_data_id {
                                account_data.unique_item_info.insert(id.clone(), text);
                            }
                        }
                        "MaxNumChars" => {
                            account_data.max_num_chars = text.parse().unwrap_or(0);
                        }
                        "NextCharSlotPrice" => {
                            account_data.next_char_slot_price = text.parse().unwrap_or(0);
                        }
                        "OwnedSkins" => {
                            account_data.owned_skins_count =
                                text.split(',').filter(|s| !s.trim().is_empty()).count() as i32;
                        }
                        "Credits" => {
                            account_data.account_credits = text.trim().parse().ok();
                        }
                        "Fame" => {
                            account_data.account_fame = text.trim().parse().ok();
                        }
                        "BestBaseFame" => {
                            // RotMG has no direct account-star field in char/list;
                            // stars are earned per class as best base fame crosses
                            // fixed thresholds. Sum each class's contribution.
                            if let Ok(v) = text.trim().parse::<i32>() {
                                let earned = class_fame_stars(v);
                                account_data.account_star =
                                    Some(account_data.account_star.unwrap_or(0) + earned);
                            }
                        }
                        _ => {}
                    }
                } else {
                    // Root `<Chars>` scope (outside any `<Char>` or `<Account>`):
                    // account-wide accelerators and the server time anchor. Guard
                    // on the direct parent being `<Chars>` so a `Timestamp` or
                    // `Accelerators` nested in another root section (e.g.
                    // PowerUpStats) can't be mistaken for these direct children.
                    let parent_is_chars = element_path
                        .iter()
                        .rev()
                        .nth(1)
                        .map(|p| p == "Chars")
                        .unwrap_or(false);
                    if parent_is_chars {
                        match current_element.as_str() {
                            "Accelerators" => {
                                if !text.trim().is_empty() {
                                    accelerators_raw = Some(text.trim().to_string());
                                }
                            }
                            "Timestamp" => {
                                server_timestamp = text.trim().parse::<i64>().ok();
                            }
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(ParseError::Xml(format!(
                    "Error at position {}: {:?}",
                    reader.buffer_position(),
                    e
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    // Apply UniqueItemInfo enchant data to all storage items
    account_data.apply_unique_item_info();

    // Fold account-wide accelerators into absolute expiries using the server
    // timestamp anchor (requires both the raw list and the `<Timestamp>`).
    account_data.server_timestamp = server_timestamp;
    if let Some(raw) = accelerators_raw {
        account_data.accelerators = super::parse_accelerators(&raw, server_timestamp);
    }

    Ok(account_data)
}

/// Error type for XML parsing.
#[derive(Debug)]
pub enum ParseError {
    /// XML parsing error
    Xml(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Xml(msg) => write!(f, "XML parse error: {}", msg),
        }
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_char_list() {
        let xml = r#"
            <Chars>
                <Char id="123">
                    <ObjectType>782</ObjectType>
                    <Level>20</Level>
                    <CurrentFame>1500</CurrentFame>
                    <MaxHitPoints>670</MaxHitPoints>
                    <MaxMagicPoints>385</MaxMagicPoints>
                    <Attack>75</Attack>
                    <Defense>25</Defense>
                    <Speed>50</Speed>
                    <Dexterity>75</Dexterity>
                    <HpRegen>40</HpRegen>
                    <MpRegen>60</MpRegen>
                </Char>
            </Chars>
        "#;

        let chars = parse_char_list(xml).unwrap();
        assert_eq!(chars.len(), 1);

        let char = &chars[0];
        assert_eq!(char.char_id, 123);
        assert_eq!(char.class, CharacterClass::Wizard);
        assert_eq!(char.level, 20);
        assert_eq!(char.fame, 1500);
        assert_eq!(char.max_hp, 670);
        assert_eq!(char.attack, 75);
    }

    #[test]
    fn test_parse_multiple_chars() {
        let xml = r#"
            <Chars>
                <Char id="1">
                    <ObjectType>768</ObjectType>
                    <Level>20</Level>
                </Char>
                <Char id="2">
                    <ObjectType>798</ObjectType>
                    <Level>15</Level>
                </Char>
            </Chars>
        "#;

        let chars = parse_char_list(xml).unwrap();
        assert_eq!(chars.len(), 2);
        assert_eq!(chars[0].class, CharacterClass::Rogue);
        assert_eq!(chars[1].class, CharacterClass::Knight);
    }

    #[test]
    fn test_parse_ld_timer() {
        let xml = r#"
            <Chars>
                <Char id="1">
                    <ObjectType>782</ObjectType>
                    <LDTimer>3600</LDTimer>
                </Char>
                <Char id="2">
                    <ObjectType>782</ObjectType>
                    <LDTimer>0</LDTimer>
                </Char>
                <Char id="3">
                    <ObjectType>782</ObjectType>
                </Char>
            </Chars>
        "#;

        let chars = parse_char_list(xml).unwrap();
        assert_eq!(chars[0].loot_boost_secs, Some(3600));
        assert_eq!(chars[1].loot_boost_secs, None);
        assert_eq!(chars[2].loot_boost_secs, None);
    }

    #[test]
    fn test_parse_equipment() {
        let xml = r#"
            <Chars>
                <Char id="1">
                    <ObjectType>782</ObjectType>
                    <Equipment>3000,3001,3002,3003</Equipment>
                </Char>
            </Chars>
        "#;

        let chars = parse_char_list(xml).unwrap();
        assert_eq!(chars[0].equipment, vec![3000, 3001, 3002, 3003]);
    }

    #[test]
    fn test_parse_account_credits_fame_star() {
        let xml = r#"
            <Chars>
                <Char id="1">
                    <ObjectType>782</ObjectType>
                    <CurrentFame>999</CurrentFame>
                </Char>
                <Account>
                    <AccountId>ACC-42</AccountId>
                    <Name>Tester</Name>
                    <Credits>123456</Credits>
                    <Stats>
                        <ClassStats objectType="0x300"><BestBaseFame>2500</BestBaseFame></ClassStats>
                        <ClassStats objectType="0x301"><BestBaseFame>300</BestBaseFame></ClassStats>
                        <ClassStats objectType="0x302"><BestBaseFame>10</BestBaseFame></ClassStats>
                        <Fame>78900</Fame>
                    </Stats>
                    <MaxNumChars>10</MaxNumChars>
                    <OwnedSkins>1,2,3</OwnedSkins>
                </Account>
            </Chars>
        "#;

        let data = parse_account_data(xml).unwrap();
        assert_eq!(data.account_credits, Some(123456));
        assert_eq!(data.account_fame, Some(78900));
        // 2500 -> 3 stars, 300 -> 1 star, 10 -> 0 stars => 4 total.
        assert_eq!(data.account_star, Some(4));
        assert_eq!(data.max_num_chars, 10);
        assert_eq!(data.owned_skins_count, 3);
        assert_eq!(data.account_id.as_deref(), Some("ACC-42"));
        assert_eq!(data.account_name.as_deref(), Some("Tester"));
    }

    #[test]
    fn test_parse_account_accelerators() {
        // Real-world shape: <Accelerators> precedes <Timestamp>, both at the
        // <Chars> root (after </Account>). 751 = 0x2ef dust boost.
        let xml = r#"
            <Chars>
                <Char id="1"><ObjectType>782</ObjectType></Char>
                <Account><AccountId>ACC-1</AccountId></Account>
                <Accelerators>751,69974</Accelerators>
                <Timestamp>1788046329</Timestamp>
            </Chars>
        "#;

        let data = parse_account_data(xml).unwrap();
        assert_eq!(data.server_timestamp, Some(1788046329));
        assert_eq!(data.accelerators.len(), 1);
        let acc = data.accelerators[0];
        assert_eq!(acc.object_type, 0x2ef);
        // Absolute expiry = timestamp + remaining seconds.
        assert_eq!(acc.expires_at, 1788046329 + 69974);
        // Live-countdown helpers.
        assert_eq!(acc.remaining_secs(1788046329), 69974);
        assert_eq!(acc.remaining_secs(acc.expires_at + 100), 0);
        assert!(acc.is_active(1788046329));
        assert!(!acc.is_active(acc.expires_at));
    }

    #[test]
    fn test_parse_accelerators_helper() {
        use super::super::parse_accelerators;
        // Multiple pairs fold into multiple accelerators.
        let accs = parse_accelerators("751,100,762,50", Some(1000));
        assert_eq!(accs.len(), 2);
        assert_eq!(accs[0].object_type, 751);
        assert_eq!(accs[0].expires_at, 1100);
        assert_eq!(accs[1].object_type, 762);
        assert_eq!(accs[1].expires_at, 1050);
        // No timestamp anchor -> nothing (we refuse to guess expiry).
        assert!(parse_accelerators("751,100", None).is_empty());
        // Odd/garbage payloads are ignored gracefully.
        assert!(parse_accelerators("", Some(1000)).is_empty());
        assert_eq!(parse_accelerators("751", Some(1000)).len(), 0);
        // A garbage token invalidates only its own pair, not the alignment of
        // the following valid pair.
        let mixed = parse_accelerators("751,bad,762,50", Some(1000));
        assert_eq!(mixed.len(), 1);
        assert_eq!(mixed[0].object_type, 762);
        assert_eq!(mixed[0].expires_at, 1050);
    }

    #[test]
    fn test_accelerator_info_dust() {
        use super::super::accelerator_info;
        let info = accelerator_info(0x2ef).expect("dust accelerator known");
        assert_eq!(info.accelerator_id, "DROPDUSTVOLUMEBOOST");
        assert_eq!(info.bonus_pct, 60.0);
        assert_eq!(info.duration_secs, 86_400);
        assert!(accelerator_info(0x1234).is_none());
    }
}
