# BLEociraptor — ESP32-C3 BLE Beacon Harvester

**Implementation specification, v1.1**
Status: ready to build · Target: ESP32-C6 now, ESP32-C3 when it arrives (§4.1) · Language: Rust

Replaces the Python `ble_exporter` currently running on a Raspberry Pi 4B. This
document is the complete build brief; it assumes no prior knowledge of the
Python implementation beyond the fixture corpus described in §17.

---

## 1. Goals

1. Passively listen for BTHome v2 advertisements from Xiaomi LYWSD03MMC sensors
   running ATC_MiThermometer firmware, decrypt where a bindkey is configured,
   and expose the readings to Prometheus.
2. Serve a self-contained mobile-friendly dashboard requiring no other running
   service.
3. Run unattended for months, recovering from realistic failures without
   intervention, and be updatable over the network.
4. Preserve the existing Prometheus metric **names and labels** exactly, so
   existing Grafana panels and historical series survive the platform swap
   unbroken. One metric's *values* change by design — see §11.1.

## 2. Non-goals

- Battery operation. The device is mains/USB powered; no sleep modes are used.
- Runtime reconfiguration. Device list and credentials are compile-time (§5).
- Discovering unknown sensors. MACs are known in advance from manual flashing.
- Any Python. The new repo contains none, at runtime or in its test suite.
- Connecting to sensors. Observer role only — never a GATT central.

## 3. Critical design constraint: metric gaps are a feature

**When a device stops beaconing, its series must disappear from `/metrics`.**

This is deliberate and load-bearing. A gap in the graph reads as "the sensor was
not there"; a flat re-scraped line reads as data that must be mentally
discounted. Do **not** substitute the more common pattern of always exporting a
`last_update` timestamp and computing staleness in PromQL. The per-device
staleness timers and active series removal are required behaviour, not
incidental complexity inherited from the Python version.

`ble_sensor_seen{device}` is the exception: it stays present and goes to `0`,
providing a continuous liveness signal alongside the gap.

**Gaps are a gauge idiom; counters never gap.** `ble_harvester_beacons_total`
carries a `device` label too, but removing it on timeout would make `rate()` read
a counter reset, breaking the very §19 cutover query the counter exists to serve.
A counter's flat line already says "nothing happened". For the same reason every
`{device} × {ok, decrypt_fail, parse_fail}` combination and all seven
`reboot_total{reason}` series are initialised to `0` at boot — a counter that
springs into existence on its first increment gives `increase()` nothing to
subtract from, which would silently disarm §15.1's reboot alert.

## 4. Platform and toolchain

| Item | Value |
|---|---|
| MCU, now | ESP32-C6 (RISC-V, 160 MHz) — the board on hand |
| MCU, eventual | ESP32-C3 "Pro Mini" (single core, 160 MHz, **400 KB SRAM**, 4 MB flash) |
| Rust target | `riscv32imac-esp-espidf` (tier 3, `std`), `CONFIG_IDF_TARGET=esp32c6`. Becomes `riscv32imc` / `esp32c3` on the switch (§4.1) |
| Framework | ESP-IDF via `esp-idf-svc` / `esp-idf-hal` / `esp-idf-sys` |
| BLE stack | NimBLE, **observer role only**, via `esp32-nimble` |
| Toolchain | Stock Rust nightly. RISC-V needs no Xtensa fork; tier-3 std target requires `-Zbuild-std=std,panic_abort`. Start from `esp-rs/esp-idf-template`. |
| Linker | `ldproxy` |
| Flashing | `espflash` / `cargo-espflash` |

Pin exact crate versions at integration time — the esp-rs ecosystem moves fast
and the `esp32-nimble` scan API in particular has changed shape between minor
releases. All API snippets in this document are **indicative, not verbatim**.

### 4.1 Platform notes that will otherwise cost a day each

- **Build for the C6, but write for the C3.** The C3 has not arrived, so the only
  target is `riscv32imac-esp-espidf` / `CONFIG_IDF_TARGET=esp32c6`. There is no
  dual-target build machinery, because an ESP-IDF image is chip-specific and a C3
  image would not boot on a C6 regardless. The C6 is a strict superset, so every
  capability it has and the C3 lacks is a trap that compiles, flashes and works
  right up until the real hardware arrives:

  | Don't use | Because the C3 |
  |---|---|
  | atomics — `AtomicU64`, `AtomicI64`, any of them | is `riscv32imc`, **no `A` extension**. §6.2's three mutexes already keep us clear by design |
  | more than **400 KB SRAM** | has 400 KB; the C6 has 512 KB |
  | 802.15.4 (Thread/Zigbee), the LP core | has neither |
  | Wi-Fi 6 / 802.11ax features, TWT | is Wi-Fi 4 only |
  | BLE extended advertising, coded PHY | supports both, but BTHome is legacy advertising — anything else is gratuitous divergence |
  | more than 4 MB flash | has 4 MB. Keep `partitions.csv` at the 4 MB layout even if the C6 board ships 8 |

  The SRAM ceiling is the entry needing active attention: it is the only one that
  degrades silently and gradually rather than failing to compile. Read
  `ble_harvester_min_free_heap_bytes` against 400 KB, not against what the C6
  reports free. Nothing in `harvester-core` is target-aware, and that is the bulk
  of the code, so the eventual switch should be a target triple and an
  `IDF_TARGET` change.
- **Use NimBLE, not Bluedroid.** Bluedroid would consume a large fraction of the
  C3's 400 KB for GATT features this project never uses.
- **Disable Wi-Fi power save**: `esp_wifi_set_ps(WIFI_PS_NONE)`. Default
  `MIN_MODEM` introduces DTIM-interval latency on scrapes. Costs ~20 mA, which
  is irrelevant on mains power.
- **The IPEX connector is probably not live by default.** On most of these
  boards, switching from the ceramic antenna to the u.FL connector requires
  moving a 0-ohm resistor or reflowing a solder jumper. Confirm against the
  board schematic *before* you need it.

## 5. Configuration — compile time

Config is baked into the binary. Maximum **8 devices** (`MAX_DEVICES`). Changing
anything means a rebuild and an OTA push (§13), which is a ~20 second operation.

`harvester/src/devices.rs` is **gitignored**; `devices.rs.example` is committed.
This mirrors the `config.yaml` / `config.yaml.example` pattern of the Python
project. No real MACs, bindkeys, PSKs or tokens are ever committed.

`harvester/src/config.rs` **is committed**, and this split is deliberate. §10.1 instructs
you to re-derive `DEVICE_TIMEOUT` from measured reception rates, and §9's scan
duty cycle may need tuning against observed coex behaviour. Those are
engineering decisions whose history matters — if they lived in the gitignored
file, every future tuning pass would start from zero with no record of what was
tried, why, or what the measurements were. Secrets go in the file git never
sees; **tunables go in the file git remembers.** Commit a message explaining the
measurement whenever you change one.

Config is split across **three** places, because they have different audiences
and different lifetimes.

### 5.1 Capacities — `harvester-core`, committed

`harvester-core` is allocation-free, so the constants sizing its fixed arrays
must be known to it at compile time. They belong to core, not to the firmware
config, and core never reads the firmware's config to get them:

```rust
// harvester-core/src/lib.rs
pub const MAX_DEVICES: usize = 8;
pub const SPARK_DEPTH: usize = 72;   // x SPARK_SAMPLE_INTERVAL -> 6 h (§18)
pub const LOG_LINES: usize = 128;    // §16
pub const LOG_LINE_BYTES: usize = 96;
```

### 5.2 Tunables — `harvester/src/config.rs`, committed

```rust
pub const LISTEN_PORT: u16 = 8183;
pub const DEVICE_TIMEOUT: Duration = Duration::from_secs(90);
pub const SCAN_INTERVAL_UNITS: u16 = 64;  // x 0.625ms = 40ms  (§9)
pub const SCAN_WINDOW_UNITS:   u16 = 48;  // x 0.625ms = 30ms  (§9)
pub const WEDGE_WINDOW: Duration = Duration::from_secs(600);        // §15.1
pub const WIFI_GIVEUP:  Duration = Duration::from_secs(300);        // §15
pub const SPARK_SAMPLE_INTERVAL: Duration = Duration::from_secs(300);
pub const OTA_VALIDATE_AFTER:    Duration = Duration::from_secs(60);  // §13
pub const OTA_VALIDATE_DEADLINE: Duration = Duration::from_secs(600); // §13
```

These are behavioural rather than structural, so they are **injected into
`harvester-core` at construction** in a `Tunables` struct. The firmware hands
core what it needs; core never reaches back.

### 5.3 Secrets and devices — `harvester/src/devices.rs`, gitignored

```rust
pub const WIFI_SSID: &str = "...";
pub const WIFI_PSK:  &str = "...";
pub const OTA_TOKEN: &str = "...";

pub const DEVICES: [Device; 3] = [
    Device::plain(mac!("AA:BB:CC:00:00:01"), "baby_room"),
    Device::plain(mac!("AA:BB:CC:00:00:02"), "humidor"),
    Device::encrypted(mac!("AA:BB:CC:00:00:03"), "living_room",
                      bindkey!("00112233445566778899aabbccddeeff")),
];
```

`Device` is **defined in `harvester-core`**; only the array is instantiated here,
and it reaches core as a `&'static [Device]`. The type, the parsers and the
matching logic stay host-testable while every secret stays in the file git never
sees.

`mac!` and `bindkey!` must be **`const fn` hex parsers** exported from core, not
runtime parsing. This yields `[u8; 6]` and `[u8; 16]` at compile time, so:

- a malformed MAC or a 31-character bindkey is a **build error**, not a device
  that silently never reports;
- runtime device matching is a 6-byte compare with no string handling at all.

A ninth device is a build error by the same principle, not a silent runtime drop:

```rust
const _: () = assert!(DEVICES.len() <= harvester_core::MAX_DEVICES);
```

Because `devices.rs` is gitignored, **a fresh clone does not compile.** Left
alone that surfaces as an unresolved-module error explaining nothing, so
`build.rs` checks for the file and fails with a message naming
`devices.rs.example`.

Do not use the `toml-cfg` crate here. It handles flat key/value pairs but not
arrays of tables, so the device list would not fit.

Bindkeys and the Wi-Fi PSK sit in plaintext flash. ESP-IDF flash encryption is
**not** enabled; this was considered and judged acceptable for a home
thermometer. Revisit if the threat model changes.

## 6. Architecture

### 6.1 Crate layout

The workspace is split by **purity, not by subject matter**. Everything that
makes a decision lives in `harvester-core` and is host-testable; `harvester`
contains only I/O, transport and effects.

```
bleociraptor/
  Cargo.toml               # workspace A: HOST crates only
  harvester-core/          # no_std, zero ESP deps, host-testable
    src/
      frame.rs             # BTHome v2 frame decrypt
      parse.rs             # object-id decoding
      registry.rs          # device state, staleness, gap logic
      clock.rs             # Clock trait (injected)
      counters.rs          # beacon / reboot / unknown-advert counters
      wedge.rs             # wedge VERDICT (pure decision, no reboot)
      logring.rs           # in-RAM log ring
      spark.rs             # sparkline history ring
      fixed.rs             # integer fixed-point formatting -- no floats (§6.1)
      render/
        prometheus.rs      # exposition text
        readings.rs        # /api/readings JSON
        history.rs         # /api/history JSON (sparklines)
    tests/
      fixtures/            # captured corpus (§17.2)
  harvester/               # workspace B: firmware, effects only
    Cargo.toml             # [workspace] of its own -- see below
    .cargo/config.toml     # target = riscv32imac-esp-espidf, build-std
    src/
      main.rs              # wiring, threads, the three mutexes (§6.2)
      ble.rs               # NimBLE observer -> harvester_core
      http.rs              # esp_http_server routes -> render::*
      wifi.rs              # esp_wifi, reconnect backoff
      ota.rs               # EspOta
      effects.rs           # esp_restart, NVS, heap/RSSI probes
      config.rs            # committed tunables (§5)
      devices.rs           # gitignored secrets (§5)
      devices.rs.example
    assets/index.html      # embedded, gzipped
  scripts/
    push_ota.sh
  SPEC.md
  README.md
```

**`harvester` must be a separate workspace, not a member of the root one.**
A `.cargo/config.toml` sets `target` for every crate at or below its directory.
If the ESP target lived at the repo root, `cargo test -p harvester-core` would
try to build the host tests for `riscv32imac-esp-espidf` — and acceptance
criterion 1 ("passes on the host with no hardware attached") becomes
unsatisfiable. Excluding `harvester/` from the root workspace and giving it its
own `Cargo.toml` and `.cargo/config.toml` keeps `cargo test` at the root
host-native, while `cargo build` inside `harvester/` cross-compiles.
`harvester` depends on `harvester-core` by relative path.

**The invariant:** if a function's behaviour can be described without mentioning
hardware, it belongs in `harvester-core`. `harvester` makes no decisions.

Two placements that look wrong at a glance but are the point of the split:

- **`wedge.rs` decides, it does not reboot.** It is a pure function of
  (per-device last-seen times, `unknown_adverts_total`, now) returning a verdict
  enum. `harvester/effects.rs` is what calls `esp_restart()`. This is what makes
  §15.1's AND-condition testable on a laptop with a fake clock, which §17.1
  requires because the hardware checklist cannot induce a wedge on demand.
- **`render/` produces bytes, it does not serve them.** `http.rs` owns the
  socket; `render::prometheus` owns the exposition format. That is what lets
  §17.1 assert on exposition output — including that a timed-out device's series
  are *absent* (§3) — with no ESP-IDF in the build.

`harvester-core` is `no_std`, allocation-free, float-free and panic-free.
Rendering writes into a caller-supplied `core::fmt::Write` sink, so host tests
render into a `String` while the firmware renders into a fixed buffer or straight
into a chunked socket writer.

**No floats.** The C3 and C6 are RV32IMC/IMAC — no `F` extension, no FPU — so
every `f32` operation is a soft-float library call. Core holds the integers the
wire already carries (centidegrees, centipercent, millivolts) and renders them as
fixed-point with integer math: `2696` → `26.96`, `2939` → `2.939`. Three wins for
one decision: no soft-float in the hot path, byte-stable golden tests (`core::fmt`
on an `f32` will happily emit `26.960001`), and no rounding drift between
`/metrics`, `/api/readings` and the sparklines. Floats appear only in host tests,
which are `std`, when comparing against the corpus's pre-scaled values (§17.2).

**No panics, structurally.** A panic on this target is a reboot, and this crate
parses untrusted radio input, so the guarantee is enforced by lint rather than by
care:

```rust
#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used,
        clippy::panic, clippy::arithmetic_side_effects)]
```

Slice access goes through `get()`, arithmetic through `checked_*` /
`saturating_*` / explicit `wrapping_*`. A length-prefixed walk over
attacker-influenced bytes is exactly where `payload[i + len]` panics, and
`beacons_total += 1` in a release build does not panic on overflow — it silently
**wraps**, which is worse. Denying `arithmetic_side_effects` forces the intent to
be written down at every site.

**Time enters only through the injected `Clock` trait**, which has two methods,
because §11.3 needs to distinguish "how long ago" from "at what wall-clock time":

```rust
trait Clock {
    fn monotonic(&self) -> Millis;          // since boot, never steps
    fn unix_seconds(&self) -> Option<u64>;  // None until SNTP syncs (§11.3)
}
```

`Millis(u64)` is core's own monotonic newtype; `std::time::Instant` does not
exist in `no_std`. Staleness (§10) uses `monotonic` only, so an NTP step can
never make a device look spuriously stale or fresh, and `unix_seconds() == None`
is exactly the condition for omitting
`ble_sensor_last_update_timestamp_seconds` while reporting
`ble_harvester_time_synced 0`.

This crate is where all the dangerous code lives — parsing untrusted radio input
— so it gets the fast, thorough, fully automated test tier (§17.1).

### 6.2 Threads and shared state

Three concurrent contexts, which is the single biggest change from Python. The
Python implementation ran everything on one asyncio event loop and therefore
could not observe a torn read. **That guarantee is gone.**

| Context | Owner | Responsibility |
|---|---|---|
| NimBLE host task | stack | GAP callback → `on_advert()` |
| Housekeeping thread | us | 1 Hz sweep: staleness, wedge check, sparkline sampling, heap sampling, OTA validation |
| HTTP task | `esp_http_server` | request handling — a **single** task, see below |

#### State budget

| Region | Contents | Size |
|---|---|---|
| `Hot` | 8 x device readings, counters, harvester health | **~0.8 KB** |
| `Spark` | 8 x `SPARK_DEPTH`(72) x 2 metrics x `i16` | **~2.3 KB** |
| `Logs` | `LOG_LINES`(128) x `LOG_LINE_BYTES`(96) | **~12 KB** |
| | total | **~15 KB** of 400 KB |

`Hot` in detail, since rule 3 below copies the whole struct onto the HTTP task's
stack on every scrape:

| Item | Bytes |
|---|---|
| 8 x readings (temp, humidity, battery, voltage, rssi, per-field presence) | ~72 |
| 8 x `last_seen: Millis` + `online` | ~72 |
| 8 x beacon counters (3 x `u32`, one per `result`) | 96 |
| 8 x beacons/min ring — 60 one-second buckets, `u8` (§18) | **480** |
| globals: reboot counts, `unknown_adverts_total` (`u64`), `parse_unknown_object_total`, novel-object log latch, `ota_in_progress`, `time_synced` | ~90 |
| | **~810** |

The beacons/min rings dominate. Re-check this table before adding anything to
`Hot`; `Logs` is the obvious place to reclaim space if it is ever needed, since
`LOG_LINES` can be halved without losing the recent history that matters.

#### Three locks, not one

`Logs` is twelve times the size of everything a `/metrics` scrape actually
reads. Behind one shared mutex, every scrape would copy the entire log ring to
render a handful of gauges, and the hot path would contend with it. So the state
is split by access pattern:

```rust
static HOT:   Mutex<Hot>      // BLE callback + housekeeping + most endpoints
static SPARK: Mutex<Spark>    // housekeeping every SPARK_SAMPLE_INTERVAL; /api/history
static LOGS:  Mutex<LogRing>  // rare writes; /logs
```

| Reader | Locks | Copies |
|---|---|---|
| `/metrics`, `/api/readings`, `/status` | `HOT` | ~0.8 KB |
| `/api/history` | `SPARK` | ~2.3 KB |
| `/logs` | `LOGS` | nothing — streams, see below |
| `/healthz` | none | — |

#### Rules

1. **The BLE callback must never block.** No allocation, no flash I/O, no
   formatting. Acquire `HOT`, mutate primitives, release. It touches `LOGS` only
   on a decrypt or parse failure, which is rare by definition.
2. **Never hold two locks at once.** The housekeeping sweep needs `HOT` and then
   `SPARK`; take them *sequentially*, never nested. This makes lock ordering a
   non-question and deadlock unreachable, which is worth more than the
   consistency a combined snapshot would buy — nothing here needs readings and
   sparklines to be atomic with respect to each other.
3. **Do not format under `HOT`.** Snapshot ~1 KB, release, then render. A slow
   scrape must never stall the radio callback.
4. **`/logs` is the exception: it streams under `LOGS`.** Copying 12 KB to a
   HTTP task's stack is not an option (below), and holding `LOGS` for the duration
   of a response is safe precisely because of who contends for it — only rare
   failure-path writes, never the hot path. A log fetch can briefly delay a log
   *write*; it can never delay a beacon.

#### `esp_http_server` runs a single task

It is not a worker pool. One task services all `max_open_sockets` connections, so
requests are **serialised**. Three consequences:

1. **A slow `/logs` reader delays a `/metrics` scrape**, and the lock split
   cannot help, because they contend for the *task* rather than for `LOGS`. Bound
   it with explicit `send_wait_timeout` and `recv_wait_timeout` — this is the
   concrete form of §12's "keep idle connection timeouts short".
2. The three-lock design above is still right, but for the contention that
   actually exists: BLE callback vs. housekeeping vs. HTTP. Endpoint-vs-endpoint
   contention was never possible.
3. `stack_size` buys one stack, not one per socket, so headroom is cheap.

#### HTTP task stack size

**Raise `httpd_config_t.stack_size` to 10240.** The `HTTPD_DEFAULT_CONFIG()`
default is 4096 bytes, which a 2.3 KB snapshot plus JSON formatting plus the
`esp_http_server` call frames will exhaust. Stack overflow presents as a
spontaneous reboot under load — it would show up as `reboot_total{reason="panic"}`
climbing whenever someone opens the dashboard, and it is a genuinely unpleasant
thing to diagnose from a closet. Since there is one task, the whole cost is a
single 10 KB allocation.

#### Boot order

**Start the watchdog before anything that can hang.** A hang *before* the
housekeeping thread subscribes to the TWDT is uncovered twice over: no watchdog
to reboot it, and therefore no reboot to trigger OTA rollback. A freshly pushed
image that deadlocks during Wi-Fi or NimBLE bring-up simply stops, forever, with
the previous good slot sitting unused.

```
init HOT / SPARK / LOGS      (pure memory, cannot block)
  -> start housekeeping thread
     -> subscribe it to esp_task_wdt
        -> Wi-Fi, SNTP, BLE, HTTP
```

The sweep must therefore tolerate a world in which nothing else exists yet. That
costs a few `Option` checks and buys watchdog coverage across the entire risky
part of startup.

### 6.3 Data flow

```
NimBLE advert
  └─ addr LE → reverse to [u8;6]
     └─ match against DEVICES (6-byte cmp)
        ├─ no match → unknown_adverts_total += 1  ... done
        └─ match
           ├─ extract service data for UUID 0xFCD2
           ├─ if device_info & 0x01 → decrypt (§8)   ─ fail → decrypt_fail
           ├─ parse BTHome objects (§7)              ─ fail → parse_fail
           ├─ range-check each reading (§7)          ─ fail → parse_fail
           ├─ lock: update readings, rssi, last_seen, counters
           └─ unlock
```

## 7. BTHome parsing

Frames arrive as service data under UUID **`0xFCD2`**. Filter on that UUID
explicitly. Do not take "the first non-empty service data entry" — that is what
the Python version does and it is only correct by accident.

Validate the version field: `(device_info >> 5) == 2` for BTHome v2. Reject
otherwise.

Then walk object-id/value pairs. **Use a whitelist, not a size table:**

| ID | Meaning | Size | Handling |
|---|---|---|---|
| `0x00` | packet id | 1 | consume, ignore |
| `0x01` | battery | 1 | `battery_percent` |
| `0x02` | temperature | 2, `i16` LE, ×0.01 | `temperature_celsius` |
| `0x03` | humidity | 2, `u16` LE, ×0.01 | `humidity_percent` |
| `0x0C` | voltage | 2, `u16` LE, ×0.001 | `voltage_volts` |
| `0x10` | binary | 1 | consume, ignore |
| `0x11` | binary | 1 | consume, ignore |
| `0x3E` | count, `u32` | 4 | consume, ignore |
| *other* | — | — | **stop parsing, return what was accumulated, increment `parse_unknown_object_total`, log once** |

`0x3E` is on the list because the fixture corpus contains it: one MIC-valid
living_room frame decrypts to `11 01 3e 00 00 00 00`, where `0x3E`'s four bytes
consume the payload exactly. Its length is therefore measured, not guessed —
which is the procedure §17.2.1 prescribes for any novel id. Leaving it off would
also make `parse_unknown_object_total` climb on roughly 6% of that device's
frames forever, turning a metric that should be an alarm into background noise.

Rationale: the Python implementation carries a hand-maintained size table for
every BTHome object id, and several entries are wrong (`0x04` pressure and
`0x05` illuminance are 3 bytes, not 2 and 1; `0x0A` energy and `0x0B` power are
3, not 1; `0x10` is 1, not 3). It works today only because two of the errors
cancel out for this firmware's exact object ordering. A guessed length that is
wrong silently desyncs the walk and produces **plausible garbage** for
subsequent known objects. Bailing on the unexpected is strictly safer, and the
whitelist above is validated empirically against the fixture corpus (§17).

**Battery comes from `0x01`, directly.** The Python version ignores the
explicit battery percentage the sensor transmits and instead derives one from
voltage via a linear 2.0–3.0 V curve — CR2032 discharge is markedly nonlinear,
so that number is fiction. Expose voltage separately as its own metric; it is
the better input for judging when to swap cells.

Values are stored as the integers the wire carries — centidegrees, centipercent,
millivolts — and scaled only at render time (§6.1). No floats anywhere in core.

### 7.1 Classifying the outcome

`beacons_total{result}` answers **"was this beacon intelligible"**, so the
discriminator is whether the object walk terminated cleanly, *not* how many
readings came out:

| Outcome | `result` | Also |
|---|---|---|
| Walk completed | `ok` | — |
| Walk stopped cleanly at an unknown object id | `ok` | `parse_unknown_object_total += 1` |
| No or empty `0xFCD2` service data; `(device_info >> 5) != 2`; an object's length runs past the end of the payload | `parse_fail` | — |
| A reading fails its range check (§7.2) | `parse_fail` | — |
| MIC check failed | `decrypt_fail` | — |

A reading-count rule was rejected: the corpus contains `11 01 3e 00 00 00 00`, a
structurally perfect frame whose objects are all on the ignore list and which
therefore yields *zero* readings. Counting that as `parse_fail` would make ~6% of
one device's beacons look like failures permanently, and §19's comparison would
read as degraded reception when reception was fine.

**"Log once" is latched per `(device, object_id)`.** Per frame would flood the
128-line ring within seconds at 0.6 beacons/sec; per boot would hide a second
novel id behind the first. The latch is a small fixed-size set in `Hot`; once
full, further novel ids are counted but not logged. Losing a log line is
acceptable — `parse_unknown_object_total` is the signal that matters.

### 7.2 Range checks

Readings outside plausible physical range are **dropped and the beacon counted
`parse_fail`**:

| Reading | Accepted range |
|---|---|
| temperature | −40 … 85 °C |
| humidity | 0 … 100 % |
| battery | 0 … 100 % |
| voltage | 0 … 4 V |

Two reasons, and the mechanical one is sufficient on its own: humidity is `u16`
on the wire but `i16` in the sparkline ring (§18), so a garbage `0xFFFF`
(655.35 %) overflows on store. The second is that plaintext devices carry **no
MIC and no authentication of any kind** — anyone in range can transmit a BTHome
frame bearing their MAC and we will publish it. The threat model accepts that
(§5), but a range check costs nothing and keeps one bad frame from parking a
655 % spike in Grafana history forever.

## 8. Decryption

Verified empirically against real hardware, and re-verified against the fixture
corpus — **all 17 encrypted frames decrypt on the first attempt** under exactly
the scheme below. Do not "fix" it to match the BTHome spec text, which is
ambiguous on the MAC byte order.

```
frame:  device_info(1) || ciphertext(N) || counter(4, LE) || mic(4)
nonce:  mac(6) || 0xD2 0xFC || device_info(1) || counter(4)     = 13 bytes
aad:    empty
cipher: AES-CCM, 128-bit key, 4-byte tag
crate:  ccm::Ccm<Aes128, U4, U13>
```

- **The MAC in the nonce is in human-readable order** (`AA:BB:CC:DD:EE:FF` →
  `[AA,BB,CC,DD,EE,FF]`).
- **NimBLE hands you `ble_gap_disc_desc.addr.val` reversed (little-endian).**
  You must reverse it, both for device matching and for nonce construction. This
  is the single most likely source of a "decryption always fails" afternoon.
- On failure: increment `beacons_total{result="decrypt_fail"}`, log to the ring,
  drop the frame. Never panic.
- Software AES is fine. A 10-byte payload at under one frame per second is
  microseconds; the hardware accelerator is not worth the `unsafe` FFI.

**Replay counter:** the counter participates in the nonce but is deliberately
**not** enforced as monotonic, and regressions are **not** counted or stored
either. ATC firmware reuses the counter across paired frames, so strict
enforcement would drop valid data — and a metric counting the reuse would fire on
a large fraction of all frames while telling us nothing actionable. There is no
per-device `last_counter` field.

The corpus makes the reuse concrete: living_room's frames span counters
1108604–1108665 with values repeating across *different* plaintexts — 1108607
appears on both an `[01 02 03]` and a `[0C 10 11]` frame. Same MAC, same
`device_info`, same counter means the same 13-byte nonce encrypting different
plaintext under the same key. This is AES-CCM nonce reuse, not merely
counter reuse, and it means these bindkeys provide weaker confidentiality than
the label suggests. Accepted for a home thermometer; revisit with the threat
model.

## 9. BLE scanning

Observer role, **passive scanning**, continuous but duty-cycled.

```rust
scan.active_scan(false)      // MANDATORY — see below
    .interval(64)            // 64 × 0.625ms = 40 ms
    .window(48)              // 48 × 0.625ms = 30 ms  → 75% duty
    .on_result(on_advert);
```

Two integration-time checks, because both fail quietly:

- **Confirm the units of `interval()` and `window()`.** Some `esp32-nimble`
  versions take 0.625 ms units, others take milliseconds. Either reading of `64`
  and `48` yields ~75% duty and a plausible-looking scanner, so a wrong guess
  never announces itself. Verify against the pinned version and record the
  derived milliseconds in a comment.
- **Confirm the scan does not end.** Some versions take a duration and stop when
  it elapses. Scanning must be continuous. A scan that quietly ends is precisely
  the §15.1 wedge case, and the wedge detector is in fact the most likely real
  cause of a `ble_wedge` reboot in production.

**Passive is mandatory, not a preference.** Active scanning makes the harvester
transmit `SCAN_REQ` and provokes `SCAN_RSP` from every sensor in range —
draining their CR2032s to learn nothing, since BTHome puts everything in the
advertisement. The Python implementation is accidentally active-scanning today
because that is `bleak`'s default.

**Window must be less than interval.** The C3 and C6 have a *single* 2.4 GHz
radio shared between Wi-Fi and BLE via the ESP-IDF coexistence arbiter. A
100%-duty scan starves the Wi-Fi side, producing scrape timeouts and
retry-driven power *increases*. 75% duty loses nothing detectable given the
sensors re-advertise every ~2 s.

## 10. State model and staleness

Per device: `temperature`, `humidity`, `battery`, `voltage`, `rssi`,
`last_seen: Millis`, `online: bool`, three beacon counters, and a 60-bucket
beacons/min ring (§18).

**Presence is per field; expiry is per device.** ATC alternates two frame shapes
and the corpus confirms they are disjoint — `{battery, temperature, humidity}` or
`{voltage}`, never both — so a device's voltage is routinely ~2 s newer than its
temperature. Each field is independently `Option`-like, but staleness is driven
by `last_seen` alone. There is no per-field expiry: the frames alternate every
~2 s, so the skew sits far below `DEVICE_TIMEOUT` and tracking it would buy
nothing.

The 1 Hz housekeeping sweep compares `now - last_seen` against
`DEVICE_TIMEOUT` (default **90 s**). On expiry:

- remove `temperature`, `humidity`, `battery`, `voltage`, `rssi`, and
  `last_update_timestamp` series for that device (§3);
- set `ble_sensor_seen{device} = 0`;
- mark offline for the dashboard.

Counters are untouched by expiry — see §3.

Prefer a single periodic sweep over N one-shot timers. With six devices it is
simpler, has no handle lifecycle to leak, and runs off the radio callback path.

**Timeout raised from 60 s to 90 s.** Duty-cycled scanning plus coex arbitration
means a higher miss rate than the Pi's dedicated adapter. At ~2 s advertising
intervals, 90 s still represents dozens of expected beacons, so a genuine
offline event is detected promptly while a run of bad luck is not misreported.
This is one `const`; tune against observed `beacons/min` after deployment.

### 10.1 Coupling to the sensors' advertising interval

`DEVICE_TIMEOUT` is not an independent knob — it is a function of how often the
sensors advertise. Keep it at **18–36 advertising intervals**.

| ATC advertising interval | Sane `DEVICE_TIMEOUT` |
|---|---|
| 2.5 s (current) | 45–90 s (spec uses 90 s) |
| 10 s (under consideration) | 180–360 s |

Raising the advertising interval does **not** affect Wi-Fi/BLE coexistence. The
arbiter's ratios are set by the scanner's duty cycle (§9) — the receiver is on
75% of the time whether or not anything is transmitting. What it does affect is
the number of chances to hear a device inside the timeout window, which falls
proportionally.

That matters more than independent probability suggests, because RF losses are
bursty and correlated — an obstructed path, a microwave, a large Wi-Fi transfer
winning arbitration — so consecutive misses cluster. Holding robustness constant
therefore costs detection latency directly, and detection latency is what the
gap-based model in §3 is built on.

**Do not tune this by estimation.** After the parallel run (§19), the exported
`ble_sensor_rssi_dbm` and per-device beacons/min give the measured per-beacon
reception rate for each sensor in its actual location. Set the timeout from that
number, and re-derive it if the advertising interval ever changes.

**Derive it from the worst device, not the average.** One sensor is periodically
placed in the freezer to watch for the clogged-condenser failure that once let it
drift to −9 °C. It is inside a grounded metal box, so its RSSI will be dreadful
and its misses will cluster hard — exactly the bursty, correlated loss described
above. Expect that device to set `DEVICE_TIMEOUT` for everyone. Expect its
`ble_sensor_seen` to flap as the steady state rather than as a fault, and give
any alert built on it a `for:` clause.

## 11. Metrics contract

### 11.1 Carried over from the Python implementation

Names and labels are byte-identical so existing dashboards and history survive
(§19). One metric changes *value semantics*, which is not the same as changing
its name:

| Metric | Type | Labels | Continuity |
|---|---|---|---|
| `ble_sensor_temperature_celsius` | gauge | `device` | identical |
| `ble_sensor_humidity_percent` | gauge | `device` | identical |
| `ble_sensor_last_update_timestamp_seconds` | gauge | `device` | identical name, see §11.3 |
| `ble_sensor_seen` | gauge | `device` | identical |
| `ble_sensor_battery_percent` | gauge | `device` | **step change — see below** |

**`ble_sensor_battery_percent` will visibly jump at cutover.** The Python
implementation derives it from voltage via a linear 2.0–3.0 V curve; this
implementation reads the explicit battery percentage from BTHome object `0x01`
(§7). The name is preserved so the panel keeps working, but the series has a
discontinuity at the swap and historical values are not comparable to new ones.

This is intentional — the old number was fiction, since CR2032 discharge is
markedly nonlinear. Annotate the cutover instant in Grafana so the step is
self-explanatory a year from now, and do not set battery alert thresholds from
pre-cutover history. During the parallel run (§19) the two instances will
disagree on this metric and only this metric; that disagreement is the expected
result, not a bug.

### 11.2 New

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `ble_sensor_rssi_dbm` | gauge | `device` | per-sensor signal; the antenna verdict |
| `ble_sensor_voltage_volts` | gauge | `device` | raw `0x0C`, honest battery health |
| `ble_harvester_uptime_seconds` | gauge | | |
| `ble_harvester_free_heap_bytes` | gauge | | |
| `ble_harvester_min_free_heap_bytes` | gauge | | low-water mark; leak detection |
| `ble_harvester_wifi_rssi_dbm` | gauge | | |
| `ble_harvester_time_synced` | gauge | | 0/1, SNTP state |
| `ble_harvester_build_info` | gauge | `version`,`git_sha` | always 1 |
| `ble_harvester_reboot_total` | counter | `reason` | **persisted in NVS**, see below |
| `ble_harvester_beacons_total` | counter | `device`,`result` | `ok`\|`decrypt_fail`\|`parse_fail` |
| `ble_harvester_unknown_adverts_total` | counter | | radio-liveness heartbeat (§15). Hold it in a **`u64`** |
| `ble_harvester_parse_unknown_object_total` | counter | | should sit at 0; if it climbs, see §17.2.1 |

**`unknown_adverts_total` is `u64`, not `u32`.** It counts every stray advert in
radio range, and in dense RF a `u32` wraps in under three years of uptime — well
inside this device's intended lifetime, and `increase()` reads a wrap as a
counter reset.

**`reboot_total` must be persisted in NVS.** A counter held only in RAM resets
on the very reboot it is meant to record, so the event can never be observed.
`reason` ∈ `panic | wifi | ble_wedge | ota | power | wdt | unknown`.

**`esp_reset_reason()` alone cannot produce those labels.** Every deliberate
`esp_restart()` — Wi-Fi giveup, wedge detector, post-OTA — reports the same
`ESP_RST_SW`, so three of the seven reasons are indistinguishable after the
fact. The reason must be recorded *before* the restart, not inferred after it:

```
intentional reboot path:
  nvs_set_str("pending_reason", "ble_wedge")   // or wifi | ota
  nvs_commit()
  esp_restart()

boot path:
  match esp_reset_reason() {
    ESP_RST_SW        => take pending_reason, else "unknown"
    ESP_RST_PANIC     => "panic"
    ESP_RST_TASK_WDT
      | ESP_RST_INT_WDT | ESP_RST_WDT => "wdt"
    ESP_RST_POWERON | ESP_RST_BROWNOUT => "power"
    _                 => "unknown"
  }
  increment that NVS counter; clear pending_reason; seed the in-memory gauge
```

Clearing `pending_reason` on every boot matters: a stale value would mislabel
the *next* unexpected reset. An `ESP_RST_SW` with no pending reason recorded is
itself a signal — something called `esp_restart()` down a path that forgot to
declare itself — so keep `unknown` visible rather than folding it into another
bucket. Two NVS writes per intentional reboot is negligible wear.

### 11.3 Time

`ble_sensor_last_update_timestamp_seconds` is a Unix timestamp and therefore
requires real time. Use `EspSntp`, started **non-blocking** at boot. Until sync
completes, omit that one metric and report `ble_harvester_time_synced 0`;
everything else works normally.

**Store `last_seen` as a monotonic instant and convert only at render time:**

```
last_update_unix = now_unix - (now_monotonic - last_seen_monotonic)
```

Never stamp a wall-clock time onto a beacon as it arrives. Beacons received
before SNTP syncs — which includes every beacon of the first few seconds after
any reboot — would be stamped from a 1970 clock and stay wrong for the entire
life of that reading. Deriving at render time means the moment sync lands, every
already-received beacon reports a correct absolute timestamp. It also keeps
staleness logic (§10) purely monotonic, so an NTP step adjustment can never make
a device look spuriously stale or spuriously fresh.

SNTP is needed for nothing else — Prometheus stamps the samples itself, and the
dashboard uses relative ages.

### 11.4 Exposition format

Emit `# HELP` and `# TYPE` for every family, and render **metric-major**: all
label sets of one family contiguously, metadata first, before moving to the next
family.

The natural implementation is device-major (`for d in devices { temp; humidity;
battery }`), which interleaves families and emits a second `TYPE` line for a
family already declared. Prometheus rejects that with `second TYPE line for
metric name ...`, and it fails the **entire scrape** — every metric disappears
and `up` goes to 0, so the symptom presents as "target down" and points at the
network rather than at the formatter.

Metric-major also puts §3's gap logic where it belongs: skip-if-absent becomes a
filter on one per-family pass, rather than a condition threaded through a
device-major loop. A family whose devices are all offline emits its `HELP`/`TYPE`
with zero samples — valid, and the honest reading of §3.

`Content-Type: text/plain; version=0.0.4; charset=utf-8`.

## 12. HTTP surface

`esp_http_server` via `esp-idf-svc`. Set `max_open_sockets` to 5 — the default
is small enough that a scrape, a browser tab and a keepalive can exhaust it.
Keep idle connection timeouts short.

| Method | Path | Auth | Response |
|---|---|---|---|
| GET | `/` | none | static dashboard, always gzip, `ETag` |
| GET | `/api/readings` | none | JSON, ~1 KB, current values + health |
| GET | `/api/history` | none | JSON, ~7 KB, sparkline series |
| GET | `/metrics` | none | Prometheus text exposition |
| GET | `/healthz` | none | `ok` |
| GET | `/status` | none | JSON, exact shape below |
| GET | `/logs` | none | text, in-RAM ring |
| POST | `/ota` | `X-Auth` token | streamed firmware image |

**`/api/readings` and `/api/history` are separate on purpose.** Sparkline data
is 8 devices x 72 samples x 2 metrics — roughly 7 KB of JSON, several times the
readings payload. Serving them together would mean re-sending the entire history
on every 15 s poll for the sake of a few current values. The page fetches
`/api/history` once on load and refreshes it on the `SPARK_SAMPLE_INTERVAL`
(§18); `/api/readings` stays small and polls often. `render/history.rs` owns the
history payload, `render/readings.rs` the current one.

Three payload details that are easy to get subtly wrong:

- **`/api/readings` sends a server-computed `age_seconds`, not a timestamp.**
  §18 wants relative ages so the page is correct before SNTP syncs; deriving the
  age client-side from a server timestamp reintroduces a second clock — the
  *browser's* — and a phone with skew renders confidently wrong ages. One clock,
  one subtraction, performed by the device that owns the monotonic counter.
- **`/api/history` carries `interval_seconds`** alongside the series, so the page
  need not hard-code `SPARK_SAMPLE_INTERVAL` and a future tuning change cannot
  silently mislabel six hours of history as something else. Missing samples are
  `null` (§18).
- `/api/readings` includes each device's `encrypted` flag, for §18's lock icon.

**`/` is always served gzipped**, with `Accept-Encoding` ignored. Only the
compressed copy is stored, so a client that does not accept gzip gets bytes it
cannot read. Every browser accepts it; `curl` needs `--compressed`, which the
README says. Storing an uncompressed copy to satisfy one client that can pass a
flag is not worth the flash.

`/status` keeps the Python shape exactly (`exporter.py:77-80`):

```json
{"device_timeout_seconds": 90, "devices_active": 5}
```

`devices_active` is the count of devices currently inside their staleness
window — the same population that has series present in `/metrics`.

All endpoints are LAN-only and unauthenticated except `/ota`. If that changes,
`/ota` is the one that must never be reachable from outside.

## 13. OTA

Dual app slots with rollback. This is the safety net that makes compile-time
config tolerable — every config change is a firmware push.

```
partitions.csv
  nvs,      data, nvs,     0x9000,   24K
  otadata,  data, ota,     0xf000,    8K
  phy_init, data, phy,     0x11000,   4K
  ota_0,    app,  ota_0,   0x20000, 1600K
  ota_1,    app,  ota_1,         , 1600K
```

- `POST /ota` with `X-Auth: <OTA_TOKEN>`, body streamed to `EspOta`
  (`initiate_update` → `write` → `complete`), then reboot.
- **Do not mark the app valid at boot.** Call
  `esp_ota_mark_app_valid_cancel_rollback()` after `OTA_VALIDATE_AFTER` (60 s)
  of healthy uptime **and any one of**:
  - at least one successfully parsed beacon from a configured device, **or**
  - at least one unknown advertisement seen — the radio demonstrably works
    even if no configured sensor is in range, **or**
  - `OTA_VALIDATE_DEADLINE` (10 min) has elapsed — unconditional escape.

  The OR is load-bearing. Requiring a parsed beacon alone would boot-loop a
  perfectly good build on a bench board with no sensors nearby: validation never
  fires, rollback triggers, the old image has the same problem, forever. The same
  trap catches a genuine deployment where every sensor happens to be dead. The
  unknown-advert clause reuses §15.1's radio-liveness signal, and the deadline
  guarantees termination regardless. A bad push still auto-reverts and the device
  cannot be bricked remotely.
- `scripts/push_ota.sh` builds, uploads, waits for the device to come back, and
  verifies `ble_harvester_build_info` reports the new `git_sha`. **Use
  `git describe --always --dirty`** for that sha: a dirty working tree otherwise
  reports the last *commit's* sha, so the verification passes while the device
  runs uncommitted code — exactly during rapid iteration, when the check matters
  most. A dirty build reports `abc1234-dirty` and fails the check honestly.

**`CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y` is mandatory.** Without it,
`esp_ota_mark_app_valid_cancel_rollback()` is a silent no-op and rollback never
happens at all. Everything above still runs, correctly and pointlessly. The
failure mode is "OTA of a panicking build bricks the device", discovered at the
worst possible moment, so treat this flag as part of the feature rather than as
configuration. See §21 for the other sdkconfig settings with silent failure modes.

**An OTA in progress inhibits every deliberate reboot.** The wedge detector
(§15.1) and the Wi-Fi giveup timer (§15) both live in the 1 Hz sweep, which keeps
running while firmware streams in, and `esp_restart()` mid-write corrupts the
update. An `ota_in_progress` flag in `Hot` suppresses both.

The flag **must clear on every exit path**, including a dropped connection, a bad
token, a short body or a write error. One left set by an aborted upload disables
the wedge detector and the Wi-Fi giveup reboot *permanently and invisibly*, until
someone power-cycles the box — strictly worse than the corruption it prevents. So
it is set and cleared by an RAII guard whose `Drop` does the clearing, never by
paired manual assignments, and the early-return behaviour is a host test.
`wedge.rs` stays pure: the flag is an *input* to the verdict function, so "OTA
suppresses the wedge" is host-testable like the rest of §15.1.

**Validation deliberately does not require Wi-Fi.** A build with wrong
credentials will satisfy the unknown-advert clause within seconds, mark itself
valid at 60 s, then reboot-loop on `WIFI_GIVEUP` forever — with no `/ota` to push
a fix to and rollback permanently unavailable, because the image verified itself
before the fault could express. Gating validation on "has held an IP at least
once" would fix that and cost three lines. It is **rejected** here only because
this device is physically accessible and a bad push is noticed within minutes of
flashing. If it is ever deployed somewhere that needs a ladder, turn the gate on;
the failure it prevents is otherwise unrecoverable remotely.

## 14. Wi-Fi

- STA mode, DHCP, with a **reservation on the router** — this is the whole of
  the addressability story and the reason push-based metrics were rejected.
- Set the netif/DHCP hostname. **No mDNS responder** — it is an extra managed
  component and extra RAM for nothing, given that the bullet above already makes
  the router reservation the whole addressability story. The hostname alone gets
  router-side name resolution.
- Reconnect with exponential backoff: 1, 2, 4, … capped at 60 s.
- `esp_wifi_set_ps(WIFI_PS_NONE)` (§4.1).
- Export RSSI from `esp_wifi_sta_get_ap_info()`.

## 15. Failure and recovery

A reboot costs almost nothing here — it produces a ~3 second gap, which is a
signal you explicitly want anyway. Recovery can therefore be more decisive than
is usual on embedded.

| Condition | Action |
|---|---|
| Thread hang | `esp_task_wdt` on the housekeeping thread → reboot (see below) |
| Panic | reboot; OTA rollback if inside the validation window |
| Wi-Fi down | backoff reconnect; unrecoverable for **5 min** → `esp_restart()` |
| Radio wedge | see below → `esp_restart()` |

**The BLE path cannot be watchdogged directly.** `esp_task_wdt` requires a task
to subscribe and then feed it from its own loop. We do not own the NimBLE host
task's loop — the stack does, and we only receive callbacks from it. Subscribing
it is not possible, and a callback-driven task has no natural feed point anyway.

So the split is: **the housekeeping thread is watchdogged, and it is also what
detects BLE liveness.** It feeds the TWDT once per 1 Hz sweep, so if it hangs the
watchdog reboots us. And because the wedge detector (§15.1) already runs in that
sweep, a silent or wedged BLE stack is caught by the wedge condition rather than
by a watchdog — which is the better instrument for it regardless, since a wedged
NimBLE task is typically alive and looping, just not delivering. A watchdog would
never have fired on it.

The HTTP task is owned by `esp_http_server` and is likewise not subscribed; a
hung request handler is bounded by the socket timeout instead (§6.2).

Both deliberate reboot paths in this table — Wi-Fi giveup and radio wedge — are
suppressed while an OTA is streaming (§13).

**The watchdog must be running before anything that can hang.** A deadlock during
Wi-Fi or NimBLE bring-up, before the housekeeping thread subscribes to the TWDT,
has neither a watchdog to reboot it nor therefore a reboot to trigger OTA
rollback: the device simply stops, with a good slot sitting unused. §6.2 fixes
the boot order accordingly.

### 15.1 The wedge detector

Naively, "no beacons from any configured device for 10 minutes" means a dead BLE
stack. But it could also mean every sensor genuinely died. Disambiguate using
stray traffic:

> **Reboot only if all configured devices have been silent for 10 minutes AND
> `unknown_adverts_total` has not increased over that window.**

A home is full of stray BLE — phones, watches, TVs, neighbours. If unknown
adverts are still arriving, the radio is demonstrably alive and the sensors are
genuinely the problem; rebooting would be wrong and would thrash. If unknown
adverts have *also* flatlined, the radio is wedged. This AND-condition removes
almost all false-positive reboots.

**The detector does not arm until at least one advert of any kind — configured
device or unknown — has arrived since boot.** Without that, both conditions are
trivially true at boot and a board in a quiet RF environment reboots every ten
minutes forever. §13's OTA validation grew three escape hatches for exactly this
scenario; the wedge detector needs one too, and this is also the more honest
semantics: a wedge is a transition from working to not working, and you cannot
detect a stall in something that never started.

The cost is that a radio dead from boot is never caught here. That case shows as
`unknown_adverts_total` pinned at 0 forever, which is trivially alertable, and a
reboot would not have fixed it anyway.

Alert on `increase(ble_harvester_reboot_total[1h]) > 3` to catch a cycling box.

## 16. Observability

There is no SSH, no `journalctl`, and no log file. Do not write per-beacon logs
to flash: at the observed ~0.6 beacons/sec across three sensors that is roughly
50,000 lines and 4 MB per day, which destroys flash and blocks the radio
callback with multi-millisecond writes.

- **In-RAM log ring**, `LOG_LINES` × `LOG_LINE_BYTES` ≈ 12 KB, served at `/logs`,
  newest first, each line stamped with an absolute time once SNTP has synced and
  a relative age before that. Lost on reboot, by design.

  Two implementation notes. The absolute stamp needs a small hand-rolled
  epoch→civil-date formatter, since `harvester-core` is `no_std`; it is about
  twenty lines and host-testable. And a line is composed directly into its fixed
  96-byte slot with no allocation and no `format!` — §6.2's rule 1 forbids
  formatting in the BLE callback, and the failure paths that write here run from
  it.
- **Serial** (`esp_println!` → UART0) mirrors the ring when plugged in.
- **Counters** in §11.2 carry everything needed for alerting and trends.

`/debug/adverts` was considered and **cut** — sensors are flashed and configured
by hand, so their MACs are known before the firmware is built and there is no
discovery workflow to support. The `unknown_adverts_total` counter it would have
fed is retained, because it turned out to serve the wedge detector (§15.1).

## 17. Testing

### 17.1 Tiers

| Tier | Where | Automated | Scope |
|---|---|---|---|
| Unit | host, `cargo test` | yes | `harvester-core`: parse, decrypt, object walk, malformed input |
| Integration | host, `cargo test` | yes | registry, staleness, gap emission, exposition rendering, `decrypt_fail` / `parse_fail` / wedge paths |
| E2E | real hardware | no — checklist §17.3 | radio, coex, Wi-Fi, OTA, watchdog |

The `harvester-core` crate compiles for the host, so unit and integration tiers run in
milliseconds on a laptop and in CI with no hardware attached. **The failure
paths the hardware checklist cannot trigger on demand — `decrypt_fail`,
`parse_fail`, out-of-range readings, the novel-object bail, the OTA-suppression
input, and the wedge detector including its arming rule — are explicitly assigned
to the host integration tier and must be covered there.**

Staleness and gap behaviour is tested against a fake `Clock`, not by sleeping.

**Negative and boundary fixed-point rendering is a required unit test** and the
corpus cannot supply it: every captured temperature is positive (§17.2), while
the freezer sensor makes sub-zero a production path (§10.1). Cover −0.05, −9.00,
−40.00 and the zero-padding cases (`.01` vs `.10`, `-0.5` → `-0.50`).

### 17.2 Fixture corpus — captured

`harvester-core/tests/fixtures/corpus.json` — 33 frames, ~10 KB, no duplicate
payloads. Safe to commit: all identities are synthetic. The rest of this section
records how it was produced and what it does and does not cover.

**The corpus needs diversity, not volume.** ATC firmware emits a fixed two-frame
rotation, so the full vocabulary is exhausted in seconds:

```
40 00 05  01 5C  02 88 0A  03 9B 14     packet_id, battery, temp, humidity
40 00 05  0C 7B 0B  10 00  11 01        packet_id, voltage, 0x10, 0x11
```

At the observed ~0.6 beacons/sec across three sensors, a multi-day capture would
be ~150,000 frames that are almost entirely duplicates. Don't do that.

**The tool already exists and is tested:** `scripts/capture_corpus.py` in the
Python repo, with 15 passing tests. It captures, reduces and scrubs in one pass,
so the developer receives `corpus.json` as a finished artifact and needs none of
this pipeline.

```
python scripts/capture_corpus.py --config config.yaml --duration 600
```

Roughly 10 minutes per sensor sees every shape hundreds of times. It scans
**passively** (BlueZ `or_pattern` on `0xFCD2`), falling back to active with a
warning — active scanning transmits `SCAN_REQ` and drains the sensors' coin
cells for nothing (§9).

**Reduction** produces `harvester-core/tests/fixtures/corpus.json`, keyed by
*frame shape* — `(device_info, [object_id...], unknown_object_id)`. Identical
raw payloads collapse to one; within each shape the frames carrying the minimum
and maximum of every numeric field are always kept so boundary values survive.
Target size is **50–200 frames**: small
enough to read, diff and review, and fast enough to run on every `cargo test`.

**Each frame's `expected` values are derived from §7 of this document, not from
the Python parser.** The capture tool implements the specified whitelist decode
itself (`spec_parse`), so `expected.battery` is object `0x01`'s value verbatim
and `expected.voltage` is the raw `0x0C` reading. It deliberately does not reuse
`ble_exporter.parser.parse_bthome`, which would have written the old
voltage-curve battery figure into the fixtures and left the Rust implementation
failing tests for being correct.

The corpus is therefore a **statement of required behaviour**, not a recording of
the old implementation's behaviour. Nothing in the new repo compares itself
against Python; `corpus.json` is the only artifact that crosses over, and it
crosses as data.

**Encrypted frames:** decrypted with the real bindkey, then re-encrypted under a
**synthetic MAC and bindkey**, preserving the original `device_info` and counter
so frames stay structurally identical. The frames stay real; the secrets never
enter the repo.

Synthetic MACs are **index-derived, never hashed from the real address**: these
sensors share a fixed OUI, so a hash would leave only 24 unknown bits and be
trivially brute-forceable.

**Synthesize, don't wait for, the adversarial cases.** Truncated payloads, a
corrupted MIC, wrong version bits, empty service data, out-of-range readings,
negative temperatures — these are constructed in unit tests. Waiting to *capture*
RF corruption is what would need days, and it buys nothing a constructed frame
doesn't already prove.

#### What the captured corpus actually contains

| | |
|---|---|
| Frames | 33, no duplicate `frame_hex` |
| Identities | `baby_room` / `humidor` plaintext, `living_room` encrypted with bindkey `0202…02` |
| Shapes | `[00 01 02 03]`, `[00 0C 10 11]`, `[0C 10 11]`, `[01 02 03]`, `[11] UNKNOWN=0x3E` |
| Ranges | temperature 23.65…28.62 °C · humidity 38.15…61.99 % · battery 48…100 % · voltage 2.586…3.175 V |

Four things to know before writing the harness:

- **`0x00` packet_id is optional** — two of the five shapes omit it. Do not
  assume a frame begins with it.
- **`expected` omits absent fields** rather than nulling them → `Option` plus
  `#[serde(default)]`.
- **`expected` values are pre-scaled floats** (`23.65`, not `2365`). Core holds
  integers (§6.1), so the harness compares
  `(expected * 100.0).round() as i16` against the stored value. Floats live in
  the test, which is `std`, and never enter core.
- **`shape` reports `info=40` for frames whose raw `device_info` is `0x41`** —
  the encryption bit is masked off in the label. Do not assert `shape` against
  the raw byte.

**Every captured temperature is positive**, so the corpus proves nothing about
sign handling; see §17.1. The one novel object id it contains, `0x3E`, is now on
the §7 whitelist with its length measured from that frame.

### 17.2.1 Shape watcher — how strong the whitelist claim actually is

Be precise about this, because it is easy to over-read. The §7 whitelist is
supported by a 33-frame reduced corpus plus roughly two hours of observation
across all sensors, during which no shape beyond those in §17.2 appeared. It is
**not** backed by a long soak, and it is not proven closed. A 48-hour watcher was
considered and skipped.

That is a deliberate and cheap bet, because being wrong is cheap. A novel object
id is exactly what §7's bail-on-unknown behaviour exists to absorb: the walk
stops, partial readings are kept, `parse_unknown_object_total` increments and one
line lands in the ring. No garbage reaches Prometheus. `0x3E` was found and
handled precisely this way.

**The standing procedure, then, is a metric rather than a capture job.**
`parse_unknown_object_total` should sit at 0. If it starts climbing, a sensor is
emitting an id we do not know: read `/logs` for the offending id, determine its
length from the BTHome v2 object table, confirm the length against a captured
frame, add it to §7 deliberately, and push. Never guess a length — §7 explains
what a wrong one does.

### 17.3 Pre-release hardware checklist

```
[ ] all configured devices appear in /metrics within 2 min of boot
[ ] promtool check metrics accepts /metrics  (catches §11.4 ordering)
[ ] pull a CR2032 → that device's gauge series vanish within 90s
[ ] ble_sensor_seen for that device reads 0, others unaffected
[ ] that device's beacons_total series are still PRESENT and flat  (§3)
[ ] reinsert → series return
[ ] /  renders on mobile; sparklines populate; ages tick
[ ] diagnostics section shows plausible rssi and beacons/min per device
[ ] freezer sensor: readings survive the metal box, or the timeout gets
    re-derived from its measured rate  (§10.1)
[ ] OTA push succeeds; build_info reports the new git_sha
[ ] OTA from a dirty tree → build_info reports -dirty, push_ota.sh fails
[ ] OTA a deliberately panicking build → auto-rollback to previous slot
[ ] abort an OTA mid-upload → wedge detector and wifi giveup still armed
[ ] boot a good build with NO sensors in range → marks valid, does NOT loop
[ ] force a wedge reboot → reboot_total{reason="ble_wedge"} increments,
    not reason="unknown"
[ ] pull Wi-Fi for 6 min → reboot; reboot_total{reason="wifi"} increments
[ ] 24h soak: min_free_heap flat, reboot_total unchanged, no gaps
```

**Forcing a wedge needs a plan.** With the arming rule (§15.1) it cannot be
induced on demand: the detector wants adverts to have arrived and then stopped.
Flash a build with `WEDGE_WINDOW` cut to ~60 s, let it hear traffic, then put the
device somewhere RF-quiet enough that everything goes silent. What is being
tested is the reason-labelling path, not the timeout arithmetic.

`scripts/e2e.sh` automates the boring repeatable subset — boot, wait, assert
every configured device is in `/metrics`, assert `build_info` matches the
expected sha, assert `/healthz` — because it gets run after every OTA push rather
than once per release. The rest of the list stays manual, which is honest: it
needs a coin cell, a phone screen and a day.

## 18. Dashboard

Static HTML+CSS+JS embedded in flash via `EMBED_FILES`, served gzipped with an
`ETag`. **Never assemble the page in RAM per request** — the Python version
rebuilds ~1.5 KB of CSS on every hit.

The page consumes **two** endpoints on **two** cadences (§12), both fetches
**gated on `document.visibilityState`** so a forgotten tab stops polling instead
of hitting the device forever:

| Endpoint | Size | Cadence |
|---|---|---|
| `/api/readings` | ~1 KB | every 15 s |
| `/api/history` | ~7 KB | once on load, then every `SPARK_SAMPLE_INTERVAL` |

Refetching 7 KB of history every 15 s to update three temperatures would be
almost entirely waste; the history only changes when a new sample is appended. The current implementation's
`<meta http-equiv="refresh" content="60">` does exactly that and must not be
carried over.

Three sections:

1. **Readings** — per device: name, lock icon if encrypted, temperature,
   humidity, and a **relative** age ("4s ago"). Offline devices render as
   `OFFLINE 6m` with values struck out, never as a silently stale number.
   Relative ages mean the page is correct even before SNTP syncs.
2. **Sparklines** — inline SVG per device from an on-device ring: one sample
   every 5 minutes, `SPARK_DEPTH`(72) deep ≈ 6 hours of history, `i16` per
   metric. Eight devices × 72 × 2 ≈ **2.3 KB of RAM**. This is what makes the
   page useful with Prometheus and Grafana entirely absent.

   **`i16::MIN` is the missing-sample sentinel**, emitted as JSON `null`, and the
   polyline breaks there. The ring needs a no-sample concept regardless — on a
   fresh boot 71 of 72 slots are unfilled, and a zero-filled ring would draw six
   hours of 0 °C — so one sentinel covers both "not yet filled" and "device was
   offline when sampled", which is less code than a fill counter plus zeroed
   gaps. It also keeps §3's whole point from being quietly undone in the
   dashboard: a flat line through an outage is exactly the misreading the gap
   model exists to prevent. `i16::MIN` is unreachable as a real value, being
   −327.68 °C.
3. **Diagnostics** (collapsible) — uptime, Wi-Fi RSSI, free and minimum-free
   heap, reboot count and last reason, unknown advert count, and a per-device
   table of RSSI and beacons/min. This table is the fastest answer to "is the
   antenna good enough".

   **beacons/min is computed on the device**, from a per-device ring of 60
   one-second buckets advanced by the 1 Hz sweep (§6.2). It is a rate, and the
   page has only one sample, so it cannot be derived from `beacons_total`
   client-side — and this table has to work with Prometheus absent (§20.4). An
   exact trailing-60 s count also responds immediately to moving the antenna,
   where an average-since-boot figure would take hours to settle, and answering
   that question quickly is the reason the table exists.

## 19. Cutover

Run both implementations in parallel for at least 7 days before retiring
anything. Identical metric names mean Prometheus separates them natively by
`instance`:

```yaml
- job_name: ble_exporter
  static_configs:
    - targets: ['pi.lan:8183', 'harvester.lan:8183']
```

Compare in Grafana:

- `count by (instance) (ble_sensor_seen == 1)` — does the C3 see every device?
  (`ble_sensor_seen` without the `== 1` counts *series*, and offline devices keep
  a series at `0` per §3, so it returns the configured device count on both
  instances no matter what and tells you nothing)
- `ble_sensor_rssi_dbm` by instance — how much margin was lost?
- `rate(ble_harvester_beacons_total{result="ok"}[5m])` — is it missing beacons?

**Calibrating the target:** the Pi as it runs today is the bar, per acceptance
criterion 6. Its reception is somewhat degraded by an attached USB3 SATA bridge —
SuperSpeed signalling is a well-documented 2.4 GHz broadband noise source — so
this is a slightly soft target, and reconstructing a cleaner historical baseline
was considered and dropped as not worth the effort for a home thermometer.

The harvester has no SuperSpeed anything nearby and will sit in a better spot, so
it may well beat the Pi outright despite a smaller antenna. If a device does come
up short, the IPEX connector takes an external whip — see §4.1 on enabling it.
Judge the freezer sensor on its own terms (§10.1); it is in a metal box and will
be the worst performer on both instances.

Retire the Pi and archive the Python repo once the parallel run is flat.

## 20. Acceptance criteria

1. `cargo test` at the repo root passes on the host with **no hardware attached
   and no ESP toolchain installed** (§6.1 — this is why `harvester/` is a
   separate workspace), and
   every frame in the fixture corpus parses identically to the recorded output.
2. A device losing power has its series removed from `/metrics` within
   `DEVICE_TIMEOUT + 5 s`, and `ble_sensor_seen` reads 0.
3. Existing Grafana panels render against the ESP32 with no query changes.
4. `/` loads and is legible on a phone with Prometheus and Grafana both stopped.
5. OTA of a panicking build auto-rolls-back without physical access.
6. 7-day soak: `min_free_heap` flat, no unexplained reboots, and per-device
   `beacons/min` within 20% of the Pi running in parallel.
7. No real MAC, bindkey, PSK or token appears anywhere in git history.

## 21. Risks

| Risk | Mitigation |
|---|---|
| Wi-Fi/BLE coex starvation | 75% scan duty; monitor `beacons/min` and scrape latency; reduce window if needed |
| Antenna underperforms | measured during parallel run; IPEX + external whip as the escape hatch |
| `esp32-nimble` API churn | pin versions; all snippets here are indicative |
| Tier-3 target build friction | start from `esp-idf-template`, do not hand-roll `.cargo/config.toml` |
| Root `.cargo/config.toml` hijacking host tests | `harvester/` is its own workspace (§6.1); verify by running `cargo test` at the root on a machine with no ESP toolchain |
| A good build boot-looping on a bench board | OTA validation has an unknown-advert clause and a hard deadline (§13) |
| httpd stack overflow under load | `stack_size = 10240`, not the 4096 default (§6.2); presents as `reboot_total{reason="panic"}` climbing whenever the dashboard is opened |
| Reboot reasons collapsing to `ESP_RST_SW` | pending-reason written to NVS before every intentional restart (§11.2) |
| C6→C3 surprises | write for the C3 while building for the C6; see §4.1's banned list, especially the 400 KB SRAM ceiling |
| **Rollback silently disabled** | `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y` (§13). Without it `mark_app_valid` is a no-op and §13 runs correctly and pointlessly |
| Other sdkconfig flags failing silently | `CONFIG_BT_ENABLED` / `CONFIG_BT_NIMBLE_ENABLED` with observer role only; `CONFIG_ESP_COEX_SW_COEXIST_ENABLE=y` (§9's duty-cycle reasoning assumes the arbiter runs); `CONFIG_ESP_TASK_WDT_*` (§15); custom partition table (§13) |
| Whitelist turns out to be incomplete | bail-on-unknown absorbs it without emitting garbage; watch `parse_unknown_object_total` and follow §17.2.1 |
| Panic in the parser reboots the device | `harvester-core` denies panicking constructs by lint (§6.1); release-mode integer wrap is denied by the same rule |
| Aborted OTA disarms recovery | `ota_in_progress` cleared by an RAII guard on every exit path (§13) |
| Wi-Fi credentials wrong in a pushed build | accepted: physically accessible device, noticed within minutes. §13 records the gate to enable if that changes |
| Heap fragmentation over months | `min_free_heap_bytes` exported; alert on downward trend rather than papering over it with a scheduled reboot |

## 22. Decision log

Recorded so these are not re-litigated later.

| # | Decision | Rationale |
|---|---|---|
| 1 | Rust `std` on ESP-IDF, not `no_std` | ESP-IDF's coex arbiter is the mature path for simultaneous Wi-Fi + BLE; `no_std` coex is younger and `riscv32imc` lacks atomics |
| 2 | Compile-time config | ≤8 devices, changes are rare, and OTA makes a rebuild cheap; avoids a provisioning UI and a mutation endpoint entirely |
| 3 | OTA from day one | partition layout is painful to change later; makes compile-time config viable |
| 4 | Metrics + `/logs` ring, no `/debug/adverts` | MACs are known from manual flashing; no discovery workflow exists |
| 5 | Watchdog + AND-gated wedge detector | reboots are cheap given the gap philosophy; stray BLE disambiguates dead radio from dead sensors |
| 6 | Host-automated tests + manual hardware checklist | a companion beacon transmitter was considered and rejected as a second firmware to maintain |
| 7 | Pull, not push | push does not fix a flaky link, and it forfeits `up{}`; buffering/backfill was the only real advantage and DHCP reservation is the cheaper fix |
| 8 | Full dashboard with sparklines | ~2.3 KB of RAM makes the page independently useful |
| 9 | Clean-slate repo | the Python app is replaced outright; only `corpus.json` crosses over, as data |
| 10 | Gaps, not PromQL staleness | see §3 — deliberate observability preference |
| 11 | Crates split by purity, not subject | §17.1 requires host coverage of the wedge detector and exposition rendering; both are pure decisions wearing hardware costumes (§6.1) |
| 12 | Two workspaces | a root ESP target would make acceptance criterion 1 unsatisfiable (§6.1) |
| 13 | Tunables committed, secrets gitignored | §10.1 asks for the timeout to be re-derived from measurements; that history has to live in git (§5) |
| 14 | `battery_percent` keeps its name despite a value break | the panel keeps working and the break is annotated, rather than orphaning history under a new name (§11.1) |
| 15 | Capacities in core, tunables injected, devices passed as a slice | core is allocation-free and must own its array sizes; §10.1's tuning history must stay in git; secrets must not (§5) |
| 16 | `MAX_DEVICES = 8` | headroom over the current three at ~0.7 KB total cost; state was never the constraint on a 400 KB part (§6.2) |
| 17 | Counters never gap, and are zero-initialised | removing a counter series breaks `rate()` and `increase()`, disarming §19's comparison and §15.1's alert (§3) |
| 18 | `result` classifies intelligibility, not reading count | a valid frame with zero readings exists in the corpus; counting it `parse_fail` would misreport ~6% of a device's beacons forever (§7.1) |
| 19 | `0x3E` added to the whitelist, length measured | found in the corpus, MIC-valid, four bytes consuming the payload exactly; keeps `parse_unknown_object_total` an alarm rather than noise (§7) |
| 20 | Counter regressions neither enforced nor counted | nonce reuse is visible in the corpus, so the metric would fire constantly and say nothing (§8) |
| 21 | Wedge detector must arm before it can fire | at boot both conditions are trivially true, so a quiet RF environment would reboot forever (§15.1) |
| 22 | No floats and no panics in core, by lint | no FPU on either chip; a panic is a reboot and this is the untrusted-input crate (§6.1) |
| 23 | Metric-major exposition with `HELP`/`TYPE` | device-major emits a duplicate `TYPE` and fails the entire scrape as "target down" (§11.4) |
| 24 | Range-check readings | humidity is `u16` on the wire and `i16` in the ring; plaintext devices are unauthenticated (§7.2) |
| 25 | Watchdog subscribed before Wi-Fi and BLE start | a hang during bring-up has neither a watchdog nor a reboot to trigger rollback (§6.2) |
| 26 | E2E stays a manual checklist plus a small smoke script | the assertions need a coin cell, a phone and a day; the repeatable subset is automated because it runs every push (§17.3) |
| 27 | OTA validation does not require Wi-Fi | explicitly accepted, given physical access; the gate and its failure mode are documented for a future remote deployment (§13) |
