# BLEociraptor

ESP32 firmware in Rust that passively harvests BTHome v2 BLE advertisements from
Xiaomi LYWSD03MMC sensors and exposes them to Prometheus on port 8183, plus a
self-contained dashboard. Replaces a Python `ble_exporter` on a Raspberry Pi 4B.

- **`SPEC.md`** — the complete build brief, decisions and rationale. Start there.
- **`harvester-core/`** — `no_std` decision crate; everything host-testable.
- **`harvester/`** — firmware: I/O, transport and effects only.

## Development

```bash
git config core.hooksPath .githooks   # once, after cloning
cargo test                            # host tests — no ESP toolchain needed
```

The dashboard at `/` is always served gzipped; with curl use `curl --compressed`.
