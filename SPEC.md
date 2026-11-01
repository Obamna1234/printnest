# PrintNest — Build Spec

Source of truth for what PrintNest is and how it must be built. AGENTS.md
holds the short rules; this file holds the reasons and details. If an issue
conflicts with this file, stop and comment on the issue.

## 1. Overview

PrintNest is a free, open-source, offline-first desktop app for Windows,
macOS and Linux for designing photo books, framed prints, canvases,
calendars, cards and photo gifts, then exporting print-ready files to print
at home or take to a kiosk (Kmart, Big W, Officeworks, chemists) or a print
shop. Built in Rust, on GitHub, by a coding agent with a human reviewer.
Security is release-blocking.

**Goals**

- Cover the common photo-product families with accurate real-world sizes,
  bleed and safe zones.
- Output that works on home printers, retail kiosks and print shops with no
  extra software.
- Never collect or transmit user photos or data. Works fully offline.
- Hard to turn into malware: hostile files can't take over the app, the app
  can't reach beyond folders the user picks, every release is verifiably
  built from public source.

**Non-goals (v1)**: ordering, payments, accounts, cloud sync; direct upload
to any retailer; server-side AI; copying any company's templates, artwork,
branding or file formats.

## 2. Boundaries

- Clean-room only. Never decompile, disassemble or reverse-engineer any
  closed-source photo-book software, file format or template.
- No company names, logos, colours or product names anywhere (app, repo,
  listings). Physical sizes (A4, 8×10 in) are facts and fine to use.
- Bundled assets are original or openly licensed (CC0, OFL, MIT/Apache),
  recorded in ASSETS.md.
- Code must be licence-compatible with MIT OR Apache-2.0 (no GPL in the
  shipped binary).
- Units: millimetres internally; UI shows mm/cm or inches. Default locale
  en-AU. Sizes live in `catalogue/*.toml`, never in code.
- Releases are **not** OS code-signed (decided). See section 8.

## 3. Product catalogue

Each product is data: trim size, bleed, safe margin, page rules, spine
rules, wrap depth, suitable export profiles. Starter families:

| Family | Starter sizes | Geometry | Output |
| --- | --- | --- | --- |
| Photo prints | 6×4, 7×5, 8×6, 10×8, 12×8 in; 5×5, 8×8 in | trim, 3 mm bleed, aspect crop | kiosk JPEG, home |
| Posters | A4–A1, 16×12, 20×16, 24×36 in | bleed, tiling for home | PDF, kiosk JPEG, home tiled |
| Framed prints | A4, A3, 8×10, 11×14, 12×16 in + mats | print vs mat window vs rebate | kiosk JPEG + mat template |
| Canvas | 20×20 cm to 76×102 cm | wrap margin 18–38 mm, edge modes | print shop PDF |
| Photo books | 20×20, A4 portrait/landscape, 30×30 cm; 20–100+ pages | spreads, gutter, spine from pages × paper thickness | interior + cover PDF |
| Calendars | A4 P/L, A3, 21×21 & 20×10 desk, 30×30 double | 12/13 months, AU holidays data, binding margin | PDF, home duplex |
| Cards | 5×7 in, A5, A6, DL | fold line, panels, envelope | home duplex, PDF |
| Gifts | mug, jigsaw, magnet, coaster, mouse pad, cushion, tote, ornament | shape mask, wrap area | PDF/PNG template |

Paper presets carry a thickness (mm) used for spine maths: plain 80 gsm,
matte 170–230 gsm, gloss 200–260 gsm, card 250–350 gsm, canvas.
A preset is `verified = false` until checked against a real retailer.

## 4. Architecture

Two processes. The **main process** (egui UI → engine → layout → render →
export) never parses untrusted bytes. A **sandboxed worker process** decodes
images and project archives under hard limits and returns pixels/validated
structures over IPC. Only exported files leave the app; there is no network.

Reuse the MIT/Apache-2.0 ArtCraft "Crafting Apps" where they fit
(DesignCraft for layout/text/PDF, PhotoCraft for adjustments); pin exact
commits and cargo-vet them like any dependency.

Repo layout:

```
apps/printnest          desktop app (egui)
apps/printnest-cli      headless validate/render/export (tests, CI)
crates/model            units, catalogue, project schema   [built]
crates/layout           pages, frames, auto-layout          [M3/M5]
crates/render           rasteriser                          [M3]
crates/imaging          decode + adjustments (in worker)    [M2]
crates/export           JPEG, PDF/X, tiling, templates      [M4]
crates/sandbox          worker process, IPC, limits         [M2]
catalogue/              product data (TOML)
assets/                 original/CC0 assets + ASSETS.md
fuzz/                   cargo-fuzz targets                  [M2]
tests/golden/           sample projects + expected renders  [M4]
supply-chain/           cargo-vet audits
```

## 5. Editor features

- New-project wizard: product → size → paper/cover → blank / layout /
  auto-fill from a folder.
- Bleed (red), trim (black), safe (blue) overlays; spine and wrap areas.
- Page panel with per-product page-count rules.
- Original layout library; local auto-layout by EXIF date (no network, no
  model download).
- Photo frames with pan/zoom, rotate, flip, borders; non-destructive basic
  adjustments; red-eye; text with bundled open fonts; stickers (CC0).
- Snapping, guides, rulers, layers, group/lock, unlimited undo.
- Effective-DPI badge: green ≥ 250, amber 150–249, red < 150.
- Preflight before export; soft-proof toggle.
- Keyboard-accessible, screen-reader labels, dark/high-contrast themes,
  autosave every 60 s, crash recovery, last 10 backups, translatable strings.

## 6. Export

| Profile | Output | Key settings |
| --- | --- | --- |
| Kiosk / photo lab | one JPEG per page, sRGB, 300 ppi, q92, ≤ 10,000 px side, simple names | exact print aspect so nothing auto-crops |
| Print shop | PDF/X-4 (X-1a option), embedded fonts, 3 mm bleed + crop marks; books: interior + cover | spine computed, profile embedded |
| Home | system print dialog or A4/Letter PDF | borderless, duplex guide, poster tiling with overlap marks |
| DIY guides | PDF templates (cover wrap, spine score, canvas stretcher, mat window, card fold, mug wrap) | 100% scale with 10 cm calibration ruler |
| Archive | zip of project + originals | validated on re-open |

Every export: strip EXIF/GPS by default; write only to a user-picked
folder; write `PRINT-ME.txt` beside kiosk files; show "How to print this";
deterministic (same input → identical bytes).

## 7. Security and privacy

Promise: **the app opens files, edits them and writes files; it has no other
way to touch the user's data or the outside world.**

| Threat | Controls |
| --- | --- |
| Malicious image/project file | memory-safe Rust decoders, limits, sandboxed worker, fuzzing |
| Data exfiltration | no network code, OS sandbox, user-picked paths only, dependency bans |
| Fake/tampered release | reproducible builds, Sigstore signatures, provenance attestations, checksums, no auto-update |
| Compromised dependency | Cargo.lock, cargo-deny, cargo-audit, cargo-vet, pinned action SHAs |
| Compromised repo/CI | branch protection, human review, least-privilege tokens, no secrets on PRs |
| Malicious template | templates are declarative data only; no scripting/plugins |
| Privacy leak in output | metadata stripped on export |
| Local tampering | strict schema validation on every load |

Rules:

1. **No network.** No HTTP, sockets, DNS, telemetry, crash upload, CDN fonts
   or update checks. Enforced in layers, because no single check is enough:
   - `deny.toml` bans known network crates (cheap, but name-based only);
   - `scripts/check-no-network.sh` statically scans Linux release binaries:
     banned libc imports (DNS, servers, datagrams), `socket`/`connect`
     allowed only from the exact `wayland_client` compositor connector
     (crate source checked in Cargo.lock), raw socket syscalls in the
     machine code (catches rustix-style direct syscalls), and `std::net`
     symbols. Its self-test proves six kinds of canary are rejected;
   - `scripts/check-no-network-runtime.sh` runs the real binaries under
     strace and fails on any socket that is not a local Unix socket.
   Known limits: libraries loaded at run time with `dlopen` (the system's
   libX11/libGL) are outside the static scan; X11 itself will connect over
   TCP if the user sets `DISPLAY` to a remote host, which is the user's own
   configuration. macOS/Windows binaries get an import report in each
   release (report-only until verified on real runs).
2. **Least file access.** Only user-picked files/folders and the app's own
   config dir. Project paths are relative; `..`, absolute, symlink, UNC or
   device paths are rejected.
3. **Hostile-file handling.** Photos are decoded only in a worker process
   (`crates/sandbox`): the app reads the user-picked file (size-capped),
   starts a copy of itself with one fixed argument and a cleared
   environment, and sends the bytes over a pipe. The worker locks down
   before reading any input. On Linux: 2 GB address-space cap, CPU, file-size
   (0), core-dump (0) and open-file limits, and a seccomp allowlist with
   argument checks: read only from fd 0, write only to fds 1-2, mmap only
   anonymous and never executable, mprotect never executable, tgkill only to
   itself with SIGABRT, plus exit, futex and a few signal/time calls. Any
   other syscall or argument (open, socket, kill, clone/fork, execve, use of
   inherited descriptors, x32/i386 ABIs) kills it. The parent enforces a
   10 s timeout and an output cap, streams the photo without copying it,
   refuses non-regular files without blocking, and validates every
   field of the answer. Decoding (`crates/imaging`): format by magic bytes
   (JPEG, PNG, WebP, TIFF only, pure-Rust decoders), file ≤ 500 MB, ≤ 100 MP,
   ≤ 60,000 px per edge, 1 GB decoder allocation budget, preview shrunk
   before EXIF orientation is applied (one full-size buffer at most). Other limits: 2,000 photos/project, ≤ 10,000 zip entries,
   uncompressed ≤ 20× compressed (archives arrive with M4).
   Platform gaps (the CLI says which level applies): macOS gets the same
   resource limits but no syscall filter; the GUI bundle adds the App
   Sandbox, which its worker inherits, but the separately shipped CLI does
   not. Windows has process isolation, timeout and caps but no restricted
   token/job object yet (planned for M9). HEIC/RAW are not supported (no
   memory-safe decoder).
4. **Data is data.** Project = zip + JSON manifest, versioned schema, unknown
   fields rejected. No scripts, macros, HTML, remote URLs. SVG sanitised to
   shapes/paths.
5. **No unsafe** in first-party crates (`forbid`).
6. **No panics on input**: `unwrap`/`expect`/`panic`/indexing denied.
7. **No secrets**, local-only logs without file contents.
8. **Privacy defaults**: strip metadata, no usernames/machine IDs in files,
   "Clear all app data".
9. **Safe output**: PDFs without JS/forms/attachments/launch actions/links.
10. **No shelling out** and no command lines built from user data. The one
    process the app starts is its own decoder worker (fixed argument,
    cleared environment, data over stdin only).

OS hardening: macOS App Sandbox (`user-selected.read-write` only, no
network), Hardened Runtime, ad-hoc signed; Windows unsigned portable zip
(MSI from M4), ASLR/CFG/DEP, static CRT, per-user; Linux tarball now,
Flatpak with portals and `--unshare=network` plus AppImage/.deb from M4.

**Linux screen readers (decision, M0).** The Linux accessibility bridge
(AT-SPI over D-Bus, via `zbus`) always compiles in TCP transport code, which
breaks rule 1. Default Linux builds therefore ship without it, and
the network checks confirm the binary's only socket is the local Wayland
(or X11) display connection. Windows and macOS keep screen-reader support (OS APIs, no
network code). Linux users who need a screen reader can build with
`--features linux-screen-reader`; revisit when the Flatpak (network
unshared at OS level) lands in M4.

Vulnerabilities: SECURITY.md, GitHub private reporting, 72 h acknowledgement,
30 days to fix high/critical, advisories for real issues.

## 8. Supply chain and releases

- `Cargo.lock` committed; `--locked` everywhere; pinned toolchain.
- cargo-deny (licences, network bans, crates.io only), cargo-audit on every
  PR and nightly, cargo-vet for every new/updated dependency (audits
  imported from Mozilla, Google, Bytecode Alliance and Embark; everything
  else is an explicit exemption in `supply-chain/config.toml`).
- Dependabot weekly; humans approve.
- Rulesets on `main` (scripts/setup-repo.sh): (A) green checks, signed
  commits, linear history, no force-push or deletion, with NO bypass;
  (B) PR + 1 code-owner approval, which a sole admin may bypass for their
  own PRs only; (C) only admins can create, move or delete `v*` tags.
  CODEOWNERS covers `.github/`, `.cargo/`, `build.rs`, `deny.toml`,
  `supply-chain/`, scripts, packaging, limits and path validation.
- Actions pinned to full SHAs; `permissions: read-all` by default;
  no `pull_request_target` with PR checkout; harden-runner on build jobs.
- Releases need a signed, annotated tag verified by GitHub; the release
  workflow re-runs fmt, clippy, tests, cargo-deny, cargo-audit and cargo-vet
  on the tagged commit before building.
- Releases: reproducible build (built twice, hashes compared), installers,
  `SHA256SUMS`, Sigstore/cosign signatures, GitHub build-provenance
  attestations, CycloneDX SBOM.
- **No paid OS code signing.** macOS shows a first-launch block, Windows
  shows SmartScreen. README and release notes explain how to verify the
  download first, then open it (macOS: System Settings → Privacy & Security
  → Open Anyway; Windows: More info → Run anyway).

## 9. GitHub automation

1. Issue opened from a template (goal, acceptance criteria, security notes).
2. Label `agent:build` on an issue runs the coding agent (its own branch +
   PR only; outbound network blocked except an allowlist; protected paths
   off-limits; never runs on pull requests, so fork code is never checked
   out beside secrets).
3. Agent reads AGENTS.md, SPEC.md, the issue; writes code + tests; opens a
   draft PR.
4. CI runs; agent iterates (max 5 attempts) or labels `needs-human`.
5. A separate read-only review job checks the diff against AGENTS.md. It
   checks out the base branch and reads the PR only as a diff.
6. A human approves and squash-merges.
7. Release: signed tag → `release` environment approval → build, sign,
   attest → draft release → human publishes.

## 10. Quality gates

fmt + clippy (`-D warnings`); tests on all three OSes; geometry tests for
every catalogue product; golden renders (≤ 0.1% pixel diff) and veraPDF
PDF/X validation [M4+]; cargo-deny/audit/vet clean; no new `unsafe`;
fuzzing with regression corpus [M2+]; hostile-file corpus; sandbox escape
tests; privacy test (no metadata, zero network calls); reproducible build;
a human test print per minor release.

## 11. Milestones

- **M0 Foundations** — repo, policies, CI, security and release workflows,
  hello window. [done in this repo]
- **M1 Model and catalogue** — units, catalogue TOML, project schema, CLI
  validate. [done in this repo]
- **M2 Safe import** — sandbox worker, decoder limits, fuzzing, hostile corpus.
  [done in this repo; `printnest-cli photo` decodes through the sandbox]
- **M3 Single-page editor** — canvas, frames, text, guides, undo, DPI badges.
- **M4 Export v1** — kiosk JPEG, PDF/X, home print, preflight, golden tests.
  First public beta.
- **M5 Photo books** — spreads, covers, spine, layouts, auto-layout.
- **M6 Wall decor** — framed prints + mats, canvas wrap templates.
- **M7 Calendars and cards.**
- **M8 Gifts.**
- **M9 Polish and 1.0** — adjustments, soft-proof, accessibility,
  translations, external security review.
