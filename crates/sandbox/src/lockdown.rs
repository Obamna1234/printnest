//! Locks the current (worker) process down before it touches untrusted data.
//!
//! Linux (x86_64 / aarch64): resource limits + a seccomp-bpf allowlist with
//! argument checks. After [`lockdown`] returns, the process can only:
//! read from stdin (fd 0), write to stdout/stderr (fds 1-2), allocate
//! anonymous, non-executable memory, signal ITSELF with SIGABRT (Rust's
//! abort path), and exit. Opening files, using any inherited descriptor
//! other than 0-2, creating sockets, starting programs, mapping executable
//! or file-backed memory, signalling other processes and every other
//! syscall kill the process immediately.
//!
//! Other Unix (macOS): the same resource limits except the address-space
//! cap (macOS doesn't support or enforce it; the decoder's own 1 GB
//! allocation budget still applies), no syscall filter. The GUI
//! app bundle additionally runs under the App Sandbox, which its child
//! processes inherit; the separately shipped CLI does not.
//! Windows: process isolation plus the parent's timeout and output caps
//! only; a restricted token / job object is planned (SPEC section 11, M9).

/// How strongly the worker is confined on this platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confinement {
    /// Resource limits + seccomp syscall allowlist.
    Seccomp,
    /// Resource limits only.
    Rlimits,
    /// Separate process + parent-side limits only.
    ProcessOnly,
}

impl Confinement {
    /// Plain-language description for users.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Confinement::Seccomp => "in a locked-down process (no file, network or program access)",
            Confinement::Rlimits => {
                "in a separate, resource-limited process (no syscall filter on this OS)"
            }
            Confinement::ProcessOnly => {
                "in a separate process (time and size limits only on this OS)"
            }
        }
    }
}

/// The confinement [`lockdown`] applies on this build's platform.
#[must_use]
pub const fn expected() -> Confinement {
    if cfg!(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )) {
        Confinement::Seccomp
    } else if cfg!(unix) {
        Confinement::Rlimits
    } else {
        Confinement::ProcessOnly
    }
}

/// Why lockdown failed. The worker must exit rather than continue.
#[derive(Debug, thiserror::Error)]
pub enum LockdownError {
    #[error("could not set resource limit: {0}")]
    Rlimit(String),
    #[error("could not build or apply seccomp filter: {0}")]
    Seccomp(String),
}

/// Applies every confinement this platform supports.
///
/// # Errors
/// If any step fails; the caller must then exit without decoding.
pub fn lockdown() -> Result<Confinement, LockdownError> {
    #[cfg(unix)]
    rlimits::apply()?;
    imp::lockdown()
}

#[cfg(unix)]
mod rlimits {
    use super::LockdownError;
    use printnest_model::limits::{DECODE_MEMORY_BYTES, DECODE_TIMEOUT_SECS};
    use rustix::process::{Resource, Rlimit, setrlimit};

    fn limit(res: Resource, value: u64) -> Result<(), LockdownError> {
        setrlimit(
            res,
            Rlimit {
                current: Some(value),
                maximum: Some(value),
            },
        )
        .map_err(|e| LockdownError::Rlimit(format!("{res:?}: {e}")))
    }

    pub(super) fn apply() -> Result<(), LockdownError> {
        // macOS reserves far more address space than this at start-up and
        // rejects the limit; it never enforced RLIMIT_AS anyway.
        if cfg!(target_os = "macos") {
            let _ = DECODE_MEMORY_BYTES;
        } else {
            limit(Resource::As, DECODE_MEMORY_BYTES)?;
        }
        limit(Resource::Cpu, DECODE_TIMEOUT_SECS + 5)?;
        limit(Resource::Fsize, 0)?; // may not grow any file
        limit(Resource::Core, 0)?; // no core dumps containing photo data
        limit(Resource::Nofile, 16)?;
        Ok(())
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod imp {
    use super::{Confinement, LockdownError};
    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
        SeccompRule, TargetArch,
    };
    use std::collections::BTreeMap;

    fn err(e: impl std::fmt::Display) -> LockdownError {
        LockdownError::Seccomp(e.to_string())
    }

    /// One condition on argument `arg`.
    fn cond(
        arg: u8,
        len: SeccompCmpArgLen,
        op: SeccompCmpOp,
        value: u64,
    ) -> Result<SeccompCondition, LockdownError> {
        SeccompCondition::new(arg, len, op, value).map_err(err)
    }

    /// A rule list where ANY one rule (each an AND of its conditions) allows.
    fn any_of(rules: Vec<Vec<SeccompCondition>>) -> Result<Vec<SeccompRule>, LockdownError> {
        rules
            .into_iter()
            .map(|c| SeccompRule::new(c).map_err(err))
            .collect()
    }

    /// Syscalls with no arguments worth restricting.
    const UNCONDITIONAL: &[i64] = &[
        libc::SYS_close,
        libc::SYS_exit,
        libc::SYS_exit_group,
        libc::SYS_brk,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_futex,
        libc::SYS_rt_sigreturn,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigaction,
        libc::SYS_sigaltstack,
        libc::SYS_getrandom,
        libc::SYS_clock_gettime,
        libc::SYS_sched_yield,
        libc::SYS_gettid,
        libc::SYS_getpid,
    ];

    fn rules() -> Result<BTreeMap<i64, Vec<SeccompRule>>, LockdownError> {
        use SeccompCmpArgLen::{Dword, Qword};
        use SeccompCmpOp::{Eq, MaskedEq};

        let mut rules: BTreeMap<i64, Vec<SeccompRule>> =
            UNCONDITIONAL.iter().map(|&nr| (nr, Vec::new())).collect();

        // read: stdin only. Inherited descriptors (sockets, files) are useless.
        rules.insert(libc::SYS_read, any_of(vec![vec![cond(0, Dword, Eq, 0)?]])?);
        // write / writev: stdout and stderr only.
        for nr in [libc::SYS_write, libc::SYS_writev] {
            rules.insert(
                nr,
                any_of(vec![
                    vec![cond(0, Dword, Eq, 1)?],
                    vec![cond(0, Dword, Eq, 2)?],
                ])?,
            );
        }
        // mmap: anonymous (fd == -1), never executable. No mapping of files
        // through inherited descriptors.
        let no_exec = || cond(2, Dword, MaskedEq(libc::PROT_EXEC as u64), 0);
        rules.insert(
            libc::SYS_mmap,
            any_of(vec![vec![
                no_exec()?,
                cond(
                    3,
                    Dword,
                    MaskedEq(libc::MAP_ANONYMOUS as u64),
                    libc::MAP_ANONYMOUS as u64,
                )?,
                cond(4, Dword, Eq, u64::from(u32::MAX))?,
            ]])?,
        );
        rules.insert(libc::SYS_mprotect, any_of(vec![vec![no_exec()?]])?);
        // tgkill: only to this process, only SIGABRT (Rust's abort path).
        let pid = u64::from(std::process::id());
        rules.insert(
            libc::SYS_tgkill,
            any_of(vec![vec![
                cond(0, Qword, Eq, pid)?,
                cond(2, Dword, Eq, libc::SIGABRT as u64)?,
            ]])?,
        );
        Ok(rules)
    }

    pub(super) fn lockdown() -> Result<Confinement, LockdownError> {
        let arch = if cfg!(target_arch = "x86_64") {
            TargetArch::x86_64
        } else {
            TargetArch::aarch64
        };
        let filter = SeccompFilter::new(
            rules()?,
            SeccompAction::KillProcess,
            SeccompAction::Allow,
            arch,
        )
        .map_err(err)?;
        let program: BpfProgram = filter
            .try_into()
            .map_err(|e: seccompiler::BackendError| err(e))?;
        // Also sets no_new_privs, so the filter cannot be shed.
        seccompiler::apply_filter(&program).map_err(err)?;
        Ok(Confinement::Seccomp)
    }
}

#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
mod imp {
    use super::{Confinement, LockdownError, expected};

    #[allow(clippy::unnecessary_wraps)]
    pub(super) fn lockdown() -> Result<Confinement, LockdownError> {
        Ok(expected())
    }
}
