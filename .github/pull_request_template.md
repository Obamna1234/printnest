## What and why

<!-- What does this change, and which issue does it close? -->
Closes #

## Security impact

<!-- Tick every box that applies, then explain any ticked box below. -->
- [ ] Touches file reading/parsing or paths
- [ ] Adds or updates a dependency (justify it here)
- [ ] Changes `.github/`, `deny.toml`, `supply-chain/`, `packaging/`, limits or sandbox code (title must start with `[SECURITY]`)
- [ ] None of the above

## How it was tested

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] `cargo deny check`
- [ ] New tests added for new behaviour
