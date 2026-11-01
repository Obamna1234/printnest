//! End-to-end sandbox tests: real worker processes, real escape attempts.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use image::{ImageBuffer, ImageEncoder, Rgb, codecs};
use printnest_sandbox::{SandboxError, Worker};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

fn selftest() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_sandbox-selftest"))
}

fn worker() -> Worker {
    Worker::with_exe(selftest())
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img: ImageBuffer<Rgb<u8>, _> =
        ImageBuffer::from_fn(w, h, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 7]));
    let mut out = Vec::new();
    codecs::png::PngEncoder::new(&mut out)
        .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
        .unwrap();
    out
}

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let img: ImageBuffer<Rgb<u8>, _> =
        ImageBuffer::from_fn(w, h, |x, _| Rgb([(x % 256) as u8, 1, 2]));
    let mut out = Vec::new();
    codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(&img)
        .unwrap();
    out
}

#[test]
fn decodes_through_the_sandbox() {
    let w = worker();
    let info = w.probe(&jpeg(1200, 800)).unwrap();
    assert_eq!((info.width, info.height), (1200, 800));
    let p = w.preview(&png(640, 480), 160).unwrap();
    assert_eq!((p.width, p.height), (160, 120));
    assert_eq!(p.rgba.len(), 160 * 120 * 4);
}

#[test]
fn bad_files_come_back_as_clean_errors() {
    let w = worker();
    for bytes in [
        b"not an image".to_vec(),
        b"\x89PNG\r\n\x1a\ntruncated".to_vec(),
        Vec::new(),
    ] {
        match w.probe(&bytes) {
            Err(SandboxError::Decode(_)) => {}
            other => panic!("expected a decode error, got {other:?}"),
        }
    }
    let good = jpeg(64, 64);
    match w.preview(&good[..good.len() / 2], 32) {
        Ok(_) | Err(SandboxError::Decode(_)) => {}
        other => panic!("truncated JPEG: {other:?}"),
    }
}

#[test]
fn rejects_invalid_preview_sizes_before_spawning() {
    assert!(matches!(
        worker().preview(&png(4, 4), 0),
        Err(SandboxError::Decode(_))
    ));
    assert!(matches!(
        worker().preview(&png(4, 4), 100_000),
        Err(SandboxError::Decode(_))
    ));
}

#[test]
fn hung_worker_is_killed_at_the_timeout() {
    let w = Worker::with_args(selftest(), vec![OsString::from("hang")])
        .timeout(Duration::from_millis(500));
    let start = Instant::now();
    assert!(matches!(w.probe(b"x"), Err(SandboxError::Timeout)));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn flooding_worker_is_cut_off() {
    let w = Worker::with_args(selftest(), vec![OsString::from("flood")])
        .timeout(Duration::from_secs(5));
    match w.probe(b"x") {
        Err(SandboxError::BadResponse(_) | SandboxError::Timeout) => {}
        other => panic!("expected the output cap to trigger, got {other:?}"),
    }
}

#[test]
fn missing_worker_is_a_spawn_error() {
    let w = Worker::with_exe(PathBuf::from("/nonexistent/printnest-worker"));
    assert!(matches!(w.probe(b"x"), Err(SandboxError::Spawn(_))));
}

/// On Linux the locked-down worker must be killed by the kernel (SIGSYS)
/// the moment it tries any of these.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[test]
fn escape_attempts_are_killed_by_seccomp() {
    use std::os::unix::process::ExitStatusExt;
    for what in ["open", "write", "socket", "spawn", "thread"] {
        let out = Command::new(selftest())
            .args(["escape", what])
            .env_clear()
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            !stdout.contains("ESCAPED"),
            "{what}: worker got past the sandbox: {stdout}"
        );
        assert_eq!(
            out.status.signal(),
            Some(31),
            "{what}: expected SIGSYS, got {:?}",
            out.status
        );
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[test]
fn memory_limit_stops_huge_allocations() {
    let out = Command::new(selftest())
        .args(["escape", "memory"])
        .env_clear()
        .stderr(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("ESCAPED"));
    assert!(!out.status.success());
}

#[test]
fn normal_work_still_runs_after_lockdown() {
    let out = Command::new(selftest())
        .arg("allowed")
        .env_clear()
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{:?}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("OK "));
}

/// A compromised worker must not be able to signal any other process
/// (the user's apps, the parent). Regression test for the review finding.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[test]
fn worker_cannot_kill_other_processes() {
    use std::os::unix::process::ExitStatusExt;
    let mut victim = Command::new("sleep").arg("30").spawn().unwrap();
    let out = Command::new(selftest())
        .args(["escape", "kill", &victim.id().to_string()])
        .env_clear()
        .output()
        .unwrap();
    let alive = victim.try_wait().unwrap().is_none();
    let _ = victim.kill();
    let _ = victim.wait();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("ESCAPED"));
    assert_eq!(out.status.signal(), Some(31), "expected SIGSYS");
    assert!(alive, "the victim process was killed");
}

/// Picking a FIFO (or device) must fail fast, not hang the app.
#[cfg(unix)]
#[test]
fn fifo_is_refused_without_blocking() {
    let dir = std::env::temp_dir().join(format!("printnest-fifo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("photo.jpg");
    let _ = std::fs::remove_file(&fifo);
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let start = Instant::now();
    let r = printnest_sandbox::read_capped(&fifo);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(matches!(r, Err(SandboxError::Io(_))), "{r:?}");
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn reported_confinement_matches_platform() {
    let c = printnest_sandbox::lockdown::expected();
    if cfg!(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )) {
        assert_eq!(c, printnest_sandbox::Confinement::Seccomp);
    } else {
        assert_ne!(c, printnest_sandbox::Confinement::Seccomp);
    }
}
