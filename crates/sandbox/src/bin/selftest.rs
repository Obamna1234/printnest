//! Test-only helper for the sandbox's own tests. NOT shipped.
//!
//! - With `--printnest-decode-worker` it is a normal decoder worker.
//! - With `escape <what>` it locks down exactly like a worker, then tries a
//!   forbidden action. The tests expect the process to be killed by the
//!   kernel (SIGSYS) before the action succeeds.
//! - With `hang` / `spin` / `flood` it misbehaves after lockdown so the
//!   parent's timeout and output caps can be tested.

#![forbid(unsafe_code)]
#![allow(clippy::print_stdout, clippy::print_stderr)]

use printnest_sandbox::lockdown::lockdown;
use std::io::Write;

fn main() {
    printnest_sandbox::maybe_run_worker();
    // nosemgrep: rust.lang.security.args.args -- test helper argument parsing
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mode, what) = match args.as_slice() {
        [m] => (m.as_str(), ""),
        [m, w] | [m, w, _] => (m.as_str(), w.as_str()),
        _ => std::process::exit(2),
    };
    if let Err(e) = lockdown() {
        eprintln!("lockdown failed: {e}");
        std::process::exit(70);
    }
    match (mode, what) {
        ("escape", "open") => {
            let ok = std::fs::File::open("/etc/hostname").is_ok();
            println!("ESCAPED open: {ok}");
        }
        ("escape", "write") => {
            let ok = std::fs::write("/tmp/printnest-sandbox-escape", b"x").is_ok();
            println!("ESCAPED write: {ok}");
        }
        ("escape", "socket") => {
            let ok = std::net::TcpStream::connect("127.0.0.1:9").is_ok();
            println!("ESCAPED socket: {ok}");
        }
        ("escape", "spawn") => {
            let ok = std::process::Command::new("/bin/true").status().is_ok();
            println!("ESCAPED spawn: {ok}");
        }
        #[cfg(unix)]
        ("escape", "kill") => {
            // Try to kill another process (the pid comes on stdin-free argv).
            // nosemgrep: rust.lang.security.args.args -- test helper argument parsing
            let target = std::env::args()
                .nth(3)
                .and_then(|p| p.parse::<i32>().ok())
                .and_then(rustix::process::Pid::from_raw);
            let ok = target.is_some_and(|pid| {
                rustix::process::kill_process(pid, rustix::process::Signal::KILL).is_ok()
            });
            println!("ESCAPED kill: {ok}");
        }
        ("escape", "thread") => {
            let ok = std::thread::spawn(|| 1).join().is_ok();
            println!("ESCAPED thread: {ok}");
        }
        ("escape", "memory") => {
            // 3 GB: over the 2 GB address-space limit.
            let v: Vec<u8> = vec![1u8; 3 * 1024 * 1024 * 1024];
            println!("ESCAPED memory: {}", v.len());
        }
        ("allowed", _) => {
            // Things the decoder legitimately does must still work.
            let v: Vec<u64> = (0..1_000_000).collect();
            let sum: u64 = v.iter().sum();
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "OK {sum}");
        }
        // Busy loop with no syscalls (sleep itself is blocked by seccomp),
        // so only the parent's timeout can stop it.
        ("hang", _) => loop {
            std::hint::spin_loop();
        },
        ("flood", _) => {
            let mut out = std::io::stdout().lock();
            let chunk = vec![b'A'; 1 << 20];
            loop {
                if out.write_all(&chunk).is_err() {
                    std::process::exit(0);
                }
            }
        }
        _ => std::process::exit(2),
    }
}
