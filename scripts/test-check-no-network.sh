#!/usr/bin/env bash
# Self-test for check-no-network.sh: builds small "canary" programs that DO
# use the network in different ways and asserts the scanner rejects every
# one, plus a clean program it must accept. Proves the scanner is not
# silently passing. Canaries are throwaway test programs, never shipped.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

write() { printf '%s\n' "$2" > "$work/$1.rs"; }

write tcp 'fn main() { let _ = std::net::TcpStream::connect("example.invalid:80"); }'
write udp 'fn main() { let _ = std::net::UdpSocket::bind("0.0.0.0:0"); }'
write dns 'use std::net::ToSocketAddrs; fn main() { let _ = "example.invalid:80".to_socket_addrs(); }'
write unix 'fn main() { let _ = std::os::unix::net::UnixStream::connect("/tmp/not-wayland"); }'
# A look-alike of the one allowlisted function, at a different module path.
write forged '
mod wayland_client { pub mod conn { pub struct Connection;
  impl Connection { #[inline(never)] pub fn connect_to_env() {
    let _ = std::os::unix::net::UnixStream::connect("/tmp/x"); } } } }
fn main() { wayland_client::conn::Connection::connect_to_env(); }'
# Raw syscalls with no libc import (what crates like rustix do). x86_64 only.
write rawsys '
#[inline(never)]
fn raw_socket() -> isize {
  let ret: isize;
  unsafe { core::arch::asm!("syscall", inlateout("rax") 41isize => ret,
    in("rdi") 2isize, in("rsi") 1isize, in("rdx") 0isize,
    lateout("rcx") _, lateout("r11") _, options(nostack)); }
  ret
}
#[inline(never)]
fn raw_connect(fd: isize) -> isize {
  let ret: isize;
  unsafe { core::arch::asm!("syscall", inlateout("rax") 42isize => ret,
    in("rdi") fd, in("rsi") 0isize, in("rdx") 0isize,
    lateout("rcx") _, lateout("r11") _, options(nostack)); }
  ret
}
fn main() { let fd = raw_socket(); std::process::exit(raw_connect(fd) as i32); }'
write clean 'fn main() { let v: Vec<u32> = (1..10).collect(); std::process::exit((v.len() % 2) as i32); }'

for name in tcp udp dns unix forged rawsys clean; do
  rustc -O -C strip=none --edition 2021 -o "$work/$name" "$work/$name.rs"
done

status=0
for name in tcp udp dns unix forged rawsys; do
  if CARGO_LOCK=/nonexistent "$here/check-no-network.sh" "$work/$name" > "$work/$name.log" 2>&1; then
    echo "FAIL: scanner accepted the '$name' canary"
    cat "$work/$name.log"
    status=1
  else
    echo "ok: '$name' canary rejected"
  fi
done
if CARGO_LOCK=/nonexistent "$here/check-no-network.sh" "$work/clean" > "$work/clean.log" 2>&1; then
  echo "ok: clean program accepted"
else
  echo "FAIL: scanner rejected a clean program"
  cat "$work/clean.log"
  status=1
fi
exit $status
