//! RC4 stream cipher implementation.

/// RC4 stream cipher, used for encrypting/decrypting RotMG packets.
#[derive(Clone)]
pub struct RC4Cipher {
    state: [u8; 256],
    i: u8,
    j: u8,
}

impl RC4Cipher {
    /// Create a new RC4 cipher initialized with the given key.
    pub fn new(key: &[u8]) -> Self {
        let mut state = [0u8; 256];

        // Initialize state array
        for (i, byte) in state.iter_mut().enumerate() {
            *byte = i as u8;
        }

        // Key scheduling algorithm (KSA)
        let mut j: u8 = 0;
        for i in 0..256 {
            j = j.wrapping_add(state[i]).wrapping_add(key[i % key.len()]);
            state.swap(i, j as usize);
        }

        Self { state, i: 0, j: 0 }
    }

    /// Reset the cipher to its initial state with a new key.
    pub fn reset(&mut self, key: &[u8]) {
        *self = Self::new(key);
    }

    /// Generate the next byte of the keystream.
    fn next_byte(&mut self) -> u8 {
        self.i = self.i.wrapping_add(1);
        self.j = self.j.wrapping_add(self.state[self.i as usize]);
        self.state.swap(self.i as usize, self.j as usize);

        let idx = self.state[self.i as usize].wrapping_add(self.state[self.j as usize]);
        self.state[idx as usize]
    }

    /// Encrypt or decrypt data in place.
    ///
    /// RC4 is symmetric, so encryption and decryption use the same operation.
    pub fn apply(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            *byte ^= self.next_byte();
        }
    }

    /// Encrypt or decrypt data, returning a new Vec.
    pub fn apply_to_vec(&mut self, data: &[u8]) -> Vec<u8> {
        let mut result = data.to_vec();
        self.apply(&mut result);
        result
    }

    /// Skip `n` bytes of the keystream -- used during initialization to discard
    /// the weak initial bytes of the RC4 keystream.
    pub fn skip(&mut self, n: usize) {
        for _ in 0..n {
            self.next_byte();
        }
    }

    /// Get a single byte from the keystream (advancing the cipher by 1).
    ///
    /// This is the XOR byte used for encryption/decryption.
    #[inline]
    pub fn get_byte(&mut self) -> u8 {
        self.next_byte()
    }
}

impl std::fmt::Debug for RC4Cipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RC4Cipher")
            .field("i", &self.i)
            .field("j", &self.j)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rc4_encrypt_decrypt() {
        let key = b"test_key";
        let plaintext = b"Hello, World!";

        let mut cipher1 = RC4Cipher::new(key);
        let ciphertext = cipher1.apply_to_vec(plaintext);

        let mut cipher2 = RC4Cipher::new(key);
        let decrypted = cipher2.apply_to_vec(&ciphertext);

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_rc4_in_place() {
        let key = b"another_key";
        let original = b"Test data for RC4";

        let mut data = original.to_vec();
        let mut cipher = RC4Cipher::new(key);
        cipher.apply(&mut data);

        // Data should be different after encryption
        assert_ne!(data, original);

        // Decrypt with fresh cipher
        let mut cipher = RC4Cipher::new(key);
        cipher.apply(&mut data);

        assert_eq!(data, original);
    }
}
