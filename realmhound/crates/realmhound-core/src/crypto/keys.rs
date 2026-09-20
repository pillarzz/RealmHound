//! RotMG encryption keys.
//!
//! These are the hardcoded RC4 keys used by the RotMG client.

/// RC4 key for incoming packets (server → client).
pub const INCOMING_KEY: &[u8] = &[
    0xc9, 0x1d, 0x9e, 0xec, 0x42, 0x01, 0x60, 0x73, 0x0d, 0x82, 0x56, 0x04, 0xe0,
];

/// RC4 key for outgoing packets (client → server).
pub const OUTGOING_KEY: &[u8] = &[
    0x5a, 0x4d, 0x20, 0x16, 0xbc, 0x16, 0xdc, 0x64, 0x88, 0x31, 0x94, 0xff, 0xd9,
];

/// RotMG RC4 encryption keys.
pub struct RotmgKeys;

impl RotmgKeys {
    /// RC4 key for incoming packets (server → client).
    pub const INCOMING: &'static [u8] = INCOMING_KEY;

    /// RC4 key for outgoing packets (client → server).
    pub const OUTGOING: &'static [u8] = OUTGOING_KEY;

    /// Get the hex string representation of the incoming key.
    pub fn incoming_hex() -> String {
        hex::encode(Self::INCOMING)
    }

    /// Get the hex string representation of the outgoing key.
    pub fn outgoing_hex() -> String {
        hex::encode(Self::OUTGOING)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_hex_strings() {
        // Verify keys match the reference implementation
        assert_eq!(RotmgKeys::incoming_hex(), "c91d9eec420160730d825604e0");
        assert_eq!(RotmgKeys::outgoing_hex(), "5a4d2016bc16dc64883194ffd9");
    }

    #[test]
    fn test_key_lengths() {
        assert_eq!(RotmgKeys::INCOMING.len(), 13);
        assert_eq!(RotmgKeys::OUTGOING.len(), 13);
    }
}
