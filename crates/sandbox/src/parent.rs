//! The app side: start the worker, send the bytes, enforce the timeout and
//! output caps, and validate every field of the answer.
//!
//! The worker is this same executable started with one fixed argument
//! ([`WORKER_ARG`]). Nothing derived from user data goes on its command
//! line; the photo travels over stdin. Its environment is cleared and its
//! stderr discarded.

use crate::protocol::{
    MAX_ERROR_LEN, Op, RESPONSE_MAGIC, RESPONSE_OK_HEADER, request_header, u32_at,
};
use printnest_imaging::{Format, Info, MAX_EDGE_PIXELS, MAX_PREVIEW_EDGE, Preview};
use printnest_model::limits::{DECODE_TIMEOUT_SECS, MAX_FILE_BYTES, MAX_IMAGE_PIXELS};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The argument that turns a PrintNest executable into a decoder worker.
pub const WORKER_ARG: &str = "--printnest-decode-worker";

/// Why a decode through the sandbox failed.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("{0}")]
    Decode(String),
    #[error("the photo took longer than {DECODE_TIMEOUT_SECS} s to read and was stopped")]
    Timeout,
    #[error("the photo decoder broke a sandbox rule and was stopped (possible malicious file)")]
    Violation,
    #[error("the photo decoder stopped unexpectedly ({0}); the file may be damaged or malicious")]
    Crashed(String),
    #[error("the photo decoder sent an invalid answer: {0}")]
    BadResponse(&'static str),
    #[error("file is larger than {MAX_FILE_BYTES} bytes")]
    TooLarge,
    #[error(
        "the photo needs more memory to open than PrintNest allows; try a smaller or 8-bit version"
    )]
    TooBig,
    #[error("could not run the photo decoder: {0}")]
    Spawn(String),
    #[error("could not read the photo: {0}")]
    Io(String),
}

/// How to start a worker.
#[derive(Debug, Clone)]
pub struct Worker {
    exe: PathBuf,
    args: Vec<OsString>,
    timeout: Duration,
}

impl Worker {
    /// A worker that is this running executable.
    ///
    /// On Linux this is `/proc/self/exe`, which always means the exact
    /// program file that is running, even if the file at its path has since
    /// been replaced. Elsewhere it is the path the OS reports; replacing
    /// that needs write access to the install folder.
    ///
    /// # Errors
    /// If the OS cannot report the current executable's path.
    pub fn current_exe() -> Result<Self, SandboxError> {
        if cfg!(target_os = "linux") {
            let me = PathBuf::from("/proc/self/exe");
            if me.exists() {
                return Ok(Self::with_exe(me));
            }
        }
        // nosemgrep: rust.lang.security.current-exe.current-exe -- only used to relaunch ourselves as the locked-down decoder; see above
        let exe = std::env::current_exe().map_err(|e| SandboxError::Spawn(e.to_string()))?;
        Ok(Self::with_exe(exe))
    }

    /// A worker at an explicit path (tests, or a separate worker binary).
    #[must_use]
    pub fn with_exe(exe: PathBuf) -> Self {
        Worker {
            exe,
            args: vec![OsString::from(WORKER_ARG)],
            timeout: Duration::from_secs(DECODE_TIMEOUT_SECS),
        }
    }

    /// A process at `exe` with custom arguments. For the sandbox's own
    /// tests only; the app always uses [`Worker::current_exe`].
    #[doc(hidden)]
    #[must_use]
    pub fn with_args(exe: PathBuf, args: Vec<OsString>) -> Self {
        Worker {
            exe,
            args,
            timeout: Duration::from_secs(DECODE_TIMEOUT_SECS),
        }
    }

    /// Overrides the timeout (tests).
    #[must_use]
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// Reads only the image header.
    ///
    /// # Errors
    /// See [`SandboxError`].
    pub fn probe(&self, bytes: &[u8]) -> Result<Info, SandboxError> {
        let raw = self.call(Op::Probe, 0, bytes, RESPONSE_OK_HEADER + MAX_ERROR_LEN)?;
        parse_response(&raw, None).map(|(info, _)| info)
    }

    /// Decodes a preview no larger than `max_edge` on its longer side.
    ///
    /// # Errors
    /// See [`SandboxError`].
    pub fn preview(&self, bytes: &[u8], max_edge: u32) -> Result<Preview, SandboxError> {
        if max_edge == 0 || max_edge > MAX_PREVIEW_EDGE {
            return Err(SandboxError::Decode("invalid preview size".into()));
        }
        let pixels = usize::try_from(u64::from(max_edge) * u64::from(max_edge) * 4)
            .map_err(|_| SandboxError::Decode("preview too large".into()))?;
        let cap = RESPONSE_OK_HEADER + pixels.max(MAX_ERROR_LEN);
        let raw = self.call(Op::Preview, max_edge, bytes, cap)?;
        let (info, preview) = parse_response(&raw, Some(max_edge))?;
        let (width, height, rgba) = preview.ok_or(SandboxError::BadResponse("missing preview"))?;
        Ok(Preview {
            info,
            width,
            height,
            rgba,
        })
    }

    fn spawn(&self) -> Result<Child, SandboxError> {
        Command::new(&self.exe)
            .args(&self.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| SandboxError::Spawn(e.to_string()))
    }

    fn call(
        &self,
        op: Op,
        max_edge: u32,
        bytes: &[u8],
        cap: usize,
    ) -> Result<Vec<u8>, SandboxError> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(SandboxError::TooLarge);
        }
        let mut child = self.spawn()?;
        let header = request_header(op, max_edge, bytes.len() as u64);
        let mut stdin = child
            .stdin
            .take()
            .ok_or(SandboxError::Spawn("no stdin".into()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or(SandboxError::Spawn("no stdout".into()))?;
        let timeout = self.timeout;

        // Write and read on their own (scoped) threads so a stuck worker
        // can't block us, and so the photo is streamed from the caller's
        // buffer without copying it.
        std::thread::scope(|scope| {
            let writer = scope.spawn(move || {
                // A worker that dies early closes the pipe; reported below.
                let _ = stdin
                    .write_all(&header)
                    .and_then(|()| stdin.write_all(bytes));
            });
            let reader = scope.spawn(move || {
                let mut out = Vec::new();
                let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
                let r = (&mut stdout).take(limit).read_to_end(&mut out);
                (r.map(|_| ()), out)
            });

            let deadline = Instant::now() + timeout;
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) if Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(SandboxError::Timeout);
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                    Err(e) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(SandboxError::Io(e.to_string()));
                    }
                }
            };
            let _ = writer.join();
            let (read_result, out) = reader
                .join()
                .map_err(|_| SandboxError::Io("reader thread failed".into()))?;

            if !status.success() {
                return Err(classify_exit(status));
            }
            read_result.map_err(|e| SandboxError::Io(e.to_string()))?;
            if out.len() > cap {
                return Err(SandboxError::BadResponse("answer larger than allowed"));
            }
            Ok(out)
        })
    }
}

#[cfg(unix)]
fn classify_exit(status: std::process::ExitStatus) -> SandboxError {
    use std::os::unix::process::ExitStatusExt;
    match status.signal() {
        Some(31) => SandboxError::Violation, // SIGSYS: seccomp killed it
        // SIGABRT: allocation failed at the memory cap; SIGXCPU: CPU cap.
        Some(6) => SandboxError::TooBig,
        Some(9) | Some(24) => SandboxError::Crashed("resource limit reached".into()),
        Some(sig) => SandboxError::Crashed(format!("signal {sig}")),
        None => exit_code_error(status.code()),
    }
}

#[cfg(not(unix))]
fn classify_exit(status: std::process::ExitStatus) -> SandboxError {
    exit_code_error(status.code())
}

fn exit_code_error(code: Option<i32>) -> SandboxError {
    match code {
        Some(crate::worker::EXIT_LOCKDOWN_FAILED) => {
            SandboxError::Crashed("the sandbox could not be set up".into())
        }
        Some(c) => SandboxError::Crashed(format!("exit code {c}")),
        None => SandboxError::Crashed("unknown".into()),
    }
}

type PreviewParts = (u32, u32, Vec<u8>);

/// Validates a raw response. Every number is checked against the limits
/// before it is trusted.
fn parse_response(
    raw: &[u8],
    max_edge: Option<u32>,
) -> Result<(Info, Option<PreviewParts>), SandboxError> {
    if raw.get(..4) != Some(RESPONSE_MAGIC.as_slice()) {
        return Err(SandboxError::BadResponse("bad magic"));
    }
    match raw.get(4) {
        Some(0) => {}
        Some(1) => {
            let len = raw
                .get(5..7)
                .and_then(|b| <[u8; 2]>::try_from(b).ok())
                .map(|a| usize::from(u16::from_le_bytes(a)))
                .ok_or(SandboxError::BadResponse("short error"))?;
            if len > MAX_ERROR_LEN {
                return Err(SandboxError::BadResponse("error message too long"));
            }
            let msg = raw
                .get(7..7 + len)
                .ok_or(SandboxError::BadResponse("short error"))?;
            // Plain printable ASCII only: no control, bidi or zero-width
            // characters from a possibly compromised worker reach the UI.
            let msg = String::from_utf8_lossy(msg)
                .chars()
                .filter(|c| c.is_ascii_graphic() || *c == ' ')
                .collect::<String>();
            return Err(SandboxError::Decode(msg));
        }
        _ => return Err(SandboxError::BadResponse("bad status")),
    }
    let format = raw
        .get(5)
        .copied()
        .and_then(Format::from_code)
        .ok_or(SandboxError::BadResponse("bad format"))?;
    let field = |i: usize| u32_at(raw, 6 + 4 * i).ok_or(SandboxError::BadResponse("short header"));
    let (width, height, pw, ph) = (field(0)?, field(1)?, field(2)?, field(3)?);
    if width == 0
        || height == 0
        || width > MAX_EDGE_PIXELS
        || height > MAX_EDGE_PIXELS
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err(SandboxError::BadResponse("impossible image size"));
    }
    let info = Info {
        format,
        width,
        height,
    };
    let pixels = raw.get(RESPONSE_OK_HEADER..).unwrap_or_default();
    match max_edge {
        None => {
            if pw != 0 || ph != 0 || !pixels.is_empty() {
                return Err(SandboxError::BadResponse("unexpected pixels"));
            }
            Ok((info, None))
        }
        Some(max) => {
            if pw == 0
                || ph == 0
                || pw > max
                || ph > max
                || pw > width.max(height)
                || ph > width.max(height)
            {
                return Err(SandboxError::BadResponse("bad preview size"));
            }
            let expected = u64::from(pw) * u64::from(ph) * 4;
            if pixels.len() as u64 != expected {
                return Err(SandboxError::BadResponse("pixel data has the wrong length"));
            }
            Ok((info, Some((pw, ph, pixels.to_vec()))))
        }
    }
}

/// Validates a raw worker answer without using it. Exposed for the fuzz
/// targets: whatever bytes a compromised worker sends, this must return an
/// error or a value within the limits, never panic.
#[doc(hidden)]
pub fn check_response(raw: &[u8], max_edge: Option<u32>) -> Result<(), SandboxError> {
    let (info, preview) = parse_response(raw, max_edge)?;
    debug_assert!(u64::from(info.width) * u64::from(info.height) <= MAX_IMAGE_PIXELS);
    if let Some((w, h, px)) = preview {
        debug_assert_eq!(px.len() as u64, u64::from(w) * u64::from(h) * 4);
    }
    Ok(())
}

/// Reads a user-picked file, refusing anything over the size limit without
/// reading it all. The parent reads; the worker never touches the disk.
///
/// # Errors
/// On I/O failure or if the file is too large.
pub fn read_capped(path: &Path) -> Result<Vec<u8>, SandboxError> {
    // Refuse FIFOs, devices and directories BEFORE opening: opening a FIFO
    // would block forever.
    let meta = std::fs::metadata(path).map_err(|e| SandboxError::Io(e.to_string()))?;
    if !meta.is_file() {
        return Err(SandboxError::Io("not a regular file".into()));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    // Non-blocking open closes the race where the path is swapped for a
    // FIFO between the check and the open.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| SandboxError::Io(e.to_string()))?;
    let meta = file
        .metadata()
        .map_err(|e| SandboxError::Io(e.to_string()))?;
    if !meta.is_file() {
        return Err(SandboxError::Io("not a regular file".into()));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(SandboxError::TooLarge);
    }
    let mut buf = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| SandboxError::Io(e.to_string()))?;
    if buf.len() as u64 > MAX_FILE_BYTES {
        return Err(SandboxError::TooLarge);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_header(format: u8, w: u32, h: u32, pw: u32, ph: u32) -> Vec<u8> {
        let mut v = RESPONSE_MAGIC.to_vec();
        v.push(0);
        v.push(format);
        for x in [w, h, pw, ph] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v
    }

    #[test]
    fn accepts_well_formed_answers() {
        let r = ok_header(1, 4000, 3000, 0, 0);
        let (info, p) = parse_response(&r, None).unwrap();
        assert_eq!((info.width, info.height, p.is_none()), (4000, 3000, true));
        let mut r = ok_header(2, 40, 30, 4, 3);
        r.extend(vec![7u8; 4 * 3 * 4]);
        let (_, p) = parse_response(&r, Some(4)).unwrap();
        assert_eq!(p.unwrap().2.len(), 48);
    }

    #[test]
    fn rejects_lying_workers() {
        // Each of these is what a compromised worker might send.
        let cases: Vec<(Vec<u8>, Option<u32>)> = vec![
            (b"XXXX".to_vec(), None),
            (ok_header(9, 10, 10, 0, 0), None), // bad format
            (ok_header(1, 0, 10, 0, 0), None),  // zero size
            (ok_header(1, 20_000, 20_000, 0, 0), None), // over pixel limit
            (ok_header(1, 10, 10, 5, 5), None), // pixels on a probe
            (ok_header(1, 100, 100, 50, 50), Some(10)), // preview over max
            (ok_header(1, 100, 100, 10, 10), Some(10)), // missing pixel data
            (
                {
                    let mut r = ok_header(1, 100, 100, 1, 1);
                    r.extend([0; 5]);
                    r
                },
                Some(10),
            ),
            (
                {
                    let mut r = RESPONSE_MAGIC.to_vec();
                    r.extend([1, 0xFF, 0xFF]);
                    r
                },
                None,
            ), // huge error len
            (RESPONSE_MAGIC.to_vec(), None),
        ];
        for (i, (raw, max)) in cases.into_iter().enumerate() {
            assert!(
                matches!(parse_response(&raw, max), Err(SandboxError::BadResponse(_))),
                "case {i} was accepted"
            );
        }
    }

    #[test]
    fn error_messages_are_sanitised() {
        let mut r = RESPONSE_MAGIC.to_vec();
        let msg = "bad\x1b[31m file\n\u{202e}gpj.exe".as_bytes();
        r.push(1);
        r.extend_from_slice(&(msg.len() as u16).to_le_bytes());
        r.extend_from_slice(msg);
        match parse_response(&r, None) {
            Err(SandboxError::Decode(m)) => {
                assert!(m.chars().all(|c| c.is_ascii_graphic() || c == ' '), "{m:?}")
            }
            other => panic!("{other:?}"),
        }
    }
}
