//! The worker side: lock down, read one request, decode, answer, exit.

use crate::lockdown::lockdown;
use crate::protocol::{
    MAX_ERROR_LEN, Op, REQUEST_HEADER, REQUEST_MAGIC, RESPONSE_MAGIC, u32_at, u64_at,
};
use printnest_imaging::{DecodeError, decode_preview, probe};
use printnest_model::limits::MAX_FILE_BYTES;
use std::io::{Read, Write};

/// Exit codes the parent understands.
pub const EXIT_OK: i32 = 0;
pub const EXIT_LOCKDOWN_FAILED: i32 = 70;
pub const EXIT_BAD_REQUEST: i32 = 71;
pub const EXIT_IO: i32 = 72;

/// Entry point for the worker process. Never returns.
pub fn run() -> ! {
    // Lock down FIRST, before reading a single byte of untrusted input.
    if lockdown().is_err() {
        std::process::exit(EXIT_LOCKDOWN_FAILED);
    }
    let code = match serve(&mut std::io::stdin().lock(), &mut std::io::stdout().lock()) {
        Ok(()) => EXIT_OK,
        Err(code) => code,
    };
    std::process::exit(code);
}

/// Handles one request. Split out so tests can drive it in-process.
///
/// # Errors
/// An exit code when the request itself is malformed or I/O fails.
pub fn serve(input: &mut impl Read, output: &mut impl Write) -> Result<(), i32> {
    let mut header = [0u8; REQUEST_HEADER];
    input
        .read_exact(&mut header)
        .map_err(|_| EXIT_BAD_REQUEST)?;
    if header.get(..4) != Some(REQUEST_MAGIC.as_slice()) {
        return Err(EXIT_BAD_REQUEST);
    }
    let op = header
        .get(4)
        .copied()
        .and_then(Op::from_u8)
        .ok_or(EXIT_BAD_REQUEST)?;
    let max_edge = u32_at(&header, 5).ok_or(EXIT_BAD_REQUEST)?;
    let len = u64_at(&header, 9).ok_or(EXIT_BAD_REQUEST)?;
    if len > MAX_FILE_BYTES {
        return Err(EXIT_BAD_REQUEST);
    }
    let len = usize::try_from(len).map_err(|_| EXIT_BAD_REQUEST)?;
    let mut bytes = vec![0u8; len];
    input.read_exact(&mut bytes).map_err(|_| EXIT_BAD_REQUEST)?;

    let mut out = Vec::new();
    out.extend_from_slice(RESPONSE_MAGIC);
    let result = match op {
        Op::Probe => probe(&bytes).map(|info| (info, None)),
        Op::Preview => {
            decode_preview(&bytes, max_edge).map(|p| (p.info, Some((p.width, p.height, p.rgba))))
        }
    };
    drop(bytes);
    match result {
        Ok((info, preview)) => {
            out.push(0);
            out.push(info.format.code());
            out.extend_from_slice(&info.width.to_le_bytes());
            out.extend_from_slice(&info.height.to_le_bytes());
            let (pw, ph, rgba) = preview.unwrap_or((0, 0, Vec::new()));
            out.extend_from_slice(&pw.to_le_bytes());
            out.extend_from_slice(&ph.to_le_bytes());
            output.write_all(&out).map_err(|_| EXIT_IO)?;
            output.write_all(&rgba).map_err(|_| EXIT_IO)?;
        }
        Err(e) => {
            let msg = error_text(&e);
            out.push(1);
            out.extend_from_slice(&(msg.len() as u16).to_le_bytes());
            out.extend_from_slice(msg.as_bytes());
            output.write_all(&out).map_err(|_| EXIT_IO)?;
        }
    }
    output.flush().map_err(|_| EXIT_IO)
}

fn error_text(e: &DecodeError) -> String {
    let mut s = e.to_string();
    // Truncate on a char boundary.
    while s.len() > MAX_ERROR_LEN {
        s.pop();
    }
    s
}
