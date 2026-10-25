//! BLAKE2b and BLAKE2s (RFC 7693), with the full parameter block CPython's
//! `_blake2` exposes: digest size, key, salt, personalization and the tree
//! hashing fields.
//!
//! The two variants share one algorithm over 64-bit and 32-bit words, so
//! [`blake2_impl!`] stamps each out from the word type, its constants and the
//! parameter-block layout.

/// BLAKE2s's IV, which is SHA-256's.
const IV_256: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];
/// BLAKE2b's IV, which is SHA-512's.
const IV_512: [u64; 8] = [
    0x6a09_e667_f3bc_c908,
    0xbb67_ae85_84ca_a73b,
    0x3c6e_f372_fe94_f82b,
    0xa54f_f53a_5f1d_36f1,
    0x510e_527f_ade6_82d1,
    0x9b05_688c_2b3e_6c1f,
    0x1f83_d9ab_fb41_bd6b,
    0x5be0_cd19_137e_2179,
];

/// Message word schedule, one row per round (rounds past 10 wrap).
const SIGMA: [[usize; 16]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];

/// The parameter block fields as `hashlib.blake2b()` / `blake2s()` take them,
/// validated against the variant's limits before construction.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Blake2Params<'a> {
    pub digest_size: u8,
    pub key: &'a [u8],
    pub salt: &'a [u8],
    pub person: &'a [u8],
    pub fanout: u8,
    pub depth: u8,
    pub leaf_size: u32,
    pub node_offset: u64,
    pub node_depth: u8,
    pub inner_size: u8,
    pub last_node: bool,
}

/// Defines one BLAKE2 variant over `$word`.
///
/// `$block` is the block size in bytes, `$max_digest` the digest and key
/// limit, `$rounds` the compression rounds,
/// `$rot` the four G rotations, `$salt_at` where the salt starts in the
/// parameter block (the personalization follows it) and `$offset_bytes` the
/// width of the `node_offset` field.
macro_rules! blake2_impl {
    (
        $(#[$meta:meta])*
        $name:ident, $word:ty, $iv:expr, $block:expr, $max_digest:expr, $rounds:expr, $rot:expr, $salt_at:expr,
        $offset_bytes:expr
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Serialize, Deserialize)]
        pub(crate) struct $name {
            h: [$word; 8],
            /// Bytes compressed so far, the `t` counter.
            counter: u128,
            /// The trailing block not yet compressed, at most `BLOCK_SIZE` bytes.
            /// Never compressed until more data arrives, since the final block
            /// carries the finalization flag.
            #[serde(with = "serde_bytes")]
            pending: Vec<u8>,
            /// Whether to finalize as the last node of a tree.
            last_node: bool,
            /// The `digest_size` parameter, which `digest()` honours.
            digest_size: u8,
        }

        impl $name {
            /// Bytes per compression block.
            pub(crate) const BLOCK_SIZE: usize = $block;
            /// The largest digest, also the largest key.
            pub(crate) const MAX_DIGEST_SIZE: u8 = $max_digest;
            /// Salt and personalization length.
            pub(crate) const SALT_SIZE: usize = Self::BLOCK_SIZE / 8;
            /// Widest `node_offset` the parameter block holds.
            pub(crate) const MAX_NODE_OFFSET: u64 = if $offset_bytes == 8 { u64::MAX } else { (1 << ($offset_bytes * 8)) - 1 };

            const IV: [$word; 8] = $iv;
            const WORD_BYTES: usize = ::std::mem::size_of::<$word>();

            /// Builds the parameter block from already-validated `params` and
            /// absorbs the key block, if any.
            pub(crate) fn new(params: Blake2Params<'_>) -> Self {
                debug_assert!(params.key.len() <= Self::MAX_DIGEST_SIZE as usize);
                debug_assert!(params.salt.len() <= Self::SALT_SIZE && params.person.len() <= Self::SALT_SIZE);
                let mut block = [0u8; Self::BLOCK_SIZE / 2];
                block[0] = params.digest_size;
                block[1] = u8::try_from(params.key.len()).expect("key length validated");
                block[2] = params.fanout;
                block[3] = params.depth;
                block[4..8].copy_from_slice(&params.leaf_size.to_le_bytes());
                block[8..8 + $offset_bytes].copy_from_slice(&params.node_offset.to_le_bytes()[..$offset_bytes]);
                block[8 + $offset_bytes] = params.node_depth;
                block[9 + $offset_bytes] = params.inner_size;
                block[$salt_at..$salt_at + params.salt.len()].copy_from_slice(params.salt);
                let person_at = $salt_at + Self::SALT_SIZE;
                block[person_at..person_at + params.person.len()].copy_from_slice(params.person);

                let mut h = Self::IV;
                for (word, chunk) in h.iter_mut().zip(block.chunks_exact(Self::WORD_BYTES)) {
                    *word ^= <$word>::from_le_bytes(chunk.try_into().expect("word-sized chunk"));
                }
                let mut hasher = Self {
                    h,
                    counter: 0,
                    pending: Vec::new(),
                    last_node: params.last_node,
                    digest_size: params.digest_size,
                };
                if !params.key.is_empty() {
                    let mut key_block = vec![0u8; Self::BLOCK_SIZE];
                    key_block[..params.key.len()].copy_from_slice(params.key);
                    hasher.update(&key_block);
                }
                hasher
            }

            pub(crate) fn digest_size(&self) -> usize {
                usize::from(self.digest_size)
            }

            pub(crate) fn update(&mut self, mut data: &[u8]) {
                while !data.is_empty() {
                    if self.pending.len() == Self::BLOCK_SIZE {
                        self.counter += Self::BLOCK_SIZE as u128;
                        let (h, counter, pending) = (&mut self.h, self.counter, &self.pending);
                        compress(h, pending, counter, false, false);
                        self.pending.clear();
                    }
                    let take = (Self::BLOCK_SIZE - self.pending.len()).min(data.len());
                    self.pending.extend_from_slice(&data[..take]);
                    data = &data[take..];
                }
            }

            /// The digest of everything absorbed so far; the state stays usable.
            pub(crate) fn digest(&self) -> Vec<u8> {
                let mut h = self.h;
                let counter = self.counter + self.pending.len() as u128;
                let mut block = self.pending.clone();
                block.resize(Self::BLOCK_SIZE, 0);
                compress(&mut h, &block, counter, true, self.last_node);
                h.iter().flat_map(|word| word.to_le_bytes()).take(self.digest_size()).collect()
            }
        }

        /// One compression of a full block, with the counter after it and the
        /// finalization flags.
        #[expect(clippy::cast_possible_truncation, reason = "the counter is split into two words")]
        fn compress(h: &mut [$word; 8], block: &[u8], counter: u128, last: bool, last_node: bool) {
            let mut m = [0 as $word; 16];
            for (word, chunk) in m.iter_mut().zip(block.chunks_exact($name::WORD_BYTES)) {
                *word = <$word>::from_le_bytes(chunk.try_into().expect("word-sized chunk"));
            }
            let mut v = [0 as $word; 16];
            v[..8].copy_from_slice(h);
            v[8..].copy_from_slice(&$name::IV);
            v[12] ^= counter as $word;
            v[13] ^= (counter >> (8 * $name::WORD_BYTES)) as $word;
            if last {
                v[14] = !v[14];
            }
            if last_node {
                v[15] = !v[15];
            }
            for round in 0..$rounds {
                let s = &SIGMA[round % 10];
                g(&mut v, 0, 4, 8, 12, m[s[0]], m[s[1]]);
                g(&mut v, 1, 5, 9, 13, m[s[2]], m[s[3]]);
                g(&mut v, 2, 6, 10, 14, m[s[4]], m[s[5]]);
                g(&mut v, 3, 7, 11, 15, m[s[6]], m[s[7]]);
                g(&mut v, 0, 5, 10, 15, m[s[8]], m[s[9]]);
                g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
                g(&mut v, 2, 7, 8, 13, m[s[12]], m[s[13]]);
                g(&mut v, 3, 4, 9, 14, m[s[14]], m[s[15]]);
            }
            for (i, word) in h.iter_mut().enumerate() {
                *word ^= v[i] ^ v[i + 8];
            }
        }

        /// The G mixing function on four words of `v`.
        fn g(v: &mut [$word; 16], a: usize, b: usize, c: usize, d: usize, x: $word, y: $word) {
            const ROT: [u32; 4] = $rot;
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
            v[d] = (v[d] ^ v[a]).rotate_right(ROT[0]);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(ROT[1]);
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
            v[d] = (v[d] ^ v[a]).rotate_right(ROT[2]);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(ROT[3]);
        }
    };
}

pub(crate) mod b {
    //! BLAKE2b: 64-bit words, 128-byte blocks, digests up to 64 bytes.
    use serde::{Deserialize, Serialize};

    use super::{Blake2Params, IV_512, SIGMA};

    blake2_impl!(
        /// Streaming BLAKE2b state.
        Blake2b,
        u64,
        IV_512,
        128,
        64,
        12,
        [32, 24, 16, 63],
        32,
        8
    );
}

pub(crate) mod s {
    //! BLAKE2s: 32-bit words, 64-byte blocks, digests up to 32 bytes.
    use serde::{Deserialize, Serialize};

    use super::{Blake2Params, IV_256, SIGMA};

    blake2_impl!(
        /// Streaming BLAKE2s state.
        Blake2s,
        u32,
        IV_256,
        64,
        32,
        10,
        [16, 12, 8, 7],
        16,
        6
    );
}

pub(crate) use b::Blake2b;
pub(crate) use s::Blake2s;
