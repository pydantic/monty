//! Keccak-f[1600] sponge (FIPS 202): SHA3-224/256/384/512 and SHAKE128/256.
//!
//! One [`Keccak`] serves every variant; the rate and the domain-separation
//! suffix pick the algorithm, and the caller chooses how much to squeeze.

use monty_types::ResourceTracker;
use serde::{Deserialize, Serialize};

use super::block::absorb;
use crate::exception_private::RunResult;

/// Domain-separation suffix of the SHA-3 fixed-output functions.
pub(super) const SUFFIX_SHA3: u8 = 0x06;
/// Domain-separation suffix of the SHAKE extendable-output functions.
pub(super) const SUFFIX_SHAKE: u8 = 0x1f;

/// Lanes in the state.
const LANES: usize = 25;
/// Bytes in the state.
const STATE_BYTES: usize = LANES * 8;

/// Iota round constants.
const RC: [u64; 24] = [
    0x0000_0000_0000_0001,
    0x0000_0000_0000_8082,
    0x8000_0000_0000_808a,
    0x8000_0000_8000_8000,
    0x0000_0000_0000_808b,
    0x0000_0000_8000_0001,
    0x8000_0000_8000_8081,
    0x8000_0000_0000_8009,
    0x0000_0000_0000_008a,
    0x0000_0000_0000_0088,
    0x0000_0000_8000_8009,
    0x0000_0000_8000_000a,
    0x0000_0000_8000_808b,
    0x8000_0000_0000_008b,
    0x8000_0000_0000_8089,
    0x8000_0000_0000_8003,
    0x8000_0000_0000_8002,
    0x8000_0000_0000_0080,
    0x0000_0000_0000_800a,
    0x8000_0000_8000_000a,
    0x8000_0000_8000_8081,
    0x8000_0000_0000_8080,
    0x0000_0000_8000_0001,
    0x8000_0000_8000_8008,
];
/// Rho rotation amounts, in the lane order [`PI`] visits.
const RHO: [u32; 24] = [
    1, 3, 6, 10, 15, 21, 28, 36, 45, 55, 2, 14, 27, 41, 56, 8, 25, 43, 62, 18, 39, 61, 20, 44,
];
/// Pi lane permutation, followed from lane 1.
const PI: [usize; 24] = [
    10, 7, 11, 17, 18, 3, 5, 16, 8, 21, 24, 4, 15, 23, 19, 13, 12, 2, 20, 14, 22, 9, 6, 1,
];

/// A Keccak sponge mid-absorption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Keccak {
    state: [u64; LANES],
    /// Bytes absorbed per permutation: `200 - 2 * security_bytes`.
    rate: usize,
    /// The padding byte that separates SHA-3 from SHAKE.
    suffix: u8,
    /// The trailing block not yet absorbed, at most `rate` bytes.
    #[serde(with = "serde_bytes")]
    pending: Vec<u8>,
}

impl Keccak {
    pub(crate) fn new(rate: usize, suffix: u8) -> Self {
        Self {
            state: [0; LANES],
            rate,
            suffix,
            pending: Vec::new(),
        }
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        let state = &mut self.state;
        absorb(&mut self.pending, self.rate, data, |block| absorb_block(state, block));
    }

    /// Pads and squeezes a fixed `length` bytes; the state stays usable.
    pub(crate) fn digest(&self, length: usize) -> Vec<u8> {
        let mut state = self.finalized();
        let mut out = Vec::with_capacity(length);
        squeeze(&mut state, self.rate, length, &mut out);
        out
    }

    /// Pads and squeezes a caller-chosen `length` bytes, polling the deadline
    /// per block since SHAKE output is unbounded; the state stays usable.
    pub(crate) fn squeeze(&self, length: usize, tracker: &ResourceTracker) -> RunResult<Vec<u8>> {
        let mut state = self.finalized();
        let mut out = Vec::with_capacity(length);
        for i in 0.. {
            tracker.check_time_every(i)?;
            if !squeeze(&mut state, self.rate, length, &mut out) {
                break;
            }
        }
        Ok(out)
    }

    /// The state after absorbing the padded trailing block.
    fn finalized(&self) -> [u64; LANES] {
        let mut state = self.state;
        let mut block = self.pending.clone();
        if block.len() == self.rate {
            absorb_block(&mut state, &block);
            block.clear();
        }
        block.push(self.suffix);
        block.resize(self.rate, 0);
        block[self.rate - 1] ^= 0x80;
        absorb_block(&mut state, &block);
        state
    }
}

/// Appends up to one rate's worth of output to `out`, permuting afterwards,
/// until it holds `length` bytes. Returns whether more is needed.
fn squeeze(state: &mut [u64; LANES], rate: usize, length: usize, out: &mut Vec<u8>) -> bool {
    loop {
        let take = (length - out.len()).min(rate);
        out.extend(state.iter().flat_map(|lane| lane.to_le_bytes()).take(take));
        if out.len() == length {
            return false;
        }
        keccak_f(state);
        if take == rate {
            return true;
        }
    }
}

/// XORs one rate-sized block into the state and permutes.
fn absorb_block(state: &mut [u64; LANES], block: &[u8]) {
    debug_assert!(block.len() <= STATE_BYTES);
    for (lane, chunk) in state.iter_mut().zip(block.chunks(8)) {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        *lane ^= u64::from_le_bytes(bytes);
    }
    keccak_f(state);
}

/// The 24-round Keccak-f[1600] permutation.
fn keccak_f(a: &mut [u64; LANES]) {
    for rc in RC {
        // Theta: column parities.
        let mut c = [0u64; 5];
        for (x, parity) in c.iter_mut().enumerate() {
            *parity = a[x] ^ a[x + 5] ^ a[x + 10] ^ a[x + 15] ^ a[x + 20];
        }
        for x in 0..5 {
            let d = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
            for y in 0..5 {
                a[x + 5 * y] ^= d;
            }
        }
        // Rho and pi: rotate each lane and move it along the permutation cycle.
        let mut last = a[1];
        for (lane, rotation) in PI.iter().zip(RHO) {
            let next = a[*lane];
            a[*lane] = last.rotate_left(rotation);
            last = next;
        }
        // Chi: non-linear row mixing.
        for y in 0..5 {
            let row: [u64; 5] = a[5 * y..5 * y + 5].try_into().expect("5-lane row");
            for x in 0..5 {
                a[5 * y + x] = row[x] ^ (!row[(x + 1) % 5] & row[(x + 2) % 5]);
            }
        }
        // Iota.
        a[0] ^= rc;
    }
}
