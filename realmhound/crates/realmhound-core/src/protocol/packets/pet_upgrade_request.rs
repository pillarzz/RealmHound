//! PetUpgradeRequestPacket implementation.
//!
//! Sent when feeding/fusing pets or upgrading the pet yard.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use super::SlotObjectData;
use std::io;

/// PetUpgradeRequestPacket (ID 16) - Outgoing
#[derive(Debug, Clone)]
pub struct PetUpgradeRequestPacket {
    /// The upgrade transaction type (raw `PetUpgradeType` ordinal).
    pub pet_trans_type: u8,
    /// The object ID of the first pet.
    pub p_id_one: i32,
    /// The object ID of the second pet.
    pub p_id_two: i32,
    /// The owner's object ID.
    pub object_id: i32,
    /// The currency used to purchase the upgrade (raw `PaymentType` ordinal).
    pub payment_type: u8,
    /// The items used to upgrade the pet.
    pub slot_objects: Vec<SlotObjectData>,
}

impl RotmgPacket for PetUpgradeRequestPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let pet_trans_type = reader.read_byte()?;
        let p_id_one = reader.read_i32()?;
        let p_id_two = reader.read_i32()?;
        let object_id = reader.read_i32()?;
        let payment_type = reader.read_byte()?;

        let count = reader.read_i16()?;
        if count < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Negative slot object count: {}", count),
            ));
        }
        let count = count as usize;
        // Each SlotObjectData is 12 bytes (3x i32); reject if larger than payload.
        if count.saturating_mul(12) > reader.remaining() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Slot object count {} exceeds remaining bytes", count),
            ));
        }
        let mut slot_objects = Vec::with_capacity(count);
        for _ in 0..count {
            slot_objects.push(SlotObjectData::deserialize(reader)?);
        }

        Ok(Self {
            pet_trans_type,
            p_id_one,
            p_id_two,
            object_id,
            payment_type,
            slot_objects,
        })
    }

    fn description(&self) -> String {
        format!(
            "PetUpgradeRequest: type={}, pIdOne={}, pIdTwo={}, items={}",
            self.pet_trans_type,
            self.p_id_one,
            self.p_id_two,
            self.slot_objects.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.push(1); // petTransType
        data.extend_from_slice(&10i32.to_be_bytes()); // pIdOne
        data.extend_from_slice(&20i32.to_be_bytes()); // pIdTwo
        data.extend_from_slice(&30i32.to_be_bytes()); // objectId
        data.push(2); // paymentType
        data.extend_from_slice(&1i16.to_be_bytes()); // slot count
        data.extend_from_slice(&5i32.to_be_bytes()); // slot.objectId
        data.extend_from_slice(&6i32.to_be_bytes()); // slot.slotId
        data.extend_from_slice(&7i32.to_be_bytes()); // slot.itemType

        let mut reader = PacketReader::new(&data);
        let packet = PetUpgradeRequestPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.pet_trans_type, 1);
        assert_eq!(packet.p_id_one, 10);
        assert_eq!(packet.p_id_two, 20);
        assert_eq!(packet.object_id, 30);
        assert_eq!(packet.payment_type, 2);
        assert_eq!(packet.slot_objects.len(), 1);
        assert_eq!(packet.slot_objects[0].object_id, 5);
        assert_eq!(packet.slot_objects[0].item_type, 7);
        assert!(reader.is_fully_parsed());
    }
}
