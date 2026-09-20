//! VaultContentPacket implementation.
//!
//! Received when the player enters or updates their vault area.
//! Contains all vault, material, gift, potion, and seasonal spoils data.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// VaultContentPacket (ID 117) - Incoming
///
/// Sent by server when player enters vault area. Contains complete vault data
/// including regular vault, materials, gifts, potions, and seasonal spoils.
///
/// **Important**: The vault instance depends on whether the player is a
/// regular or seasonal character. Regular chars see regular vault + spoils,
/// seasonal chars see seasonal vault (no spoils).
#[derive(Debug, Clone)]
pub struct VaultContentPacket {
    /// If this is the last vault packet (vault can be split across multiple packets)
    pub last_vault_packet: bool,
    /// Vault chest object ID
    pub vault_chest_object_id: i32,
    /// Material chest object ID
    pub material_chest_object_id: i32,
    /// Gift chest object ID
    pub gift_chest_object_id: i32,
    /// Potion storage object ID
    pub potion_storage_object_id: i32,
    /// Seasonal spoils chest object ID
    pub seasonal_spoil_chest_object_id: i32,
    /// The contents of the player's vault (-1 = empty slot)
    pub vault_contents: Vec<i32>,
    /// The material contents
    pub material_contents: Vec<i32>,
    /// The contents of the player's gift vault
    pub gift_contents: Vec<i32>,
    /// The contents of the player's potion vault
    pub potion_contents: Vec<i32>,
    /// The contents of the player's seasonal spoils
    pub seasonal_spoil_contents: Vec<i32>,
    /// Cost in gold for the next vault upgrade
    pub vault_upgrade_cost: i16,
    /// Cost in gold for the next material upgrade
    pub material_upgrade_cost: i16,
    /// Cost in gold for the next potion vault upgrade
    pub potion_upgrade_cost: i16,
    /// Current slot size of the player's potion vault
    pub current_potion_max: i16,
    /// Size of the player's potion vault after purchasing the current upgrade
    pub next_potion_max: i16,
    /// Unknown short field (not documented in the reference implementation)
    pub unknown_short: i16,
    /// Base64 string indicating enchantments in player's vault
    pub vault_chest_enchants: String,
    /// Base64 string indicating enchantments in player's gift vault
    pub gift_chest_enchants: String,
    /// Base64 string indicating enchantments in player's spoils chest
    pub spoils_chest_enchants: String,
}

impl RotmgPacket for VaultContentPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let last_vault_packet = reader.read_bool()?;
        let vault_chest_object_id = reader.read_compressed_int()?;
        let material_chest_object_id = reader.read_compressed_int()?;
        let gift_chest_object_id = reader.read_compressed_int()?;
        let potion_storage_object_id = reader.read_compressed_int()?;
        let seasonal_spoil_chest_object_id = reader.read_compressed_int()?;

        // Read vault contents array
        let vault_len = reader.read_array_length()?;
        let mut vault_contents = Vec::with_capacity(vault_len);
        for _ in 0..vault_len {
            vault_contents.push(reader.read_compressed_int()?);
        }

        // Read material contents array
        let material_len = reader.read_array_length()?;
        let mut material_contents = Vec::with_capacity(material_len);
        for _ in 0..material_len {
            material_contents.push(reader.read_compressed_int()?);
        }

        // Read gift contents array
        let gift_len = reader.read_array_length()?;
        let mut gift_contents = Vec::with_capacity(gift_len);
        for _ in 0..gift_len {
            gift_contents.push(reader.read_compressed_int()?);
        }

        // Read potion contents array
        let potion_len = reader.read_array_length()?;
        let mut potion_contents = Vec::with_capacity(potion_len);
        for _ in 0..potion_len {
            potion_contents.push(reader.read_compressed_int()?);
        }

        // Read seasonal spoil contents array
        let spoil_len = reader.read_array_length()?;
        let mut seasonal_spoil_contents = Vec::with_capacity(spoil_len);
        for _ in 0..spoil_len {
            seasonal_spoil_contents.push(reader.read_compressed_int()?);
        }

        // Read upgrade costs and potion limits
        let vault_upgrade_cost = reader.read_i16()?;
        let material_upgrade_cost = reader.read_i16()?;
        let potion_upgrade_cost = reader.read_i16()?;
        let current_potion_max = reader.read_i16()?;
        let next_potion_max = reader.read_i16()?;

        // Additional short field not documented in the original reference implementation
        let unknown_short = reader.read_i16()?;

        // Read enchantment data (Base64 encoded)
        let vault_chest_enchants = reader.read_string()?;
        let gift_chest_enchants = reader.read_string()?;
        let spoils_chest_enchants = reader.read_string()?;

        Ok(Self {
            last_vault_packet,
            vault_chest_object_id,
            material_chest_object_id,
            gift_chest_object_id,
            potion_storage_object_id,
            seasonal_spoil_chest_object_id,
            vault_contents,
            material_contents,
            gift_contents,
            potion_contents,
            seasonal_spoil_contents,
            vault_upgrade_cost,
            material_upgrade_cost,
            potion_upgrade_cost,
            current_potion_max,
            next_potion_max,
            unknown_short,
            vault_chest_enchants,
            gift_chest_enchants,
            spoils_chest_enchants,
        })
    }

    fn description(&self) -> String {
        format!(
            "VaultContent: vault={} items, materials={}, gifts={}, potions={}, spoils={}{}",
            self.vault_contents.len(),
            self.material_contents.len(),
            self.gift_contents.len(),
            self.potion_contents.len(),
            self.seasonal_spoil_contents.len(),
            if self.last_vault_packet {
                " (last)"
            } else {
                ""
            }
        )
    }
}

impl VaultContentPacket {
    /// Count non-empty vault slots
    pub fn vault_item_count(&self) -> usize {
        self.vault_contents.iter().filter(|&&id| id != -1).count()
    }

    /// Count non-empty gift slots
    pub fn gift_item_count(&self) -> usize {
        self.gift_contents.iter().filter(|&&id| id != -1).count()
    }

    /// Count non-empty material slots
    pub fn material_item_count(&self) -> usize {
        self.material_contents
            .iter()
            .filter(|&&id| id != -1)
            .count()
    }

    /// Count non-empty potion slots
    pub fn potion_item_count(&self) -> usize {
        self.potion_contents.iter().filter(|&&id| id != -1).count()
    }

    /// Count non-empty seasonal spoil slots
    pub fn spoil_item_count(&self) -> usize {
        self.seasonal_spoil_contents
            .iter()
            .filter(|&&id| id != -1)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vault_content_empty() {
        let mut data = Vec::new();

        // last_vault_packet: bool
        data.push(1); // true

        // 5x object IDs (compressed int - single byte for small values)
        data.push(1); // vault_chest_object_id
        data.push(2); // material_chest_object_id
        data.push(3); // gift_chest_object_id
        data.push(4); // potion_storage_object_id
        data.push(5); // seasonal_spoil_chest_object_id

        // 5x empty arrays (length 0)
        data.push(0); // vault_contents length
        data.push(0); // material_contents length
        data.push(0); // gift_contents length
        data.push(0); // potion_contents length
        data.push(0); // seasonal_spoil_contents length

        // 6x shorts (upgrade costs, potion limits, and unknown)
        data.extend_from_slice(&100i16.to_be_bytes()); // vault_upgrade_cost
        data.extend_from_slice(&50i16.to_be_bytes()); // material_upgrade_cost
        data.extend_from_slice(&75i16.to_be_bytes()); // potion_upgrade_cost
        data.extend_from_slice(&16i16.to_be_bytes()); // current_potion_max
        data.extend_from_slice(&24i16.to_be_bytes()); // next_potion_max
        data.extend_from_slice(&0i16.to_be_bytes()); // unknown_short

        // 3x empty strings
        data.extend_from_slice(&0u16.to_be_bytes()); // vault_chest_enchants
        data.extend_from_slice(&0u16.to_be_bytes()); // gift_chest_enchants
        data.extend_from_slice(&0u16.to_be_bytes()); // spoils_chest_enchants

        let mut reader = PacketReader::new(&data);
        let packet = VaultContentPacket::deserialize(&mut reader).unwrap();

        assert!(packet.last_vault_packet);
        assert_eq!(packet.vault_chest_object_id, 1);
        assert_eq!(packet.vault_contents.len(), 0);
        assert_eq!(packet.vault_upgrade_cost, 100);
        assert_eq!(packet.current_potion_max, 16);
    }
}
