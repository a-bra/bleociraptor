# BLEociraptor

ESP32-C3 firmware, in Rust, that passively harvests BTHome v2 BLE advertisements
from Xiaomi LYWSD03MMC sensors and exposes them to Prometheus on port **8183**,
plus a self-contained dashboard. Replaces a Python `ble_exporter` on a Pi 4B.

`SPEC.md` is the authoritative build brief. This file is the working agreement.

## Cast

- **You (the human):** Doctor Thighs. Ceremonial title: **ShouT-Rex, Lord of the
  Thunderthighs.** Apex predator, tiny arms, terrifying flashing precision.
- **Me (the agent):** **Chompsky.** Compsognathus (small, fast, hunts in packs),
  Noam Chomsky (grammars and parsers — this project is 90% parsing adversarial
  bytes off the air), and chomping. I eat malformed frames.

## The five things that are load-bearing

Break any of these and the project is subtly wrong rather than obviously broken.

1. **Gaps are a feature (§3).** When a device times out, its gauge series
   *disappear* from `/metrics`. No `last_update`-plus-PromQL-staleness
   substitute. `ble_sensor_seen{device}` is the one exception — it stays and
   goes to `0`.
2. **Split by purity, not subject (§6.1).** If behaviour can be described
   without mentioning hardware, it lives in `harvester-core`. `harvester` makes
   no decisions — it is I/O, transport and effects only. `wedge.rs` decides;
   `effects.rs` reboots. `render/` produces bytes; `http.rs` serves them.
3. **Two workspaces (§6.1, §20.1).** `harvester/` is *excluded* from the root
   workspace and has its own `Cargo.toml` + `.cargo/config.toml`. `cargo test`
   at the root must pass on a machine with **no ESP toolchain installed**.
4. **Bail on the unknown (§7).** BTHome object parsing uses a whitelist, never a
   guessed size table. An unrecognised object id stops the walk and returns what
   was accumulated. A wrong length silently desyncs and produces *plausible
   garbage*, which is worse than no data.
5. **Never commit a secret (§20.7).** `harvester/src/devices.rs` is gitignored;
   `devices.rs.example` is committed. No real MAC, bindkey, PSK or OTA token in
   git history, ever. `.gitignore` lands in the first commit, before anything else.

## Config split (§5)

- `harvester-core` — **capacities.** `MAX_DEVICES = 8`, `SPARK_DEPTH`,
  `LOG_LINES`. Core is allocation-free, so it must own its own array sizes.
- `harvester/src/config.rs` — **committed.** Behavioural tunables, injected into
  core as a `Tunables` struct. §10.1 asks us to re-derive `DEVICE_TIMEOUT` from
  measurements; that history has to live in git. When you change a tunable, the
  commit message records the measurement.
- `harvester/src/devices.rs` — **gitignored.** Secrets and the device list only.
  `Device` is defined in core; only the array lives here.
- `mac!` / `bindkey!` are `const fn` hex parsers. A malformed MAC is a *build
  error*, not a sensor that silently never reports.

## Commands

```bash
# Host tests — must work with no ESP toolchain, no hardware. Acceptance crit. 1.
cargo test                          # from repo root
cargo clippy --all-targets -- -D warnings

# Firmware (from harvester/). Single target: esp32c6 until the C3 arrives (D8).
cargo build --release
./scripts/flash.sh
./scripts/push_ota.sh harvester.lan # build, upload, verify new git_sha
./scripts/e2e.sh harvester.lan      # post-OTA smoke: devices present, sha, healthz
```

Target is `riscv32imac-esp-espidf` / `CONFIG_IDF_TARGET=esp32c6`. Never introduce
`AtomicU64`/`AtomicI64` — the C3 is `riscv32imc` and has no atomics extension.
§6.2's three mutexes already keep us clear of this by design.

## Conventions

- Every code file opens with two `ABOUTME: ` comment lines.
- TDD. Test first, watch it fail, minimal code to green, refactor.
- `harvester-core` is `no_std`, **allocation-free, float-free and panic-free**.
  No `format!`, no `Vec`, no `String`, no `f32` (neither chip has an FPU), no
  indexing, no `unwrap`, no bare arithmetic — the lints in §6.1 enforce all of
  it. Render into a caller-supplied `core::fmt::Write` sink.
- Use `sg` (ast-grep) for code search and refactoring, not grep/sed.
- Time only ever enters `harvester-core` through the injected `Clock` trait.
  Staleness logic is purely monotonic; wall-clock conversion happens at render
  time (§11.3).
- Port 8183. Don't change it — it is the Python exporter's port and §19's
  parallel run scrapes both instances on it.

## Where things are written down

- `SPEC.md` (v1.1) — **the** document. Brief, decisions and rationale in one
  place, deliberately, so nobody has to reconcile two sources. §22 is the
  decision log; don't re-litigate those. New decisions go into the relevant
  section *and* get a §22 row — written evergreen, describing what is, not what
  changed.
- Journal — session-by-session notes, dead ends, frustrations, jokes.
