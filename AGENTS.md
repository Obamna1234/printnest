# AGENTS.md — rules for any AI agent working in this repo

Read SPEC.md before every task. If an issue conflicts with SPEC.md or this
file, stop and comment on the issue instead of guessing.

## Never

- Add networking of any kind (HTTP, sockets, DNS, telemetry, update checks,
  remote fonts/images). `deny.toml` bans network crates; do not edit it to
  get around that.
- Add `unsafe` code to first-party crates (`unsafe_code = "forbid"`).
- Read or write files outside paths the user chose or the app's own
  config/project directories.
- Execute, eval or interpret anything loaded from a file (no scripts, macros,
  plugins, expressions).
- Launch other processes or build shell commands from user data. (The only
  exception is the existing decoder worker in `crates/sandbox`, which
  starts the app's own executable with a fixed argument. Don't add others.)
- Decompile or copy closed-source software, file formats, templates or assets
  from any company. Do not use any company's name, logo or product names.
- Add a dependency without justifying it in the PR; never add a network,
  OpenSSL or GPL crate.
- Edit `.github/`, `deny.toml`, `supply-chain/`, `rust-toolchain.toml`,
  entitlements or sandbox config unless the issue explicitly asks; flag such
  PRs with `[SECURITY]` in the title.
- Commit secrets, real user photos or large binaries.
- Weaken, skip or delete a test, lint, fuzz target or CI gate to make a build
  pass.
- Use `unwrap`, `expect`, `panic!` or slice indexing in code that handles file
  input (clippy denies these workspace-wide).

## Always

- Decode photos only through `printnest_sandbox::Worker` (never call
  `printnest_imaging` from app code directly), within the limits in
  `crates/model/src/limits.rs`.
- Add a cargo-fuzz target (`fuzz/`) for any new parser.
- Return errors, never panic, on bad input.
- Add tests with every change: unit tests, a golden render for visual changes
  (from M4), a fuzz target for any new parser (from M2).
- Keep product sizes in `catalogue/*.toml`, never hard-coded.
- Record every bundled asset with source and licence in ASSETS.md.
- Keep PRs small (≤ 400 changed lines excluding tests/data) and link the issue.
- Run `cargo fmt --all`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
  `cargo test --workspace --locked` and `cargo deny check` before pushing.
- PR description: what changed, why, security impact, how it was tested.

## Reuse

- Prefer the MIT/Apache-2.0 ArtCraft Crafting Apps (DesignCraft, PhotoCraft)
  over writing new code where they fit; keep licence headers, credit them in
  README, pin exact versions/commits and add cargo-vet entries.
