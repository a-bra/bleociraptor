#!/usr/bin/env bash
# ABOUTME: Build and flash the harvester over USB with the §13 partition table.
# ABOUTME: Usage: scripts/flash.sh [--monitor]  (run from anywhere in the repo)
set -euo pipefail
cd "$(dirname "$0")/../harvester"
cargo build --release
exec espflash flash --partition-table partitions.csv \
    ${1:---monitor} target/riscv32imac-esp-espidf/release/harvester
