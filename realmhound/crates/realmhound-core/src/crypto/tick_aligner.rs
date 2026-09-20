//! RC4 cipher alignment using tick packets.
//!
//! The RC4 cipher can be aligned mid-stream using tick packets. Both NEWTICK (server→client)
//! and MOVE (client→server) packets contain a tick counter that increments by 1 between
//! consecutive packets. By capturing two consecutive tick packets and brute-force searching
//! for the cipher offset where the decrypted values differ by 1, we can synchronize the cipher.

use super::RC4Cipher;
use crate::protocol::PacketType;

/// Maximum number of positions to search during cipher sync.
/// Matches the reference SEARCH_SIZE = 10,000,000.
/// With O(n) algorithm, this completes in milliseconds.
const MAX_SYNC_SEARCH: usize = 10_000_000;

/// Packet type IDs used for tick alignment.
const NEWTICK_ID: u8 = PacketType::NewTick as u8;
const MOVE_ID: u8 = PacketType::Move as u8;

/// Tick aligner for synchronizing RC4 cipher mid-stream.
///
/// When joining a game session mid-stream, the cipher state is unknown.
/// This aligner collects consecutive tick packets and uses brute-force
/// search to find the correct cipher offset.
#[derive(Debug)]
pub struct TickAligner {
    /// Whether the cipher is synchronized.
    synced: bool,
    /// Current tick count (for validation).
    current_tick: i32,
    /// Accumulated encrypted bytes between tick packets.
    packet_bytes: usize,
    /// First tick packet data (encrypted, 4 bytes).
    tick_a: Option<[u8; 4]>,
    /// The cipher being aligned.
    cipher: RC4Cipher,
    /// Fork of cipher for non-destructive testing.
    cipher_fork: RC4Cipher,
    /// The key used for this cipher (for reset during sync).
    key: Vec<u8>,
}

impl TickAligner {
    /// Create a new tick aligner with the given key.
    pub fn new(key: &[u8]) -> Self {
        let cipher = RC4Cipher::new(key);
        let cipher_fork = cipher.clone();
        Self {
            synced: false,
            current_tick: -1,
            packet_bytes: 0,
            tick_a: None,
            cipher,
            cipher_fork,
            key: key.to_vec(),
        }
    }

    /// Create a new tick aligner that is already synced (for fresh connections).
    ///
    /// Use this when you catch a connection from the SYN packet, meaning
    /// the cipher starts at position 0.
    pub fn new_synced(key: &[u8]) -> Self {
        let cipher = RC4Cipher::new(key);
        let cipher_fork = cipher.clone();
        Self {
            synced: true,
            current_tick: -1,
            packet_bytes: 0,
            tick_a: None,
            cipher,
            cipher_fork,
            key: key.to_vec(),
        }
    }

    /// Mark the aligner as synced (for fresh connections caught from SYN).
    pub fn mark_synced(&mut self) {
        self.synced = true;
        self.current_tick = -1;
        self.packet_bytes = 0;
        self.tick_a = None;
    }

    /// Check if the cipher is synchronized.
    pub fn is_synced(&self) -> bool {
        self.synced
    }

    /// Get the current tick count.
    pub fn current_tick(&self) -> i32 {
        self.current_tick
    }

    /// Get a reference to the synchronized cipher.
    ///
    /// Returns None if not yet synced.
    pub fn cipher(&self) -> Option<&RC4Cipher> {
        if self.synced {
            Some(&self.cipher)
        } else {
            None
        }
    }

    /// Get a mutable reference to the synchronized cipher.
    ///
    /// Returns None if not yet synced.
    pub fn cipher_mut(&mut self) -> Option<&mut RC4Cipher> {
        if self.synced {
            Some(&mut self.cipher)
        } else {
            None
        }
    }

    /// Process a packet for tick alignment. Returns `true` once the cipher is
    /// synced and the packet can be decrypted, `false` while still searching.
    pub fn check_alignment(
        &mut self,
        encrypted_data: &[u8],
        packet_size: usize,
        packet_type: u8,
    ) -> bool {
        if self.synced {
            // Validate the running tick counter (non-destructive). On drift the
            // aligner resets itself; bail out without tracking bytes.
            if !self.validate_synced_tick(encrypted_data, packet_type) {
                return false;
            }

            // Track bytes for potential re-sync
            if packet_size > 5 {
                self.packet_bytes += packet_size - 5;
            }
            return true;
        }

        // Not synced - try to align using tick packets
        if packet_type == NEWTICK_ID || packet_type == MOVE_ID {
            if encrypted_data.len() >= 9 {
                let mut tick_bytes = [0u8; 4];
                tick_bytes.copy_from_slice(&encrypted_data[5..9]);

                if let Some(tick_a) = self.tick_a.take() {
                    // We have two tick packets - attempt sync
                    tracing::debug!(
                        "Attempting sync with {} bytes between ticks",
                        self.packet_bytes
                    );

                    if let Some((offset, tick_value)) = sync_cipher(
                        &mut self.cipher,
                        &self.key,
                        &tick_a,
                        &tick_bytes,
                        self.packet_bytes,
                    ) {
                        self.synced = true;
                        // After sync_cipher finds the offset, reset cipher to that position
                        // Then skip past all the bytes we've seen (packet_bytes) plus the tick_b bytes
                        self.cipher.reset(&self.key);
                        self.cipher.skip(offset);
                        self.cipher.skip(self.packet_bytes); // Skip to tick_b position
                        self.cipher.skip(4); // Skip tick_b's tick bytes
                        if packet_size > 9 {
                            self.cipher.skip(packet_size - 9); // Skip rest of tick_b's payload
                        }
                        self.current_tick = tick_value;
                        self.packet_bytes = 0;

                        tracing::info!(
                            "Cipher synced mid-stream at offset {} (tick {})",
                            offset,
                            tick_value
                        );
                        return true;
                    } else {
                        tracing::debug!("Tick sync failed. Retrying with next tick pair.");
                        self.packet_bytes = packet_size - 5;
                    }
                } else {
                    // First tick packet - save it
                    self.tick_a = Some(tick_bytes);
                    self.packet_bytes = packet_size - 5;
                    tracing::debug!("Captured first tick packet for sync");
                }
            }
        } else {
            // Non-tick packet - accumulate bytes
            if packet_size > 5 {
                self.packet_bytes += packet_size - 5;
            }
        }

        false
    }

    /// Validate a synced tick packet's counter WITHOUT advancing the real cipher.
    ///
    /// Call this on every packet while synced, BEFORE the payload is decrypted.
    /// `encrypted_data` must be the full framed packet (plaintext 5-byte header +
    /// still-encrypted payload). For NewTick/Move packets the 4-byte tick counter
    /// lives at `encrypted_data[5..9]`; it is decrypted on a *fork* of the cipher
    /// so the real keystream is untouched (the caller's decrypt advances it).
    ///
    /// Behaviour:
    /// * non-tick packets (and `!synced`) always pass.
    /// * the first tick seen since (re)sync anchors `current_tick` (baseline only).
    /// * subsequent ticks must equal `current_tick + 1`.
    /// * a malformed too-short tick packet, or a counter mismatch, `reset()`s the
    ///   aligner and returns `false` so the caller drops the packet and resyncs.
    pub fn validate_synced_tick(&mut self, encrypted_data: &[u8], packet_type: u8) -> bool {
        if !self.synced {
            return true;
        }
        if packet_type != NEWTICK_ID && packet_type != MOVE_ID {
            return true;
        }
        if encrypted_data.len() < 9 {
            // A complete tick packet always carries at least a 4-byte counter;
            // anything shorter is misframed/corrupt, so force a resync.
            tracing::warn!(
                "Tick packet too short ({} bytes) to validate; re-syncing",
                encrypted_data.len()
            );
            self.reset();
            return false;
        }

        let mut tick_bytes = [0u8; 4];
        tick_bytes.copy_from_slice(&encrypted_data[5..9]);

        // Decrypt the tick counter on a fork so the real cipher is not consumed.
        self.cipher_fork = self.cipher.clone();
        self.cipher_fork.apply(&mut tick_bytes);
        let tick = i32::from_be_bytes(tick_bytes);

        if self.current_tick < 0 {
            // First tick since (re)sync: establish the baseline, do not validate.
            self.current_tick = tick;
            return true;
        }

        let expected = self.current_tick.wrapping_add(1);
        if expected != tick {
            tracing::warn!(
                "Tick sync lost! Expected {} got {}. Re-syncing...",
                expected,
                tick
            );
            self.reset();
            return false;
        }
        self.current_tick = expected;
        true
    }

    /// Reset the aligner state (called when sync is lost or session changes).
    /// This also resets the cipher to position 0, matching the reference behavior.
    pub fn reset(&mut self) {
        self.synced = false;
        self.current_tick = -1;
        self.packet_bytes = 0;
        self.tick_a = None;
        // Reset cipher to position 0 (matches the reference rc4.reset() on sync failure)
        self.cipher = RC4Cipher::new(&self.key);
        self.cipher_fork = self.cipher.clone();
    }

    /// Reset the cipher with a new key.
    pub fn reset_cipher(&mut self, new_key: &[u8]) {
        self.key = new_key.to_vec();
        self.cipher = RC4Cipher::new(new_key);
        self.cipher_fork = self.cipher.clone();
        self.reset();
    }
}

/// Brute-force search for cipher sync position: an offset where decrypting
/// `tick_a` yields value N and `tick_b` yields N+1. `delta` is the byte offset
/// between the two tick packets. Returns `Some((offset, tick_value))` on
/// success, or `None` if no sync was found.
fn sync_cipher(
    _cipher: &mut RC4Cipher,
    key: &[u8],
    tick_a: &[u8; 4],
    tick_b: &[u8; 4],
    delta: usize,
) -> Option<(usize, i32)> {
    // O(n) algorithm:
    // Keep two ciphers running in parallel:
    // - finder_a: at current offset (for decrypting tick_a)
    // - finder_b: at current offset + delta (for decrypting tick_b)
    // Advance both by 1 byte each iteration until we find a match.

    let mut finder_a = RC4Cipher::new(key);
    let mut finder_b = RC4Cipher::new(key);
    finder_b.skip(delta); // finder_b is always `delta` bytes ahead

    for offset in 0..MAX_SYNC_SEARCH {
        // Try decrypting tick_a at current offset
        let value_a = decrypt_tick_value(tick_a, &mut finder_a.clone());
        // Try decrypting tick_b at current offset + delta
        let value_b = decrypt_tick_value(tick_b, &mut finder_b.clone());

        // Check if consecutive tick values (a + 1 == b)
        if value_a >= 0 && value_b >= 0 && value_a.wrapping_add(1) == value_b {
            tracing::debug!(
                "Found sync at offset {} (tick {} -> {})",
                offset,
                value_a,
                value_b
            );
            return Some((offset, value_b));
        }

        // Advance both ciphers by 1 byte
        finder_a.get_byte();
        finder_b.get_byte();
    }

    None
}

/// Decrypt a 4-byte tick value using a cipher (non-destructive via clone).
#[inline]
fn decrypt_tick_value(encrypted: &[u8; 4], cipher: &mut RC4Cipher) -> i32 {
    let mut decrypted = *encrypted;
    cipher.apply(&mut decrypted);
    i32::from_be_bytes(decrypted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::INCOMING_KEY;

    #[test]
    fn test_tick_aligner_creation() {
        let aligner = TickAligner::new(INCOMING_KEY);
        assert!(!aligner.is_synced());
        assert_eq!(aligner.current_tick(), -1);
    }

    #[test]
    fn test_sync_with_known_ticks() {
        // Simulate two consecutive tick packets with known offsets
        let mut cipher = RC4Cipher::new(INCOMING_KEY);

        // Create tick values
        let tick_a_value: i32 = 100;
        let tick_b_value: i32 = 101;

        // tick_a is at cipher position `offset`
        // tick_b is at cipher position `offset + delta`
        let offset = 1000;
        let delta = 500; // bytes between start of tick_a and start of tick_b

        // Encrypt tick A at position `offset`
        cipher.skip(offset);
        let mut tick_a_encrypted = tick_a_value.to_be_bytes();
        cipher.apply(&mut tick_a_encrypted);

        // Reset and encrypt tick B at position `offset + delta`
        let mut cipher2 = RC4Cipher::new(INCOMING_KEY);
        cipher2.skip(offset + delta);
        let mut tick_b_encrypted = tick_b_value.to_be_bytes();
        cipher2.apply(&mut tick_b_encrypted);

        // Now try to sync
        let mut test_cipher = RC4Cipher::new(INCOMING_KEY);
        let result = sync_cipher(
            &mut test_cipher,
            INCOMING_KEY,
            &tick_a_encrypted,
            &tick_b_encrypted,
            delta,
        );

        assert!(result.is_some(), "Sync should succeed");
        let (found_offset, tick_value) = result.unwrap();
        assert_eq!(found_offset, offset, "Should find correct offset");
        assert_eq!(tick_value, tick_b_value, "Should return tick B value");
    }

    #[test]
    fn test_tick_packet_ids() {
        assert_eq!(NEWTICK_ID, 10);
        assert_eq!(MOVE_ID, 62);
    }

    /// After a mid-stream sync succeeds on a tick packet, the aligner's cipher
    /// must be positioned at the START of the NEXT packet's payload, not the
    /// sync packet's. The caller must therefore NOT decrypt the sync packet
    /// itself (doing so emits garbage AND over-advances the cipher, desyncing
    /// every subsequent packet).
    #[test]
    fn test_cipher_positioned_for_next_packet_after_sync() {
        let key = INCOMING_KEY;

        // Plaintext payload lengths for: tickA, middle, tickB, next packet.
        let (pa, pm, pb, pc) = (8usize, 10usize, 8usize, 6usize);

        // Build plaintext payloads. Tick payloads carry the tick counter in
        // their first 4 bytes (big-endian), matching NEWTICK framing.
        let mut a_pl = vec![0xAAu8; pa];
        a_pl[0..4].copy_from_slice(&100i32.to_be_bytes());
        let m_pl = vec![0xBBu8; pm];
        let mut b_pl = vec![0xCCu8; pb];
        b_pl[0..4].copy_from_slice(&101i32.to_be_bytes());
        let c_pl = vec![0xDDu8; pc];

        // Encrypt payloads with one continuous keystream (only payloads are
        // encrypted; headers are plaintext on the wire).
        let mut master = RC4Cipher::new(key);
        let enc_a = master.apply_to_vec(&a_pl);
        let _enc_m = master.apply_to_vec(&m_pl);
        let enc_b = master.apply_to_vec(&b_pl);
        let enc_c = master.apply_to_vec(&c_pl);

        // Helper to frame a packet: 4-byte big-endian size + 1-byte type + payload.
        let frame = |ty: u8, enc: &[u8]| -> Vec<u8> {
            let size = (enc.len() + 5) as u32;
            let mut v = size.to_be_bytes().to_vec();
            v.push(ty);
            v.extend_from_slice(enc);
            v
        };

        let pkt_a = frame(NEWTICK_ID, &enc_a);
        let pkt_b = frame(NEWTICK_ID, &enc_b);

        let mut aligner = TickAligner::new(key);

        // First tick: stored, not yet synced.
        assert!(!aligner.check_alignment(&pkt_a, pkt_a.len(), NEWTICK_ID));
        // A middle (non-tick) packet contributes to the byte delta.
        assert!(!aligner.check_alignment(&[0u8; 5 + 10], 5 + pm, 99));
        // Second tick: should sync.
        assert!(
            aligner.check_alignment(&pkt_b, pkt_b.len(), NEWTICK_ID),
            "aligner should sync on the second tick packet"
        );
        assert!(aligner.is_synced());

        // The cipher must now decrypt the NEXT packet (C) correctly, proving it
        // is positioned past tick B - so tick B must not be decrypted.
        let cipher = aligner.cipher_mut().expect("synced cipher");
        let mut dec_c = enc_c.clone();
        cipher.apply(&mut dec_c);
        assert_eq!(
            dec_c, c_pl,
            "post-sync cipher must decode the packet AFTER the sync packet"
        );
    }

    /// Frame a NewTick packet whose payload encodes `tick` in its first 4 bytes,
    /// using the supplied continuous keystream `master` (only payloads are
    /// encrypted on the wire). Returns (framed_packet, plaintext_payload).
    fn frame_tick(master: &mut RC4Cipher, tick: i32, payload_len: usize) -> (Vec<u8>, Vec<u8>) {
        let mut pl = vec![0u8; payload_len];
        pl[0..4].copy_from_slice(&tick.to_be_bytes());
        let enc = master.apply_to_vec(&pl);
        let size = (enc.len() + 5) as u32;
        let mut framed = size.to_be_bytes().to_vec();
        framed.push(NEWTICK_ID);
        framed.extend_from_slice(&enc);
        (framed, pl)
    }

    /// A healthy synced stream with correct consecutive ticks must validate
    /// without spurious resets, and the cipher must stay aligned (decrypting
    /// each payload back to its plaintext) across many packets.
    #[test]
    fn test_synced_tick_validation_healthy_stream() {
        let key = INCOMING_KEY;
        let mut master = RC4Cipher::new(key);

        let frames: Vec<(Vec<u8>, Vec<u8>)> =
            (50..56).map(|t| frame_tick(&mut master, t, 12)).collect();

        let mut aligner = TickAligner::new_synced(key);

        for (i, (framed, plaintext)) in frames.iter().enumerate() {
            // Mimic the reassembler: validate (non-destructive) then decrypt.
            assert!(
                aligner.validate_synced_tick(framed, NEWTICK_ID),
                "healthy tick #{} should validate",
                i
            );
            assert!(aligner.is_synced(), "stream must stay synced");

            // Real payload decrypt advances the cipher exactly once.
            let cipher = aligner.cipher_mut().expect("synced cipher");
            let mut payload = framed[5..].to_vec();
            cipher.apply(&mut payload);
            assert_eq!(
                &payload, plaintext,
                "cipher must stay aligned and decode payload #{}",
                i
            );
        }

        assert_eq!(
            aligner.current_tick(),
            55,
            "tick counter should track the stream"
        );
    }

    /// An injected tick mismatch on a synced stream must reset the aligner so it
    /// re-syncs, and the offending packet must not be emitted.
    #[test]
    fn test_synced_tick_validation_mismatch_resets() {
        let key = INCOMING_KEY;
        let mut master = RC4Cipher::new(key);

        let (f0, _p0) = frame_tick(&mut master, 50, 12);
        // Next packet decrypts to tick 99 but the stream expects 51.
        let (f1, _p1) = frame_tick(&mut master, 99, 12);

        let mut aligner = TickAligner::new_synced(key);

        // First tick anchors the baseline.
        assert!(aligner.validate_synced_tick(&f0, NEWTICK_ID));
        assert_eq!(aligner.current_tick(), 50);
        // Advance the real cipher to mimic decrypting the first payload.
        {
            let cipher = aligner.cipher_mut().expect("synced cipher");
            let mut payload = f0[5..].to_vec();
            cipher.apply(&mut payload);
        }

        // Second tick is out of sequence -> reset.
        assert!(
            !aligner.validate_synced_tick(&f1, NEWTICK_ID),
            "tick drift must fail validation"
        );
        assert!(!aligner.is_synced(), "mismatch must reset to unsynced");
        assert_eq!(aligner.current_tick(), -1, "reset clears the tick baseline");
    }

    /// Non-tick packets always pass (and don't anchor); a complete-but-too-short
    /// tick packet is treated as corruption and forces a resync.
    #[test]
    fn test_synced_validation_non_tick_and_short_tick() {
        let key = INCOMING_KEY;
        let mut aligner = TickAligner::new_synced(key);

        // Non-tick packet: passes, stays synced, does not establish a baseline.
        let non_tick = vec![0u8; 20];
        assert!(aligner.validate_synced_tick(&non_tick, 99));
        assert!(aligner.is_synced());
        assert_eq!(aligner.current_tick(), -1);

        // Malformed tick packet shorter than header + 4-byte counter -> resync.
        let short = vec![0u8; 8];
        assert!(!aligner.validate_synced_tick(&short, NEWTICK_ID));
        assert!(!aligner.is_synced());
    }
}
