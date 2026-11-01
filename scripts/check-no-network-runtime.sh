#!/usr/bin/env bash
# Runtime half of the no-network guarantee (SPEC section 7, rule 1).
#
#   scripts/check-no-network-runtime.sh <path-to-printnest> <path-to-printnest-cli>
#
# Runs the CLI through every command, and the GUI for a few seconds on a
# virtual X display, all under strace. Fails if ANY process opens a socket
# that is not a local Unix socket (AF_UNIX), whichever library or syscall
# path it came from. Requires: strace, Xvfb (for the GUI part).
set -euo pipefail

gui="${1:?usage: $0 <printnest> <printnest-cli>}"
cli="${2:?usage: $0 <printnest> <printnest-cli>}"
work="$(mktemp -d)"
trap 'rm -rf "$work"; [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null || true' EXIT

trace() { strace -f -qq -e trace=socket,socketpair,connect -o "$work/trace.$1" "${@:2}"; }

echo "Tracing the CLI"
trace cat "$cli" catalogue > /dev/null
trace info "$cli" info book-20x20 --pages 40 > /dev/null
trace dpi "$cli" dpi canvas-60x40 4000 3000 > /dev/null
trace new "$cli" new print-6x4 --out "$work/p.json" > /dev/null
trace val "$cli" validate "$work/p.json" > /dev/null
# Photo decoding starts the sandboxed worker; strace -f follows it too.
fixture="$(cd "$(dirname "$0")/.." && pwd)/tests/fixtures/rotated-64x48.jpg"
trace photo "$cli" photo "$fixture" --product print-6x4 > /dev/null

if command -v Xvfb >/dev/null; then
  echo "Tracing the GUI for 8 seconds on a virtual display"
  Xvfb :97 -screen 0 1280x800x24 -nolisten tcp > /dev/null 2>&1 &
  xvfb_pid=$!
  sleep 1
  # Local display only: a remote DISPLAY would be the user's own choice.
  DISPLAY=:97 WAYLAND_DISPLAY='' LIBGL_ALWAYS_SOFTWARE=1 \
    timeout --signal=TERM 8 strace -f -qq -e trace=socket,socketpair,connect \
      -o "$work/trace.gui" "$gui" > /dev/null 2>&1 || true
  [[ -s "$work/trace.gui" ]] || { echo "::error::GUI trace is empty; the app did not start"; exit 1; }
else
  echo "::warning::Xvfb not installed; GUI not traced"
fi

# Every socket()/socketpair() must be AF_UNIX; connect() must target a path.
bad="$(cat "$work"/trace.* \
  | grep -E '(socket|socketpair)\(' | grep -v 'AF_UNIX' || true)"
bad+="$(cat "$work"/trace.* \
  | grep -E 'connect\(' | grep -E 'AF_INET|AF_INET6|AF_PACKET|AF_NETLINK|sin_port|sin6_port' || true)"

echo "Socket calls seen:"
cat "$work"/trace.* | grep -E 'socket|connect' | sed -E 's/^[0-9]+ +//' | cut -c1-110 | sort | uniq -c || true

if [[ -n "$bad" ]]; then
  echo "::error::non-local network activity:"
  printf '%s\n' "$bad"
  exit 1
fi
echo "OK: only local Unix sockets were used."
