//! ReskinPetPacket implementation.
//!
//! Sent to change the form of the pet currently following the player.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use super::SlotObjectData;
use std::io;

/// PetChangeFormPacket (ID 53) - Outgoing
#[derive(Debug, Clone)]
pub struct PetChangeFormPacket {
    /// The instance id of the pet to update.
    pub instance_id: i32,
    /// The pet type the pet will become after the form change.
    pub new_pet_type: i32,
    /// The slot object of a pet stone, if one is used.
    pub item: SlotObjectData,
}

impl RotmgPacket for PetChangeFormPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let instance_id = reader.read_i32()?;
        let new_pet_type = reader.read_i32()?;
        let item = SlotObjectData::deserialize(reader)?;
        Ok(Self {
            instance_id,
            new_pet_type,
            item,
        })
    }

    fn description(&self) -> String {
        format!(
            "PetChangeForm: instanceId={}, newPetType={}",
            self.instance_id, self.new_pet_type
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&42i32.to_be_bytes()); // instanceId
        data.extend_from_slice(&7i32.to_be_bytes()); // newPetType
        data.extend_from_slice(&1i32.to_be_bytes()); // item.objectId
        data.extend_from_slice(&2i32.to_be_bytes()); // item.slotId
        data.extend_from_slice(&3i32.to_be_bytes()); // item.itemType

        let mut reader = PacketReader::new(&data);
        let packet = PetChangeFormPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.instance_id, 42);
        assert_eq!(packet.new_pet_type, 7);
        assert_eq!(packet.item.object_id, 1);
        assert_eq!(packet.item.item_type, 3);
        assert!(reader.is_fully_parsed());
    }
}
