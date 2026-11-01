#!/usr/bin/env bash
# Fails if a Linux x86_64 binary contains network code (SPEC section 7, rule 1).
#
#   scripts/check-no-network.sh <binary>...
#
# Scan UNSTRIPPED builds (CARGO_PROFILE_RELEASE_STRIP=none) so callers can be
# identified; the release workflow scans first and strips afterwards. This is
# a static check. CI also runs the binaries under strace
# (scripts/check-no-network-runtime.sh), which catches what statics miss.
#
# Layers:
#  1. Always-banned libc imports: DNS, servers (bind/listen/accept),
#     datagram I/O.
#  2. libc `socket`/`connect`: allowed only if EVERY calling function is the
#     exact Wayland compositor connector from the crates.io `wayland-client`
#     crate (local Unix socket). Matched on the full mangled symbol, and the
#     crate's source is checked in Cargo.lock, so a look-alike name fails.
#  3. Raw socket syscalls made without libc (e.g. via rustix), found in the
#     machine code by scripts/raw-socket-syscalls.awk. Always banned.
#  4. Rust std::net symbols (TcpStream, UdpSocket, ToSocketAddrs, ...).
#
# Known limits (see SPEC section 7): code loaded at run time with dlopen
# (e.g. the system's libX11, which honours a remote DISPLAY) is outside any
# static scan; the runtime strace check covers PrintNest's own behaviour.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"

if [[ $# -eq 0 ]]; then
  echo "usage: $0 <binary>..." >&2
  exit 2
fi
for tool in nm objdump awk; do
  command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }
done

always_banned='^(getaddrinfo|freeaddrinfo|gai_strerror|getnameinfo|gethostbyname|gethostbyname2|gethostbyname_r|gethostbyaddr|res_init|res_query|res_search|res_nquery|res_nsearch|bind|listen|accept|accept4|sendto|recvfrom|getpeername)(@.*)?$'
socket_calls='^(socket|connect|socketpair)(@.*)?$'
# The ONE allowed caller, as its exact v0-mangled symbol: crate root
# `wayland_client` (any disambiguator hash), module `conn`, type
# `Connection`, method `connect_to_env`. A function at any other path, such
# as evil::wayland_client::..., does not match.
allowed_caller='^<_RNvMNtCs[0-9A-Za-z]+_14wayland_client4connNtB[0-9A-Za-z]*_10Connection14connect_to_env>:$'
banned_rust='std::net::(tcp|udp|socket_addr)|TcpStream|TcpListener|UdpSocket|ToSocketAddrs|lookup_host'

indent() { while IFS= read -r line; do printf '    %s\n' "$line"; done; }

fail=0

# The allowlisted crate must be the real one from crates.io.
lock="${CARGO_LOCK:-Cargo.lock}"
if [[ -f "$lock" ]]; then
  bad_src="$(awk '
      /^\[\[package\]\]/ {name=""; src=""}
      /^name = / {name=$3}
      /^source = / {src=$3}
      /^$/ && name=="\"wayland-client\"" && src!="\"registry+https://github.com/rust-lang/crates.io-index\"" {print src; name=""}
      END { if (name=="\"wayland-client\"" && src!="\"registry+https://github.com/rust-lang/crates.io-index\"") print src }
    ' "$lock")"
  if [[ -n "$bad_src" ]]; then
    echo "::error::wayland-client in $lock does not come from crates.io ($bad_src)"
    fail=1
  fi
fi

for bin in "$@"; do
  if [[ ! -f "$bin" ]]; then
    echo "::error::$bin not found"
    fail=1
    continue
  fi
  echo "Scanning $bin"
  imports="$(nm -D --undefined-only "$bin" 2>/dev/null | awk '{print $NF}' || true)"
  symbol_count="$(nm "$bin" 2>/dev/null | wc -l || true)"
  if [[ "$symbol_count" -lt 100 ]]; then
    echo "::error::$bin is stripped; scan an unstripped build (CARGO_PROFILE_RELEASE_STRIP=none)"
    fail=1
    continue
  fi
  disasm="$(objdump -d --no-show-raw-insn "$bin")"

  # 1. Always banned.
  hits="$(grep -E "$always_banned" <<< "$imports" || true)"
  if [[ -n "$hits" ]]; then
    echo "::error::$bin imports banned network functions:"
    indent <<< "$hits"
    fail=1
  else
    echo "  1. no DNS, server or datagram imports"
  fi

  # 2. socket/connect: every caller must be the exact allowlisted symbol.
  sock="$(grep -E "$socket_calls" <<< "$imports" || true)"
  if [[ -n "$sock" ]]; then
    relocs="$(objdump -R "$bin")"
    while IFS= read -r sym; do
      name="${sym%%@*}"
      got="$(awk -v s="$sym" '$3==s {print $1}' <<< "$relocs" | sed 's/^0*//')"
      callers="$(awk -v a="$got" -v p="<$name@plt>" '
          /^[0-9a-f]+ </ {fn=$2}
          (a != "" && index($0, a)) || index($0, p) {print fn}' <<< "$disasm" | sort -u)"
      if [[ -z "$callers" ]]; then
        echo "::error::$bin imports $name but no caller could be identified"
        fail=1
        continue
      fi
      bad="$(grep -vE "$allowed_caller" <<< "$callers" || true)"
      if [[ -n "$bad" ]]; then
        echo "::error::$bin calls $name from functions that are not allowlisted:"
        indent <<< "$bad"
        fail=1
      else
        echo "  2. $name: only called by wayland_client::conn::Connection::connect_to_env"
      fi
    done <<< "$sock"
  else
    echo "  2. no socket/connect imports"
  fi

  # 3. Raw socket syscalls (no libc involved).
  raw="$(awk -f "$here/raw-socket-syscalls.awk" <<< "$disasm" | sort -u || true)"
  if [[ -n "$raw" ]]; then
    echo "::error::$bin makes raw socket syscalls:"
    indent <<< "$raw"
    fail=1
  else
    echo "  3. no raw socket/connect/bind/listen/accept syscalls"
  fi

  # 4. Rust networking types.
  rust_hits="$(nm -C "$bin" | grep -oE "$banned_rust" | sort | uniq -c | sort -rn || true)"
  if [[ -n "$rust_hits" ]]; then
    echo "::error::$bin contains Rust networking code:"
    indent <<< "$rust_hits"
    fail=1
  else
    echo "  4. no std::net types (TcpStream, UdpSocket, ToSocketAddrs, ...)"
  fi
done

if [[ $fail -ne 0 ]]; then
  echo
  echo "Network code found. PrintNest must not be able to reach the network."
  echo "Trace it with: nm -C <binary> | grep -E 'TcpStream|getaddrinfo'"
  echo "then: cargo tree -i <crate> --target x86_64-unknown-linux-gnu"
  exit 1
fi
echo "OK: no network code found."
