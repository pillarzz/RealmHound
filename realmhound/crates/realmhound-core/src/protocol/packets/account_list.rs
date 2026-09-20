//! AccountList packet implementation.
//!
//! Received to provide lists of account ids (locked, ignored, etc.).

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Maximum number of account ids accepted. The i16 length prefix is
/// bound-checked against this before allocation.
const MAX_ACCOUNT_IDS: i16 = 8192;

/// AccountList packet (ID 99) - Incoming
#[derive(Debug, Clone)]
pub struct AccountListPacket {
    /// The id of the account id list.
    pub account_list_id: i32,
    /// The account ids included in the list.
    pub account_ids: Vec<String>,
    /// Unknown lock action.
    pub lock_action: i32,
}

impl RotmgPacket for AccountListPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let account_list_id = reader.read_i32()?;

        let len = reader.read_i16()?;
        if len < 0 || len > MAX_ACCOUNT_IDS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid account id count: {}", len),
            ));
        }
        let len = len as usize;
        let mut account_ids = Vec::with_capacity(len);
        for _ in 0..len {
            account_ids.push(reader.read_string()?);
        }

        let lock_action = reader.read_i32()?;

        Ok(Self {
            account_list_id,
            account_ids,
            lock_action,
        })
    }

    fn description(&self) -> String {
        format!(
            "AccountList: listId={} ids={} lockAction={}",
            self.account_list_id,
            self.account_ids.len(),
            self.lock_action
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&7i32.to_be_bytes()); // accountListId
        data.extend_from_slice(&2i16.to_be_bytes()); // count
        for id in ["111", "222"] {
            data.extend_from_slice(&(id.len() as u16).to_be_bytes());
            data.extend_from_slice(id.as_bytes());
        }
        data.extend_from_slice(&1i32.to_be_bytes()); // lockAction

        let mut reader = PacketReader::new(&data);
        let packet = AccountListPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.account_list_id, 7);
        assert_eq!(
            packet.account_ids,
            vec!["111".to_string(), "222".to_string()]
        );
        assert_eq!(packet.lock_action, 1);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_negative_length_rejected() {
        let mut data = Vec::new();
        data.extend_from_slice(&0i32.to_be_bytes());
        data.extend_from_slice(&(-1i16).to_be_bytes());
        let mut reader = PacketReader::new(&data);
        assert!(AccountListPacket::deserialize(&mut reader).is_err());
    }
}
