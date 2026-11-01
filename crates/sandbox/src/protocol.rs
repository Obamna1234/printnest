//! The wire format between the app and the decoder worker.
//!
//! Deliberately tiny and fixed-size so both sides can validate every field.
//! Integers are little-endian.
//!
//! Request:  b"PNQ1" | op: u8 | max_edge: u32 | len: u64 | len bytes
//! Response: b"PNA1" | status: u8 | ...
//!   status 0 (ok):    format: u8 | width: u32 | height: u32
//!                     | preview_w: u32 | preview_h: u32 | preview_w*preview_h*4 bytes
//!                     (preview fields are 0 for a probe)
//!   status 1 (error): msg_len: u16 | msg_len bytes of UTF-8 (≤ 512)

pub const REQUEST_MAGIC: &[u8; 4] = b"PNQ1";
pub const RESPONSE_MAGIC: &[u8; 4] = b"PNA1";
pub const MAX_ERROR_LEN: usize = 512;

/// What the worker should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Read the header only.
    Probe = 1,
    /// Decode and return a downscaled preview.
    Preview = 2,
}

impl Op {
    #[must_use]
    pub fn from_u8(v: u8) -> Option<Op> {
        match v {
            1 => Some(Op::Probe),
            2 => Some(Op::Preview),
            _ => None,
        }
    }
}

/// Size of the fixed request header.
pub const REQUEST_HEADER: usize = 4 + 1 + 4 + 8;
/// Size of the fixed part of an OK response.
pub const RESPONSE_OK_HEADER: usize = 4 + 1 + 1 + 4 * 4;

/// Just the fixed request header; the photo bytes follow it unchanged, so
/// the parent never needs a second copy of a large photo.
#[must_use]
pub fn request_header(op: Op, max_edge: u32, len: u64) -> [u8; REQUEST_HEADER] {
    let mut h = [0u8; REQUEST_HEADER];
    let parts: [&[u8]; 4] = [
        REQUEST_MAGIC,
        &[op as u8],
        &max_edge.to_le_bytes(),
        &len.to_le_bytes(),
    ];
    let mut at = 0;
    for part in parts {
        for (dst, src) in h.iter_mut().skip(at).zip(part) {
            *dst = *src;
        }
        at += part.len();
    }
    h
}

#[must_use]
pub fn encode_request(op: Op, max_edge: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(REQUEST_HEADER + bytes.len());
    out.extend_from_slice(REQUEST_MAGIC);
    out.push(op as u8);
    out.extend_from_slice(&max_edge.to_le_bytes());
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
    out
}

/// Reads a little-endian u32 at `at`, or `None` if out of range.
#[must_use]
pub fn u32_at(buf: &[u8], at: usize) -> Option<u32> {
    let b: [u8; 4] = buf.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(b))
}

/// Reads a little-endian u64 at `at`, or `None` if out of range.
#[must_use]
pub fn u64_at(buf: &[u8], at: usize) -> Option<u64> {
    let b: [u8; 8] = buf.get(at..at.checked_add(8)?)?.try_into().ok()?;
    Some(u64::from_le_bytes(b))
}
