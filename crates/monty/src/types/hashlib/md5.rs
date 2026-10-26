//! MD5 (RFC 1321), for `hashlib.md5`.

use serde::{Deserialize, Serialize};

use super::block::{absorb, md_pad};

/// Bytes per compression block.
pub(super) const BLOCK_SIZE: usize = 64;
/// Bytes in a digest.
pub(super) const DIGEST_SIZE: usize = 16;

/// Per-round left-rotation amounts.
const SHIFTS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15,
    21,
];

/// Round constants, `floor(abs(sin(i + 1)) * 2**32)`.
const K: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

/// Streaming MD5 state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Md5 {
    state: [u32; 4],
    /// The trailing block not yet compressed, at most [`BLOCK_SIZE`] bytes.
    #[serde(with = "serde_bytes")]
    pending: Vec<u8>,
    /// Bytes absorbed so far, for the length suffix.
    length: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self {
            state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
            pending: Vec::new(),
            length: 0,
        }
    }
}

impl Md5 {
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
            slot.copy_from_slice(&bits.to_le_bytes());
        });
        for block in padded.as_chunks::<BLOCK_SIZE>().0 {
            compress(&mut state, block);
        }
        state.iter().flat_map(|word| word.to_le_bytes()).collect()
    }
}

/// One MD5 compression of a 64-byte block.
#[expect(
    clippy::many_single_char_names,
    reason = "the specification's names for the same algorithm"
)]
fn compress(state: &mut [u32; 4], block: &[u8]) {
    let mut m = [0u32; 16];
    for (word, chunk) in m.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*chunk);
    }
    let [mut a, mut b, mut c, mut d] = *state;
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(f.rotate_left(SHIFTS[i]));
    }
    for (slot, word) in state.iter_mut().zip([a, b, c, d]) {
        *slot = slot.wrapping_add(word);
    }
}
