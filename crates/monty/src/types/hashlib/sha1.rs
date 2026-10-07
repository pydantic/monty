//! SHA-1 (FIPS 180-4), for `hashlib.sha1`.

use serde::{Deserialize, Serialize};

use super::block::{absorb, md_pad};

/// Bytes per compression block.
pub(super) const BLOCK_SIZE: usize = 64;
/// Bytes in a digest.
pub(super) const DIGEST_SIZE: usize = 20;

/// Streaming SHA-1 state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Sha1 {
    state: [u32; 5],
    /// The trailing block not yet compressed, at most [`BLOCK_SIZE`] bytes.
    #[serde(with = "serde_bytes")]
    pending: Vec<u8>,
    /// Bytes absorbed so far, for the length suffix.
    length: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self {
            state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476, 0xc3d2_e1f0],
            pending: Vec::new(),
            length: 0,
        }
    }
}

impl Sha1 {
    pub(crate) fn update(&mut self, data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        let state = &mut self.state;
        absorb(&mut self.pending, BLOCK_SIZE, data, |block| compress(state, block));
    }

    /// The digest of everything absorbed so far; the state stays usable.
    pub(crate) fn digest(&self) -> Vec<u8> {
        let mut state = self.state;
        let bits = self.length.wrapping_mul(8);
        let padded = md_pad(&self.pending, BLOCK_SIZE, 8, |slot| {
            slot.copy_from_slice(&bits.to_be_bytes());
        });
        for block in padded.as_chunks::<BLOCK_SIZE>().0 {
            compress(&mut state, block);
        }
        state.iter().flat_map(|word| word.to_be_bytes()).collect()
    }
}

/// One SHA-1 compression of a 64-byte block.
#[expect(
    clippy::many_single_char_names,
    reason = "the specification's names for the same algorithm"
)]
fn compress(state: &mut [u32; 5], block: &[u8]) {
    let mut w = [0u32; 80];
    for (word, chunk) in w.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*chunk);
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (i, word) in w.iter().enumerate() {
        let (f, k) = match i / 20 {
            0 => ((b & c) | (!b & d), 0x5a82_7999),
            1 => (b ^ c ^ d, 0x6ed9_eba1),
            2 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
            _ => (b ^ c ^ d, 0xca62_c1d6),
        };
        let temp = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = temp;
    }
    for (slot, word) in state.iter_mut().zip([a, b, c, d, e]) {
        *slot = slot.wrapping_add(word);
    }
}
