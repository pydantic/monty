//! Block buffering shared by the hash cores.
//!
//! Every algorithm here consumes its input in fixed-size blocks and keeps the
//! partial trailing block until more data arrives or the digest is taken. The
//! final block is always left buffered, even when it is full, because BLAKE2
//! finalizes with a flag on the last block and the Merkle–Damgård and sponge
//! constructions pad into it.

/// Feeds `data` through `compress` one block at a time, leaving the trailing
/// block (whole or partial) in `pending` for finalization.
///
/// `pending` never exceeds `block` bytes.
pub(super) fn absorb(pending: &mut Vec<u8>, block: usize, mut data: &[u8], mut compress: impl FnMut(&[u8])) {
    debug_assert!(pending.len() <= block, "pending block overflowed");
    // Top up a partial block before touching whole ones.
    if !pending.is_empty() && pending.len() < block {
        let take = (block - pending.len()).min(data.len());
        pending.extend_from_slice(&data[..take]);
        data = &data[take..];
    }
    if data.is_empty() {
        return;
    }
    // More input follows, so a full buffered block can be compressed now.
    if pending.len() == block {
        compress(pending);
        pending.clear();
    }
    // Whole blocks followed by at least one more byte go straight from the input.
    let whole = (data.len() - 1) / block * block;
    for chunk in data[..whole].chunks_exact(block) {
        compress(chunk);
    }
    pending.extend_from_slice(&data[whole..]);
}

/// Pads a Merkle–Damgård trailing block: `0x80`, zeros, then the message bit
/// length in `length_bytes` bytes written by `write_length` (which receives the
/// slot to fill), producing one or two blocks.
pub(super) fn md_pad(
    pending: &[u8],
    block: usize,
    length_bytes: usize,
    write_length: impl FnOnce(&mut [u8]),
) -> Vec<u8> {
    let mut padded = Vec::with_capacity(2 * block);
    padded.extend_from_slice(pending);
    padded.push(0x80);
    let unpadded = padded.len();
    let total = if unpadded + length_bytes <= block {
        block
    } else {
        2 * block
    };
    padded.resize(total, 0);
    write_length(&mut padded[total - length_bytes..]);
    padded
}
