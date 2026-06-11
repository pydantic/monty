//! Length-prefixed framing for protocol messages over byte streams.
//!
//! Each frame is a 4-byte unsigned **little-endian** length prefix followed by
//! that many bytes of protobuf. LE matches the convention used elsewhere in
//! monty's serialization and is trivial to implement in any language
//! (`readUInt32LE` in Node, `struct.unpack('<I', ...)` in Python).
//!
//! The reader enforces [`MAX_FRAME_LEN`] so a corrupted or byzantine peer
//! cannot make the receiving process allocate unbounded memory from a single
//! bogus length prefix. Writers flush after every frame — the protocol is a
//! strict alternation, so an unflushed frame would deadlock both sides.

use std::{
    error, fmt,
    io::{self, Read, Write},
};

use prost::Message;

/// Default maximum frame length (256 MiB).
///
/// Far above any sane payload, but small enough that a corrupted length
/// prefix cannot trigger a multi-gigabyte allocation in the receiver.
pub const MAX_FRAME_LEN: u32 = 256 * 1024 * 1024;

/// Framing or decoding failure while reading or writing protocol messages.
#[derive(Debug)]
pub enum FrameError {
    /// Underlying stream I/O failure (includes broken pipes — peer death).
    Io(io::Error),
    /// Frame contents were not a valid protobuf message.
    Decode(prost::DecodeError),
    /// Length prefix exceeded the reader's maximum frame length.
    FrameTooLarge {
        /// Length claimed by the prefix.
        len: u32,
        /// The reader's configured maximum.
        max: u32,
    },
    /// The stream ended mid-frame: the peer died while writing.
    Truncated,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "frame I/O error: {e}"),
            Self::Decode(e) => write!(f, "frame decode error: {e}"),
            Self::FrameTooLarge { len, max } => write!(f, "frame of {len} bytes exceeds maximum of {max} bytes"),
            Self::Truncated => f.write_str("stream ended mid-frame"),
        }
    }
}

impl error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Writes length-prefixed protobuf frames to a byte stream.
///
/// Flushes after every frame; see the module docs for why this is required.
#[derive(Debug)]
pub struct FrameWriter<W: Write> {
    inner: W,
}

impl<W: Write> FrameWriter<W> {
    /// Wraps a byte stream. Pass a buffered writer (e.g. `BufWriter<Stdout>`)
    /// when the underlying stream is unbuffered.
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    /// Direct access to the underlying stream — for tests that need to write
    /// raw (malformed) bytes.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Encodes `msg` and writes it as one frame, then flushes.
    pub fn write(&mut self, msg: &impl Message) -> Result<(), FrameError> {
        let len = u32::try_from(msg.encoded_len()).map_err(|_| FrameError::FrameTooLarge {
            len: u32::MAX,
            max: MAX_FRAME_LEN,
        })?;
        // encode_to_vec cannot fail (Vec<u8> grows as needed)
        let body = msg.encode_to_vec();
        self.inner.write_all(&len.to_le_bytes())?;
        self.inner.write_all(&body)?;
        self.inner.flush()?;
        Ok(())
    }
}

/// Reads length-prefixed protobuf frames from a byte stream.
#[derive(Debug)]
pub struct FrameReader<R: Read> {
    inner: R,
    max_frame_len: u32,
}

impl<R: Read> FrameReader<R> {
    /// Wraps a byte stream with the default [`MAX_FRAME_LEN`].
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            max_frame_len: MAX_FRAME_LEN,
        }
    }

    /// Wraps a byte stream with a custom maximum frame length.
    pub fn with_max_frame_len(inner: R, max_frame_len: u32) -> Self {
        Self { inner, max_frame_len }
    }

    /// Reads one frame and decodes it as `M`.
    ///
    /// Returns `Ok(None)` on a clean EOF at a frame boundary (the peer closed
    /// the stream between messages). EOF *inside* a frame is
    /// [`FrameError::Truncated`] — the peer died mid-write.
    pub fn read<M: Message + Default>(&mut self) -> Result<Option<M>, FrameError> {
        let mut len_bytes = [0u8; 4];
        match read_exact_or_eof(&mut self.inner, &mut len_bytes)? {
            ReadOutcome::CleanEof => return Ok(None),
            ReadOutcome::Truncated => return Err(FrameError::Truncated),
            ReadOutcome::Filled => {}
        }
        let len = u32::from_le_bytes(len_bytes);
        if len > self.max_frame_len {
            return Err(FrameError::FrameTooLarge {
                len,
                max: self.max_frame_len,
            });
        }
        let mut body = vec![0u8; len as usize];
        match read_exact_or_eof(&mut self.inner, &mut body)? {
            ReadOutcome::Filled => {}
            // EOF after a length prefix is always mid-frame.
            ReadOutcome::CleanEof | ReadOutcome::Truncated => return Err(FrameError::Truncated),
        }
        M::decode(body.as_slice()).map(Some).map_err(FrameError::Decode)
    }
}

/// Outcome of [`read_exact_or_eof`].
enum ReadOutcome {
    /// The buffer was completely filled.
    Filled,
    /// EOF before any byte was read.
    CleanEof,
    /// EOF after some but not all bytes were read.
    Truncated,
}

/// Like `read_exact` but distinguishes "EOF at the boundary" from "EOF
/// mid-buffer", which the framing layer must report differently.
fn read_exact_or_eof(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<ReadOutcome> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => {
                return Ok(if filled == 0 {
                    ReadOutcome::CleanEof
                } else {
                    ReadOutcome::Truncated
                });
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(ReadOutcome::Filled)
}
