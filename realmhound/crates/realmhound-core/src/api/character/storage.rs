//! Account storage data structures (vault, gifts, potions, materials).

use std::collections::HashMap;

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
        self.enchant_data
            .as_ref()
            .map(|data| parse_enchant_ids(data))
            .unwrap_or_default()
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

/// Decode a URL-safe base64 string (RotMG's "6-bit string" encoding).
/// This uses `-` and `_` instead of `+` and `/`.
fn decode_rotmg_base64(s: &str) -> Option<Vec<u8>> {
    let padding_idx = s.find('=').unwrap_or(s.len());
    let padding = s.len() - padding_idx;
    let output_len = (s.len() / 4) * 3 - padding;

    if s.len() % 4 != 0 {
        return None;
    }

    let mut output = vec![0u8; output_len];
    let mut o = 0;

    let chars: Vec<char> = s.chars().collect();
    for i in (0..s.len()).step_by(4) {
        let v1 = char_value(chars[i]);
        let v2 = char_value(chars[i + 1]);
        let c3 = chars[i + 2];
        let c4 = chars[i + 3];
        let v3 = char_value(c3);
        let v4 = char_value(c4);

        output[o] = ((v1 << 2) | (v2 >> 4)) as u8;
        if c3 != '=' && o + 1 < output.len() {
            output[o + 1] = (((v2 & 0x0F) << 4) | (v3 >> 2)) as u8;
            if c4 != '=' && o + 2 < output.len() {
                output[o + 2] = (((v3 & 0x03) << 6) | v4) as u8;
            }
        }
        o += 3;
    }

    Some(output)
}

/// Convert a character to its 6-bit value (RotMG's custom base64 alphabet).
fn char_value(c: char) -> u8 {
    match c {
        '0'..='9' => (c as u8) - b'0' + 52, // 0-9 -> 52-61
        'A'..='Z' => (c as u8) - b'A',      // A-Z -> 0-25
        'a'..='z' => (c as u8) - b'a' + 26, // a-z -> 26-51
        '-' => 62,                          // URL-safe replacement for +
        '_' => 63,                          // URL-safe replacement for /
        '=' => 0,                           // padding
        _ => 0,
    }
}

/// Per-slot breakdown of an item's enchant slots, decoded from an encoded
/// enchant string.
///
/// Slot markers in the encoded data: a value `>= 0` is a filled enchant, `-1`
/// is an empty (unlocked) slot, `-2` is a locked slot, and `-3` marks the end
/// of the item's real slots (any trailing shorts are metadata).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnchantSlots {
    /// Enchant type IDs occupying filled slots, in slot order.
    pub filled: Vec<u16>,
    /// Number of empty (unlocked, unenchanted) slots.
    pub empty: u8,
    /// Number of locked slots.
    pub locked: u8,
}

impl EnchantSlots {
    /// Total number of real enchant slots (filled + empty + locked).
    pub fn total(&self) -> usize {
        self.filled.len() + self.empty as usize + self.locked as usize
    }
}

/// Parse the per-slot enchant breakdown from an encoded enchant string.
///
/// Reads up to 4 slots, stopping at the first `-3` (slot does not exist).
pub fn parse_enchant_slots(code: &str) -> EnchantSlots {
    if code.is_empty() {
        return EnchantSlots::default();
    }

    match decode_rotmg_base64(code) {
        Some(bytes) => parse_enchant_slots_bytes(&bytes),
        None => EnchantSlots::default(),
    }
}

/// Parse the per-slot enchant breakdown from decoded enchant bytes.
fn parse_enchant_slots_bytes(bytes: &[u8]) -> EnchantSlots {
    let mut slots = EnchantSlots::default();

    // Skip header byte; need at least header + 2-byte type.
    if bytes.len() < 3 {
        return slots;
    }

    // Read type as little-endian short (bytes 1-2). Type 1026 (0x0402) is enchant data.
    let type_val = (bytes[1] as u16) | ((bytes[2] as u16) << 8);
    if type_val != 1026 {
        return slots;
    }

    let mut pos = 3;
    let mut read = 0;
    while pos + 1 < bytes.len() && read < 4 {
        let slot = (bytes[pos] as i16) | ((bytes[pos + 1] as i16) << 8);
        pos += 2;
        read += 1;

        match slot {
            -3 => break, // slot does not exist: no further real slots
            -2 => slots.locked = slots.locked.saturating_add(1),
            -1 => slots.empty = slots.empty.saturating_add(1),
            id if id >= 0 => slots.filled.push(id as u16),
            _ => {}
        }
    }

    slots
}

/// Parse the filled enchant type IDs from an encoded enchant string.
/// Returns a vector of up to 4 enchant type IDs (as u16).
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
