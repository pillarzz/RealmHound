//! Account storage data structures (vault, gifts, potions, materials).

use std::collections::HashMap;
use base64::{engine::general_purpose::URL_SAFE, Engine as _};

/// An item in storage (can have an instance ID for unique items).
#[derive(Debug, Clone, Default)]
pub struct StorageItem {
    /// Item type ID (-1 = empty slot)
    pub item_id: i32,
    /// Instance ID for unique items (None for stackable/normal items)
    pub instance_id: Option<String>,
    /// Enchant data (base64 encoded, from UniqueItemInfo)
    pub enchant_data: Option<String>,
}

impl StorageItem {
    /// Parse an item from the comma-separated format.
    /// Format: "itemId" or "itemId#instanceId"
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s.is_empty() || s == "-1" {
            return Self {
                item_id: -1,
                instance_id: None,
                enchant_data: None,
            };
        }

        if let Some(hash_pos) = s.find('#') {
            let (id_part, instance_part) = s.split_at(hash_pos);
            Self {
                item_id: id_part.parse().unwrap_or(-1),
                instance_id: Some(instance_part[1..].to_string()),
                enchant_data: None,
            }
        } else {
            Self {
                item_id: s.parse().unwrap_or(-1),
                instance_id: None,
                enchant_data: None,
            }
        }
    }

    /// Check if this slot is empty.
    pub fn is_empty(&self) -> bool {
        self.item_id == -1
    }

    /// Get the number of enchants on this item (0-4).
    /// Returns 0 if no enchant data or parsing fails.
    pub fn enchant_count(&self) -> usize {
        self.enchant_ids().len()
    }

    /// Get the enchant type IDs on this item.
    /// Returns a vector of up to 4 enchant IDs (as u16).
    pub fn enchant_ids(&self) -> Vec<u16> {
        self.enchant_slots().filled
    }

    /// Decode this item's enchant-slot breakdown (filled, empty, locked).
    /// Returns the default (no slots) when there is no enchant data.
    pub fn enchant_slots(&self) -> EnchantSlots {
        self.enchant_data
            .as_ref()
            .map(|data| parse_enchant_slots(data))
            .unwrap_or_default()
    }

    /// Total number of enchant slots (filled + empty + locked). This is the
    /// item's rarity tier, so empty-but-slotted items count here too.
    pub fn total_enchant_slots(&self) -> usize {
        self.enchant_slots().total()
    }
}

/// A chest in the vault (8 slots per chest).
#[derive(Debug, Clone, Default)]
pub struct VaultChest {
    /// Items in this chest (8 slots)
    pub items: Vec<StorageItem>,
}

impl VaultChest {
    /// Parse a chest from the comma-separated item list.
    pub fn parse(s: &str) -> Self {
        let items = s.split(',').map(StorageItem::parse).collect();
        Self { items }
    }

    /// Count non-empty slots.
    pub fn item_count(&self) -> usize {
        self.items.iter().filter(|i| !i.is_empty()).count()
    }

    /// Get the total slot count (should be 8).
    pub fn slot_count(&self) -> usize {
        self.items.len()
    }
}

/// Account vault storage (multiple 8-slot chests).
#[derive(Debug, Clone, Default)]
pub struct AccountVault {
    /// Vault chests (8 items per chest)
    pub chests: Vec<VaultChest>,
}

impl AccountVault {
    /// Total number of vault slots.
    pub fn total_slots(&self) -> usize {
        self.chests.iter().map(|c| c.slot_count()).sum()
    }

    /// Total number of items stored.
    pub fn total_items(&self) -> usize {
        self.chests.iter().map(|c| c.item_count()).sum()
    }

    /// Number of chests.
    pub fn chest_count(&self) -> usize {
        self.chests.len()
    }
}

/// Gift chest storage (flat array, unlimited).
#[derive(Debug, Clone, Default)]
pub struct GiftStorage {
    /// All gift items as a flat array
    pub items: Vec<StorageItem>,
}

impl GiftStorage {
    /// Parse gifts from comma-separated format.
    pub fn parse(s: &str) -> Self {
        if s.trim().is_empty() {
            return Self { items: Vec::new() };
        }
        let items = s
            .split(',')
            .map(StorageItem::parse)
            .filter(|i| !i.is_empty())
            .collect();
        Self { items }
    }

    /// Total gift count.
    pub fn count(&self) -> usize {
        self.items.len()
    }
}

/// Potion storage (16 slots).
#[derive(Debug, Clone, Default)]
pub struct PotionStorage {
    /// Potion items (16 slots)
    pub items: Vec<StorageItem>,
}

impl PotionStorage {
    /// Parse potions from comma-separated format.
    pub fn parse(s: &str) -> Self {
        if s.trim().is_empty() {
            return Self { items: Vec::new() };
        }
        let items = s.split(',').map(StorageItem::parse).collect();
        Self { items }
    }

    /// Count non-empty slots.
    pub fn item_count(&self) -> usize {
        self.items.iter().filter(|i| !i.is_empty()).count()
    }

    /// Total slot count.
    pub fn slot_count(&self) -> usize {
        self.items.len()
    }
}

/// Material storage (multiple 8-slot chests).
#[derive(Debug, Clone, Default)]
pub struct MaterialStorage {
    /// Material chests (8 items per chest)
    pub chests: Vec<VaultChest>,
}

impl MaterialStorage {
    /// Total number of material slots.
    pub fn total_slots(&self) -> usize {
        self.chests.iter().map(|c| c.slot_count()).sum()
    }

    /// Total number of materials stored.
    pub fn total_items(&self) -> usize {
        self.chests.iter().map(|c| c.item_count()).sum()
    }

    /// Number of material chests.
    pub fn chest_count(&self) -> usize {
        self.chests.len()
    }
}

/// Complete account data including characters and storage.
#[derive(Debug, Clone, Default)]
pub struct AccountData {
    /// Characters on the account
    pub characters: Vec<super::RealmCharacter>,
    /// Exaltation progress per class (keyed by class_type ID)
    pub exaltation_stats: HashMap<i32, super::ClassExaltation>,
    /// Vault storage
    pub vault: AccountVault,
    /// Gift chest contents
    pub gifts: GiftStorage,
    /// Potion storage
    pub potions: PotionStorage,
    /// Material storage
    pub materials: MaterialStorage,
    /// Unique item info map: instance_id -> enchant_data (base64)
    pub unique_item_info: HashMap<String, String>,
    /// Maximum number of character slots on the account (`maxNumChars`).
    pub max_num_chars: i32,
    /// Price (fame) of the next character slot (`NextCharSlotPrice`).
    pub next_char_slot_price: i32,
    /// Number of skins owned (`OwnedSkins`, comma-separated id list).
    pub owned_skins_count: i32,
    /// Skin object ids the account owns (`OwnedSkins`), for the tooltip's
    /// account-owned tag. Empty when the response carried no list.
    pub owned_skin_ids: Vec<i32>,
    /// Emote object ids the account owns (`OwnedEmotes`).
    pub owned_emote_ids: Vec<i32>,
    /// Pet-skin object ids the account owns (`account/getOwnedPetSkins`, a
    /// separate request: char/list carries no pet-skin list).
    pub owned_pet_skin_ids: Vec<i32>,
    /// Item ids whose forge blueprint is unlocked in the regular forge
    /// (`RegularForgeFireBlueprints`, the same set packet 120 sends).
    pub regular_forge_blueprints: Vec<i32>,
    /// Account gold (`Credits`), when present in the char/list response.
    pub account_credits: Option<i32>,
    /// Account fame (`Fame`), when present in the char/list response.
    pub account_fame: Option<i32>,
    /// Account rank / star count, derived by summing per-class star thresholds
    /// from the char/list `<Stats>` block (no direct field exists).
    pub account_star: Option<i32>,
    /// Owning account id (`<AccountId>` in the char/list `<Account>` block).
    /// Used to prevent a mule/alt account's data from overwriting the main.
    pub account_id: Option<String>,
    /// Owning account name (`<Name>` in the char/list `<Account>` block).
    pub account_name: Option<String>,
    /// Server Unix timestamp (`<Timestamp>` at the `<Chars>` root), used to
    /// anchor account-wide accelerator countdowns to an absolute expiry.
    pub server_timestamp: Option<i64>,
    /// Active account-wide accelerators (`<Accelerators>` at the `<Chars>`
    /// root), e.g. the Acc Dust Chance Day potion. Empty when none are active
    /// or when no `<Timestamp>` anchor was present to compute expiry.
    pub accelerators: Vec<AccountAccelerator>,
}

/// An active account-wide accelerator ("potion") parsed from the char/list
/// `<Accelerators>` field. Unlike the per-character XP/loot timers, these apply
/// to the whole account and persist across characters. The wire format is a
/// flat comma-separated list of `objectType,secondsRemaining` pairs, where the
/// seconds are relative to the response's `<Timestamp>`; we fold that into an
/// absolute [`expires_at`](Self::expires_at) so the countdown stays correct no
/// matter when it is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccountAccelerator {
    /// Object type of the accelerator item (e.g. `0x2ef` = Acc Dust Chance Day).
    pub object_type: i32,
    /// Absolute expiry as a Unix timestamp (seconds): the char/list
    /// `<Timestamp>` plus the accelerator's remaining seconds.
    pub expires_at: i64,
    /// True when the expiry is a live estimate from an activation packet (id
    /// 153) rather than an authoritative API value. The next API refresh
    /// replaces it with the exact remaining time. Defaults to `false` so cached
    /// entries and API-derived values stay authoritative.
    #[serde(default)]
    pub estimated: bool,
}

impl AccountAccelerator {
    /// Seconds remaining until expiry relative to `now_unix` (0 once expired).
    pub fn remaining_secs(&self, now_unix: i64) -> u32 {
        u32::try_from(self.expires_at.saturating_sub(now_unix).max(0)).unwrap_or(u32::MAX)
    }

    /// Whether the accelerator is still active at `now_unix`.
    pub fn is_active(&self, now_unix: i64) -> bool {
        self.expires_at > now_unix
    }
}

/// Static display metadata for a known account-wide accelerator, mirroring the
/// `<Activate>StartAccelerator</Activate>` nodes in `equip.xml`. Only the small
/// set of account-scope accelerators is listed; per-character boosts (XP, loot
/// drop, loot tier) are delivered separately via the per-`<Char>` timers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AcceleratorInfo {
    /// Object type of the accelerator item.
    pub object_type: i32,
    /// `acceleratorId` from equip.xml (e.g. `DROPDUSTVOLUMEBOOST`).
    pub accelerator_id: &'static str,
    /// Bonus magnitude in percent (equip.xml `mult` * 100).
    pub bonus_pct: f32,
    /// Short human label for the thing being boosted (e.g. `dust dropped`).
    pub label: &'static str,
    /// Total active duration in seconds (equip.xml `duration`). Used to estimate
    /// an expiry when only an activation packet (which carries no timer) is seen.
    pub duration_secs: i64,
}

/// Look up display metadata for a known account-wide accelerator object type.
/// Returns `None` for unknown types so callers can render a generic fallback.
pub fn accelerator_info(object_type: i32) -> Option<AcceleratorInfo> {
    match object_type {
        // 0x2ef = 751 "Acc Dust Chance Day": +60% dust dropped for 24h.
        0x2ef => Some(AcceleratorInfo {
            object_type: 0x2ef,
            accelerator_id: "DROPDUSTVOLUMEBOOST",
            bonus_pct: 60.0,
            label: "dust dropped",
            duration_secs: 86_400,
        }),
        _ => None,
    }
}

/// Parse the raw `<Accelerators>` payload (`"type,secs[,type,secs...]"`) plus a
/// server `<Timestamp>` anchor into absolute-expiry accelerators. Returns an
/// empty vec when the anchor is missing or the payload has no valid pairs.
pub fn parse_accelerators(raw: &str, server_timestamp: Option<i64>) -> Vec<AccountAccelerator> {
    let Some(anchor) = server_timestamp else {
        return Vec::new();
    };
    // Pair up the raw string tokens first, then parse both sides of each pair.
    // Doing this on string tokens (rather than after filtering to numbers) keeps
    // a malformed token from shifting the type/seconds alignment of later pairs.
    let tokens: Vec<&str> = raw.split(',').map(|s| s.trim()).collect();
    tokens
        .chunks_exact(2)
        .filter_map(|pair| {
            let object_type = pair[0].parse::<i32>().ok()?;
            let remaining = pair[1].parse::<i64>().ok()?.max(0);
            let expires_at = anchor.checked_add(remaining)?;
            Some(AccountAccelerator {
                object_type,
                expires_at,
                estimated: false,
            })
        })
        .collect()
}

impl AccountData {
    /// Get total items across all storage types.
    pub fn total_stored_items(&self) -> usize {
        self.vault.total_items()
            + self.gifts.count()
            + self.potions.item_count()
            + self.materials.total_items()
    }

    /// Apply UniqueItemInfo enchant data to all storage items.
    /// Call this after parsing to populate enchant_data fields.
    pub fn apply_unique_item_info(&mut self) {
        // Apply to vault items
        for chest in &mut self.vault.chests {
            for item in &mut chest.items {
                if let Some(ref instance_id) = item.instance_id {
                    if let Some(data) = self.unique_item_info.get(instance_id) {
                        item.enchant_data = Some(data.clone());
                    }
                }
            }
        }

        // Apply to gift items
        for item in &mut self.gifts.items {
            if let Some(ref instance_id) = item.instance_id {
                if let Some(data) = self.unique_item_info.get(instance_id) {
                    item.enchant_data = Some(data.clone());
                }
            }
        }

        // Apply to potion items
        for item in &mut self.potions.items {
            if let Some(ref instance_id) = item.instance_id {
                if let Some(data) = self.unique_item_info.get(instance_id) {
                    item.enchant_data = Some(data.clone());
                }
            }
        }

        // Apply to material items
        for chest in &mut self.materials.chests {
            for item in &mut chest.items {
                if let Some(ref instance_id) = item.instance_id {
                    if let Some(data) = self.unique_item_info.get(instance_id) {
                        item.enchant_data = Some(data.clone());
                    }
                }
            }
        }
    }
}

/// Decode RotMG's URL-safe Base64 encoding, accepting unpadded input.
fn decode_rotmg_base64(s: &str) -> Option<Vec<u8>> {
    let mut padded = s.to_owned();
    padded.push_str(&"=".repeat((4 - padded.len() % 4) % 4));
    URL_SAFE.decode(padded).ok()
}

/// All information decoded from an item's encoded enchant string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnchantSlots {
    /// Type-1 key grade, or empty when absent/invalid.
    pub key_grade: String,
    /// Type-1 key modifier IDs.
    pub key_mods: Vec<u16>,

    /// Type-2 enchant IDs, including sentinel values:
    /// `-3` = nonexistent slot, `-2` = locked, `-1` = empty.
    pub enchants: Vec<i16>,
    /// Non-negative type-2 enchant IDs only.
    pub filled: Vec<u16>,
    /// Number of `-1` type-2 enchant entries.
    pub empty: u8,
    /// Number of `-2` type-2 enchant entries.
    pub locked: u8,

    /// Type-3 unlocked enchant IDs.
    pub unlocked_enchants: Vec<i16>,
    /// Type-4 legacy reroll count; `-1` means absent.
    pub times_rerolled_legacy: i16,
    /// Type-5 meter values, indexed using the encoded meter ID.
    pub meters: Vec<i16>,
}

impl Default for EnchantSlots {
    fn default() -> Self {
        Self {
            key_grade: String::new(),
            key_mods: Vec::new(),
            enchants: Vec::new(),
            filled: Vec::new(),
            empty: 0,
            locked: 0,
            unlocked_enchants: Vec::new(),
            times_rerolled_legacy: -1,
            meters: Vec::new(),
        }
    }
}

impl EnchantSlots {
    /// Total filled, empty, and locked enchant slots.
    pub fn total(&self) -> usize {
        self.filled.len() + self.empty as usize + self.locked as usize
    }

    /// Meter
    pub fn meter_value(&self) -> i16 {
        self.meters.first().copied().unwrap_or(-1)
    }

    pub fn meter(&self, meter_id: usize) -> Option<i16> {
        self.meters.get(meter_id).copied()
    }
}

/// Parse every record from an encoded enchant string.
pub fn parse_enchant_slots(code: &str) -> EnchantSlots {
    if code.is_empty() {
        return EnchantSlots::default();
    }

    let Some(bytes) = decode_rotmg_base64(code) else {
        return EnchantSlots::default();
    };

    parse_enchant_slots_bytes(&bytes)
}

fn parse_enchant_slots_bytes(bytes: &[u8]) -> EnchantSlots {
    let mut slots = EnchantSlots::default();
    let mut pos = 0;

    // Skip version. If absent, there are no records to parse.
    if read_u8(bytes, &mut pos).is_some() {
        'parse: while let Some(parse_type) = read_u8(bytes, &mut pos) {
            match parse_type {
                1 => {
                    let (Some(grade), Some(mod_count)) =
                        (read_u8(bytes, &mut pos), read_u8(bytes, &mut pos))
                    else {
                        break 'parse;
                    };

                    slots.key_grade = if grade.is_ascii_alphabetic() {
                        char::from(grade).to_string()
                    } else {
                        "#".to_owned()
                    };

                    for _ in 0..mod_count {
                        let Some(mod_id) = read_u16_le(bytes, &mut pos) else {
                            break 'parse;
                        };
                        slots.key_mods.push(mod_id);
                    }
                }

                2 => {
                    let Some(enchant_count) = read_u8(bytes, &mut pos) else {
                        break 'parse;
                    };

                    for _ in 0..enchant_count {
                        let Some(enchant_id) = read_i16_le(bytes, &mut pos) else {
                            break 'parse;
                        };

                        slots.enchants.push(enchant_id);

                        match enchant_id {
                            -2 => slots.locked = slots.locked.saturating_add(1),
                            -1 => slots.empty = slots.empty.saturating_add(1),
                            id if id >= 0 => slots.filled.push(id as u16),
                            -3 => {} // Does not stop slot or record parsing.
                            _ => {}
                        }
                    }
                }

                3 => {
                    let Some(enchant_count) = read_i16_le(bytes, &mut pos) else {
                        break 'parse;
                    };

                    for _ in 0..enchant_count.max(0) as usize {
                        let Some(enchant_id) = read_i16_le(bytes, &mut pos) else {
                            break 'parse;
                        };
                        slots.unlocked_enchants.push(enchant_id);
                    }
                }

                4 => {
                    let Some(count) = read_u8(bytes, &mut pos) else {
                        break 'parse;
                    };
                    slots.times_rerolled_legacy = i16::from(count);
                }

                5 => {
                    let Some(meter_count) = read_u8(bytes, &mut pos) else {
                        break 'parse;
                    };

                    for _ in 0..meter_count {
                        let (Some(meter_id), Some(meter_value)) =
                            (read_u8(bytes, &mut pos), read_i16_le(bytes, &mut pos))
                        else {
                            break 'parse;
                        };

                        let index = usize::from(meter_id).min(slots.meters.len());
                        slots.meters.insert(index, meter_value);
                    }
                }

                // Unknown type: consume only its tag, then continue.
                _ => {}
            }
        }
    }

    slots
}

fn read_u8(bytes: &[u8], pos: &mut usize) -> Option<u8> {
    let value = *bytes.get(*pos)?;
    *pos += 1;
    Some(value)
}

fn read_u16_le(bytes: &[u8], pos: &mut usize) -> Option<u16> {
    let value = u16::from_le_bytes([*bytes.get(*pos)?, *bytes.get(*pos + 1)?]);
    *pos += 2;
    Some(value)
}

fn read_i16_le(bytes: &[u8], pos: &mut usize) -> Option<i16> {
    read_u16_le(bytes, pos).map(|value| value as i16)
}

/// Return only non-negative type-2 enchant IDs.
pub fn parse_enchant_ids(code: &str) -> Vec<u16> {
    parse_enchant_slots(code).filled
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a decoded enchant blob: header byte + LE type 1026 + LE i16 slots.
    fn blob(slots: &[i16]) -> Vec<u8> {
        let mut bytes = vec![0u8, 0x02, 0x04]; // header + type 1026 (0x0402)
        for &s in slots {
            let v = s as u16;
            bytes.push((v & 0xFF) as u8);
            bytes.push((v >> 8) as u8);
        }
        bytes
    }

    #[test]
    fn test_parse_slots_single_filled() {
        // Enchanted Queen's Stinger: [1458, -3, -3, -3, 5]
        let s = parse_enchant_slots_bytes(&blob(&[1458, -3, -3, -3, 5]));
        assert_eq!(s.filled, vec![1458]);
        assert_eq!(s.empty, 0);
        assert_eq!(s.locked, 0);
        assert_eq!(s.total(), 1);
    }

    #[test]
    fn test_parse_slots_single_empty() {
        // Unenchanted Queen's Stinger: [-1, -3, -3, -3]
        let s = parse_enchant_slots_bytes(&blob(&[-1, -3, -3, -3]));
        assert!(s.filled.is_empty());
        assert_eq!(s.empty, 1);
        assert_eq!(s.locked, 0);
        assert_eq!(s.total(), 1);
    }

    #[test]
    fn test_parse_slots_three_filled() {
        let s = parse_enchant_slots_bytes(&blob(&[781, 1160, 1457, -3]));
        assert_eq!(s.filled, vec![781, 1160, 1457]);
        assert_eq!(s.total(), 3);
    }

    #[test]
    fn test_parse_slots_mixed_and_locked() {
        // filled + empty + locked, all four slots real
        let s = parse_enchant_slots_bytes(&blob(&[1458, -1, -2, -1]));
        assert_eq!(s.filled, vec![1458]);
        assert_eq!(s.empty, 2);
        assert_eq!(s.locked, 1);
        assert_eq!(s.total(), 4);
    }

    #[test]
    fn test_parse_slots_empty_string() {
        let s = parse_enchant_slots("");
        assert_eq!(s, EnchantSlots::default());
        assert_eq!(s.total(), 0);
    }

    #[test]
    fn test_parse_slots_wrong_type_ignored() {
        // type != 1026 -> no slots
        let bytes = vec![0u8, 0x00, 0x00, 0x01, 0x00];
        assert_eq!(parse_enchant_slots_bytes(&bytes), EnchantSlots::default());
    }

    #[test]
    fn test_parse_ids_wrapper_matches_filled() {
        let bytes = blob(&[1458, -1, -3, -3]);
        let ids = parse_enchant_slots_bytes(&bytes).filled;
        assert_eq!(ids, vec![1458]);
    }
}
