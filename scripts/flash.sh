#!/usr/bin/env bash
# ABOUTME: Build and flash the harvester over USB with the §13 partition table.
# ABOUTME: Usage: scripts/flash.sh [--monitor]  (run from anywhere in the repo)
set -euo pipefail
cd "$(dirname "$0")/../harvester"
cargo build --release

# A USB flash writes ota_0 but never touches otadata — after any OTA push the
# active slot may be ota_1, and the bootloader would keep booting the old image
# there, ignoring what was just flashed (§13). Erasing otadata makes the
# bootloader fall back to ota_0, i.e. makes THIS flash authoritative. It also
# clears any pending-verify state, which is correct for a physical flash.
espflash erase-parts --partition-table partitions.csv otadata

exec espflash flash --partition-table partitions.csv \
    ${1:---monitor} target/riscv32imac-esp-espidf/release/harvester
