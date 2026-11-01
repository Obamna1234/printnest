//! Sandboxed photo decoding (SPEC section 7, rule 3).
//!
//! Untrusted photos are never decoded in the app's own process. The app
//! ([`parent`]) reads the user-picked file, starts a short-lived copy of
//! itself as a worker, sends the bytes over a pipe and validates the answer.
//! The worker ([`worker`]) first locks itself down ([`lockdown`]): on Linux
//! with resource limits and a seccomp filter that kills it if it tries to
//! open a file, create a socket, start a program or map executable memory.
//! A decoder bug or malicious photo is therefore contained in a process that
//! can do nothing but compute and reply, and is killed after a timeout.

#![forbid(unsafe_code)]

pub mod lockdown;
pub mod parent;
pub mod protocol;
pub mod worker;

pub use lockdown::Confinement;
pub use parent::{SandboxError, WORKER_ARG, Worker, read_capped};

/// Call first thing in `main` of any binary that can act as a worker:
/// if the process was started as a worker, this serves one request and
/// exits; otherwise it returns immediately.
pub fn maybe_run_worker() {
    let mut args = std::env::args_os().skip(1);
    if let (Some(first), None) = (args.next(), args.next())
        && first == WORKER_ARG
    {
        worker::run();
    }
}
