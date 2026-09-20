//! EditAccountListPacket implementation.
//!
//! Sent to edit an account id list.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// EditAccountListPacket (ID 27) - Outgoing
#[derive(Debug, Clone)]
pub struct EditAccountListPacket {
    /// The id of the account id list being edited.
    pub account_list_id: i32,
    /// Whether the edit adds to the list (true) or removes from it (false).
    pub add: bool,
    /// The object id of the player to add/remove.
    pub object_id: i32,
}

impl RotmgPacket for EditAccountListPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let account_list_id = reader.read_i32()?;
        let add = reader.read_bool()?;
        let object_id = reader.read_i32()?;

        Ok(Self {
            account_list_id,
            add,
            object_id,
        })
    }

    fn description(&self) -> String {
        format!(
            "EditAccountList: listId={}, add={}, objectId={}",
            self.account_list_id, self.add, self.object_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 0, 0, 1, 1, 0, 0, 0, 9];
        let mut reader = PacketReader::new(&data);
        let packet = EditAccountListPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.account_list_id, 1);
        assert!(packet.add);
        assert_eq!(packet.object_id, 9);
        assert!(reader.is_fully_parsed());
    }
}
