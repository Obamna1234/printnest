# PrintNest

**Free, offline, open-source designer for photo books, prints, frames,
canvases, calendars, cards and photo gifts.** Design on Windows, macOS or
Linux, then print at home or take the file to a photo kiosk or print shop.

> Status: **early development** (milestones 0–3 of 9). You can pick a
> product (poster, print, canvas, card…), lay out photos and text on it with
> crop, rotate, snapping, undo and print-sharpness badges, and save/open
> projects. Exporting print files arrives in milestone 4. See
> [SPEC.md](SPEC.md) for the plan.

## Privacy and security promise

PrintNest opens files, edits them and writes files. **It has no other way to
touch your data or the outside world:**

- **No network code.** No accounts, telemetry, analytics or auto-updater.
  The build fails if a network library enters the dependency tree; release
  binaries are scanned for network code (down to raw system calls) and run
  under a tracer, and the only socket allowed is the local connection to
  your screen's display server.
- **Your photos never leave your computer.** Exports strip GPS and camera
  metadata by default (from milestone 4).
- **Hostile files can't take over the app.** Every file is size-limited and
  strictly validated, and paths inside project files can't escape the
  project folder. Photos are decoded in a separate, locked-down process: on
  Linux the kernel kills it if it even tries to open a file, make a network
  connection or start a program.
- **Memory-safe Rust** with `unsafe` code forbidden in PrintNest's own crates.
- **Every release is verifiable**: reproducible build, checksums, Sigstore
  signatures, GitHub build provenance and a full SBOM.

Found a problem? Please report it privately: see [SECURITY.md](SECURITY.md).

## Download and verify

Get builds only from this repo's
[Releases](https://github.com/OWNER/printnest/releases) page.

PrintNest is **not** OS code-signed (no paid certificates), so verify the
download first, then open it:

```sh
sha256sum -c SHA256SUMS --ignore-missing
gh attestation verify <downloaded-file> --repo OWNER/printnest
```

- **macOS:** open the DMG, drag PrintNest to Applications, try to open it once,
  then System Settings → Privacy & Security → **Open Anyway**.
- **Windows:** unzip and run `printnest.exe`; on the SmartScreen warning choose
  **More info → Run anyway**.
- **Linux:** `tar -xzf printnest-*.tar.gz && ./printnest-*/printnest`

## Products

8 families, 46 starter sizes, all defined as data in
[`catalogue/products.toml`](catalogue/products.toml): photo prints, posters,
framed prints (with mats), canvas (with wrap), photo books (soft, hard and
layflat covers with automatic spine width), calendars, folded cards and gifts
(mugs, puzzles, magnets, coasters, mouse pads, cushions, totes, ornaments).

Sizes are marked `verified = false` until someone checks them against a real
retailer. Corrections and new sizes are welcome as PRs.

## Build from source

You need [Rust](https://rustup.rs) (the exact version is pinned in
`rust-toolchain.toml` and installs automatically). On Linux you also need:
`libxkbcommon-dev libwayland-dev libgl1-mesa-dev libx11-dev`.

```sh
cargo run --release -p printnest            # desktop app
cargo run --release -p printnest-cli -- catalogue
cargo run --release -p printnest-cli -- info book-20x20 --pages 40
cargo run --release -p printnest-cli -- dpi canvas-60x40 4000 3000
cargo run --release -p printnest-cli -- photo my-photo.jpg --product poster-a3
```

Before opening a PR:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check
```

## How this project is built

PrintNest is built mostly by an AI coding agent working through GitHub, with
a human approving every merge and release:

1. A maintainer opens an issue from the **Build task** template and adds the
   `agent:build` label.
2. The agent follows [AGENTS.md](AGENTS.md) and [SPEC.md](SPEC.md), writes code
   and tests on its own branch and opens a PR.
3. CI tests on all three OSes; security checks run on every PR; a separate
   read-only AI reviewer comments on the diff.
4. A human approves and merges. Releases need a signed tag **and** a human
   approval before anything is published.

## Repository layout

```
apps/printnest        desktop app
apps/printnest-cli    command-line tool (used by CI)
crates/model          units, limits, safe paths, catalogue, project format
crates/imaging        JPEG/PNG/WebP/TIFF decoding with hard limits
crates/sandbox        locked-down decoder worker process (seccomp on Linux)
crates/layout         crop/ppi maths, hit-testing, snapping, undo
crates/document       project folders: safe load/save and photo import
fuzz/                 cargo-fuzz targets (run in CI on nightly)
catalogue/            product sizes (data)
packaging/            macOS sandbox entitlements and bundle info
scripts/              repo setup, network scanners and their self-tests
supply-chain/         cargo-vet audits and exemptions
.github/              CI, security, release, agent and review workflows
```

## Licence

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at
your option. Bundled third-party assets are listed in [ASSETS.md](ASSETS.md).

PrintNest is an independent project. It is not affiliated with any photo
printing company or retailer.
