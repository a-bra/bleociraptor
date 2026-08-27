# harvester — firmware workspace

This directory is its own Cargo workspace, deliberately excluded from the repo
root (SPEC.md §6.1): its `.cargo/config.toml` will set the ESP target, and that
must never leak into host test builds.

**Populated at firmware bring-up (plan step 9).** Until then it holds only
`src/devices.rs.example` — the template for the gitignored secrets file — so the
secret-handling story is complete from the first commit.

To build (once populated): copy `src/devices.rs.example` to `src/devices.rs`,
fill in real values, then `cargo build --release` from this directory.
