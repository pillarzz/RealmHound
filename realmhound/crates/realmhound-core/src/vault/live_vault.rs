//! Live vault data structures captured from VaultContentPacket.

use crate::api::character::parse_enchant_ids;
use crate::protocol::VaultContentPacket;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Serde module for Option<SystemTime> (serialized as Unix millis).
mod option_system_time {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S>(opt: &Option<SystemTime>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match opt {
            Some(time) => {
                let millis = time
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                Some(millis).serialize(serializer)
            }
            None => Option::<u64>::None.serialize(serializer),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<SystemTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<u64> = Option::deserialize(deserializer)?;
        Ok(opt.map(|millis| UNIX_EPOCH + Duration::from_millis(millis)))
    }
}

/// Type of vault (regular or seasonal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VaultType {
    Regular,
    Seasonal,
}

/// A single item in live vault storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveVaultItem {
    /// Item type ID (-1 = empty slot)
    pub item_id: i32,
    /// Enchant IDs on this item (up to 4)
    pub enchant_ids: Vec<u16>,
}

impl Default for LiveVaultItem {
    fn default() -> Self {
        Self {
            item_id: -1,
            enchant_ids: Vec::new(),
        }
    }
}

impl LiveVaultItem {
    /// Create a new item from an item ID.
    pub fn new(item_id: i32) -> Self {
        Self {
            item_id,
            enchant_ids: Vec::new(),
        }
    }

    /// Create a new item with enchants.
    pub fn with_enchants(item_id: i32, enchant_ids: Vec<u16>) -> Self {
        Self {
            item_id,
            enchant_ids,
        }
    }

    /// Check if this slot is empty.
    pub fn is_empty(&self) -> bool {
        self.item_id == -1
    }

    /// Get the number of enchants on this item.
    pub fn enchant_count(&self) -> usize {
        self.enchant_ids.len()
    }
}

/// Storage for a single vault type (regular or seasonal).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveVaultStorage {
    /// Vault items (grouped by chests of 8)
    pub vault_items: Vec<LiveVaultItem>,
    /// Material items (grouped by chests of 8)
    pub material_items: Vec<LiveVaultItem>,
    /// Gift items (flat array)
    pub gift_items: Vec<LiveVaultItem>,
    /// Potion items
    pub potion_items: Vec<LiveVaultItem>,
    /// Seasonal spoils items (only used in regular vault)
    pub spoils_items: Vec<LiveVaultItem>,
    /// Last time this vault was updated (serialized as Unix timestamp in millis)
    #[serde(with = "option_system_time")]
    pub last_updated: Option<SystemTime>,
    /// Whether we're waiting for more vault packets
    #[serde(skip)]
    pub pending_update: bool,
}

impl LiveVaultStorage {
    /// Create a new empty vault storage.
    pub fn new() -> Self {
        Self::default()
    }

    /// Update from a VaultContentPacket.
    pub fn update_from_packet(&mut self, packet: &VaultContentPacket, enchants: &ParsedEnchants) {
        // Extend vault items
        self.vault_items
            .extend(packet.vault_contents.iter().enumerate().map(|(i, &id)| {
                let mut item = LiveVaultItem::new(id);
                // Apply enchant data for this slot
                if let Some(enchant_ids) = enchants.vault_enchants.get(&i) {
                    item.enchant_ids = enchant_ids.clone();
                }
                item
            }));

        // Extend material items
        self.material_items.extend(
            packet
                .material_contents
                .iter()
                .map(|&id| LiveVaultItem::new(id)),
        );

        // Extend gift items (reversed to match in-game display order)
        self.gift_items.extend(
            packet
                .gift_contents
                .iter()
                .enumerate()
                .rev()
                .map(|(i, &id)| {
                    let mut item = LiveVaultItem::new(id);
                    // Enchant index is based on original packet order, not reversed order
                    if let Some(enchant_ids) = enchants.gift_enchants.get(&i) {
                        item.enchant_ids = enchant_ids.clone();
                    }
                    item
                }),
        );

        // Extend potion items
        self.potion_items.extend(
            packet
                .potion_contents
                .iter()
                .map(|&id| LiveVaultItem::new(id)),
        );

        // Extend spoils items
        self.spoils_items
            .extend(
                packet
                    .seasonal_spoil_contents
                    .iter()
                    .enumerate()
                    .map(|(i, &id)| {
                        let mut item = LiveVaultItem::new(id);
                        if let Some(enchant_ids) = enchants.spoils_enchants.get(&i) {
                            item.enchant_ids = enchant_ids.clone();
                        }
                        item
                    }),
            );

        self.pending_update = !packet.last_vault_packet;
        if packet.last_vault_packet {
            self.last_updated = Some(SystemTime::now());
        }
    }

    /// Clear all data and prepare for a new vault update.
    pub fn clear(&mut self) {
        self.vault_items.clear();
        self.material_items.clear();
        self.gift_items.clear();
        self.potion_items.clear();
        self.spoils_items.clear();
        self.pending_update = true;
    }

    /// Count non-empty vault slots.
    pub fn vault_item_count(&self) -> usize {
        self.vault_items.iter().filter(|i| !i.is_empty()).count()
    }

    /// Count vault chests (8 slots each).
    pub fn vault_chest_count(&self) -> usize {
        (self.vault_items.len() + 7) / 8
    }

    /// Count non-empty gift slots.
    pub fn gift_item_count(&self) -> usize {
        self.gift_items.iter().filter(|i| !i.is_empty()).count()
    }

    /// Count non-empty material slots.
    pub fn material_item_count(&self) -> usize {
        self.material_items.iter().filter(|i| !i.is_empty()).count()
    }

    /// Count material chests (8 slots each).
    pub fn material_chest_count(&self) -> usize {
        (self.material_items.len() + 7) / 8
    }

    /// Count non-empty potion slots.
    pub fn potion_item_count(&self) -> usize {
        self.potion_items.iter().filter(|i| !i.is_empty()).count()
    }

    /// Count non-empty spoils slots.
    pub fn spoils_item_count(&self) -> usize {
        self.spoils_items.iter().filter(|i| !i.is_empty()).count()
    }

    /// Check if we have any data.
    pub fn has_data(&self) -> bool {
        !self.vault_items.is_empty()
            || !self.material_items.is_empty()
            || !self.gift_items.is_empty()
            || !self.potion_items.is_empty()
            || !self.spoils_items.is_empty()
    }

    /// Get a vault item at index (cloned, for swap operations).
    pub fn get_vault_item(&self, index: usize) -> Option<LiveVaultItem> {
        self.vault_items.get(index).cloned()
    }

    /// Set a vault item at index (full item with enchants).
    pub fn set_vault_item(&mut self, index: usize, item: LiveVaultItem) -> bool {
        if index < self.vault_items.len() {
            self.vault_items[index] = item;
            self.last_updated = Some(std::time::SystemTime::now());
            true
        } else {
            false
        }
    }

    /// Get a material item at index.
    pub fn get_material_item(&self, index: usize) -> Option<LiveVaultItem> {
        self.material_items.get(index).cloned()
    }

    /// Set a material item at index.
    pub fn set_material_item(&mut self, index: usize, item: LiveVaultItem) -> bool {
        if index < self.material_items.len() {
            self.material_items[index] = item;
            self.last_updated = Some(std::time::SystemTime::now());
            true
        } else {
            false
        }
    }

    /// Get a gift item at index.
    pub fn get_gift_item(&self, index: usize) -> Option<LiveVaultItem> {
        self.gift_items.get(index).cloned()
    }

    /// Set a gift item at index.
    pub fn set_gift_item(&mut self, index: usize, item: LiveVaultItem) -> bool {
        if index < self.gift_items.len() {
            self.gift_items[index] = item;
            self.last_updated = Some(std::time::SystemTime::now());
            true
        } else {
            false
        }
    }

    /// Get a potion item at index.
    pub fn get_potion_item(&self, index: usize) -> Option<LiveVaultItem> {
        self.potion_items.get(index).cloned()
    }

    /// Set a potion item at index.
    pub fn set_potion_item(&mut self, index: usize, item: LiveVaultItem) -> bool {
        if index < self.potion_items.len() {
            self.potion_items[index] = item;
            self.last_updated = Some(std::time::SystemTime::now());
            true
        } else {
            false
        }
    }

    /// Update a vault slot by absolute index.
    /// Returns true if the slot was updated.
    pub fn update_vault_slot(&mut self, index: usize, item_id: i32) -> bool {
        if index < self.vault_items.len() {
            self.vault_items[index].item_id = item_id;
            self.vault_items[index].enchant_ids.clear(); // Enchants lost on move
            self.last_updated = Some(std::time::SystemTime::now());
            true
        } else {
            false
        }
    }

    /// Update a material slot by absolute index.
    /// Returns true if the slot was updated.
    pub fn update_material_slot(&mut self, index: usize, item_id: i32) -> bool {
        if index < self.material_items.len() {
            self.material_items[index].item_id = item_id;
            self.last_updated = Some(std::time::SystemTime::now());
            true
        } else {
            false
        }
    }

    /// Update a gift slot by absolute index.
    /// Returns true if the slot was updated.
    pub fn update_gift_slot(&mut self, index: usize, item_id: i32) -> bool {
        if index < self.gift_items.len() {
            self.gift_items[index].item_id = item_id;
            self.gift_items[index].enchant_ids.clear();
            self.last_updated = Some(std::time::SystemTime::now());
            true
        } else {
            false
        }
    }

    /// Update a potion slot by absolute index.
    /// Returns true if the slot was updated.
    pub fn update_potion_slot(&mut self, index: usize, item_id: i32) -> bool {
        if index < self.potion_items.len() {
            self.potion_items[index].item_id = item_id;
            self.last_updated = Some(std::time::SystemTime::now());
            true
        } else {
            false
        }
    }

    /// Update a spoils slot by absolute index.
    /// Returns true if the slot was updated.
    pub fn update_spoils_slot(&mut self, index: usize, item_id: i32) -> bool {
        if index < self.spoils_items.len() {
            self.spoils_items[index].item_id = item_id;
            self.spoils_items[index].enchant_ids.clear();
            self.last_updated = Some(std::time::SystemTime::now());
            true
        } else {
            false
        }
    }

    /// Find which vault page (0-indexed) matches the given inventory contents.
    /// Each page is 8 slots. Returns None if no match found.
    pub fn find_matching_vault_page(&self, inventory: &[i32; 8]) -> Option<usize> {
        let num_pages = (self.vault_items.len() + 7) / 8;
        for page in 0..num_pages {
            let start = page * 8;
            let mut matches = true;
            for (slot, &item_id) in inventory.iter().enumerate() {
                let idx = start + slot;
                let stored_id = self.vault_items.get(idx).map(|i| i.item_id).unwrap_or(-1);
                if stored_id != item_id {
                    matches = false;
                    break;
                }
            }
            if matches {
                return Some(page);
            }
        }
        None
    }

    /// Find which material page matches the given inventory contents.
    pub fn find_matching_material_page(&self, inventory: &[i32; 8]) -> Option<usize> {
        let num_pages = (self.material_items.len() + 7) / 8;
        for page in 0..num_pages {
            let start = page * 8;
            let mut matches = true;
            for (slot, &item_id) in inventory.iter().enumerate() {
                let idx = start + slot;
                let stored_id = self
                    .material_items
                    .get(idx)
                    .map(|i| i.item_id)
                    .unwrap_or(-1);
                if stored_id != item_id {
                    matches = false;
                    break;
                }
            }
            if matches {
                return Some(page);
            }
        }
        None
    }

    /// Find which potion page matches (potions can exceed 8 slots).
    pub fn find_matching_potion_page(&self, inventory: &[i32; 8]) -> Option<usize> {
        let num_pages = (self.potion_items.len() + 7) / 8;
        for page in 0..num_pages {
            let start = page * 8;
            let mut matches = true;
            for (slot, &item_id) in inventory.iter().enumerate() {
                let idx = start + slot;
                let stored_id = self.potion_items.get(idx).map(|i| i.item_id).unwrap_or(-1);
                if stored_id != item_id {
                    matches = false;
                    break;
                }
            }
            if matches {
                return Some(page);
            }
        }
        None
    }

    /// Find which gift page matches.
    pub fn find_matching_gift_page(&self, inventory: &[i32; 8]) -> Option<usize> {
        let num_pages = (self.gift_items.len() + 7) / 8;
        for page in 0..num_pages {
            let start = page * 8;
            let mut matches = true;
            for (slot, &item_id) in inventory.iter().enumerate() {
                let idx = start + slot;
                let stored_id = self.gift_items.get(idx).map(|i| i.item_id).unwrap_or(-1);
                if stored_id != item_id {
                    matches = false;
                    break;
                }
            }
            if matches {
                return Some(page);
            }
        }
        None
    }
}

/// Parsed enchant data from the packet's enchant strings.
#[derive(Debug, Default)]
pub struct ParsedEnchants {
    /// Vault item enchants by slot index
    pub vault_enchants: std::collections::HashMap<usize, Vec<u16>>,
    /// Gift item enchants by slot index
    pub gift_enchants: std::collections::HashMap<usize, Vec<u16>>,
    /// Spoils item enchants by slot index
    pub spoils_enchants: std::collections::HashMap<usize, Vec<u16>>,
}

impl ParsedEnchants {
    /// Parse enchants from VaultContentPacket.
    pub fn from_packet(packet: &VaultContentPacket) -> Self {
        let mut result = Self::default();

        // Parse vault enchants
        if !packet.vault_chest_enchants.is_empty() {
            result.vault_enchants = Self::parse_enchant_string(
                &packet.vault_chest_enchants,
                packet.vault_contents.len(),
            );
        }

        // Parse gift enchants
        if !packet.gift_chest_enchants.is_empty() {
            result.gift_enchants =
                Self::parse_enchant_string(&packet.gift_chest_enchants, packet.gift_contents.len());
        }

        // Parse spoils enchants
        if !packet.spoils_chest_enchants.is_empty() {
            result.spoils_enchants = Self::parse_enchant_string(
                &packet.spoils_chest_enchants,
                packet.seasonal_spoil_contents.len(),
            );
        }

        result
    }

    /// Parse a base64 enchant string into a map of slot -> enchant IDs.
    /// Format: comma-separated base64 strings, one per slot with enchants.
    fn parse_enchant_string(
        s: &str,
        _slot_count: usize,
    ) -> std::collections::HashMap<usize, Vec<u16>> {
        let mut map = std::collections::HashMap::new();

        // The enchant string format: each item's enchants are separated
        // For items with enchants, we get their base64 data
        for (idx, part) in s.split(',').enumerate() {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                let ids = parse_enchant_ids(trimmed);
                if !ids.is_empty() {
                    map.insert(idx, ids);
                }
            }
        }

        map
    }
}

/// Live vault data containing both regular and seasonal vaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveVaultData {
    /// Regular vault storage
    pub regular: LiveVaultStorage,
    /// Seasonal vault storage
    pub seasonal: LiveVaultStorage,
}

impl LiveVaultData {
    /// Create new empty live vault data.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the storage for a specific vault type.
    pub fn get(&self, vault_type: VaultType) -> &LiveVaultStorage {
        match vault_type {
            VaultType::Regular => &self.regular,
            VaultType::Seasonal => &self.seasonal,
        }
    }

    /// Get mutable storage for a specific vault type.
    pub fn get_mut(&mut self, vault_type: VaultType) -> &mut LiveVaultStorage {
        match vault_type {
            VaultType::Regular => &mut self.regular,
            VaultType::Seasonal => &mut self.seasonal,
        }
    }

    /// Update from a VaultContentPacket.
    /// The vault type determines which storage to update.
    pub fn update_from_packet(&mut self, packet: &VaultContentPacket, vault_type: VaultType) {
        let storage = self.get_mut(vault_type);

        // If this is a new vault session (not a continuation), clear existing data
        if !storage.pending_update {
            storage.clear();
        }

        // Parse enchants from the packet
        let enchants = ParsedEnchants::from_packet(packet);

        // Update the storage
        storage.update_from_packet(packet, &enchants);

        tracing::debug!(
            "[VAULT] Updated {} vault: {} vault items, {} materials, {} gifts, {} potions, {} spoils (complete={})",
            match vault_type {
                VaultType::Regular => "regular",
                VaultType::Seasonal => "seasonal",
            },
            storage.vault_item_count(),
            storage.material_item_count(),
            storage.gift_item_count(),
            storage.potion_item_count(),
            storage.spoils_item_count(),
            packet.last_vault_packet
        );
    }

    /// Check if we have any live vault data.
    pub fn has_any_data(&self) -> bool {
        self.regular.has_data() || self.seasonal.has_data()
    }

    /// Save live vault data to a file.
    pub fn save_to_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load live vault data from a file.
    pub fn load_from_file(path: &std::path::Path) -> std::io::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let data: Self = serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_live_vault_item_empty() {
        let item = LiveVaultItem::default();
        assert!(item.is_empty());
        assert_eq!(item.enchant_count(), 0);
    }

    #[test]
    fn test_live_vault_item_with_id() {
        let item = LiveVaultItem::new(1234);
        assert!(!item.is_empty());
        assert_eq!(item.item_id, 1234);
    }

    #[test]
    fn test_live_vault_storage_counts() {
        let mut storage = LiveVaultStorage::new();
        storage.vault_items = vec![
            LiveVaultItem::new(100),
            LiveVaultItem::default(), // empty
            LiveVaultItem::new(200),
        ];

        assert_eq!(storage.vault_item_count(), 2);
        assert_eq!(storage.vault_chest_count(), 1); // 3 items = 1 chest
    }

    #[test]
    fn test_vault_type() {
        let mut data = LiveVaultData::new();
        data.regular.vault_items.push(LiveVaultItem::new(1));
        data.seasonal.vault_items.push(LiveVaultItem::new(2));

        assert_eq!(data.get(VaultType::Regular).vault_items[0].item_id, 1);
        assert_eq!(data.get(VaultType::Seasonal).vault_items[0].item_id, 2);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut data = LiveVaultData::new();
        data.regular.vault_items.push(LiveVaultItem::new(100));
        data.regular.vault_items.push(LiveVaultItem {
            item_id: 200,
            enchant_ids: vec![1, 2, 3],
        });
        data.regular.last_updated = Some(SystemTime::now());
        data.seasonal.gift_items.push(LiveVaultItem::new(300));

        // Serialize to JSON
        let json = serde_json::to_string(&data).unwrap();

        // Deserialize back
        let loaded: LiveVaultData = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.regular.vault_items.len(), 2);
        assert_eq!(loaded.regular.vault_items[0].item_id, 100);
        assert_eq!(loaded.regular.vault_items[1].enchant_ids, vec![1, 2, 3]);
        assert!(loaded.regular.last_updated.is_some());
        assert_eq!(loaded.seasonal.gift_items[0].item_id, 300);
    }
}
