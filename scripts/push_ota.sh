#!/usr/bin/env bash
# ABOUTME: Build, push OTA, wait for reboot, verify the device runs the new sha.
# ABOUTME: Usage: scripts/push_ota.sh <host> — needs OTA_TOKEN in env or harvester/src/devices.rs
set -euo pipefail

HOST="${1:?usage: push_ota.sh <host, e.g. harvester.lan>}"
cd "$(dirname "$0")/../harvester"

# --dirty is load-bearing (§13): a dirty tree reports abc1234-dirty and the
# verification below fails honestly instead of passing on uncommitted code.
EXPECTED_SHA=$(git describe --always --dirty)

# Token: env wins; otherwise pull it from the gitignored devices.rs.
TOKEN="${OTA_TOKEN:-$(sed -n 's/^pub const OTA_TOKEN: &str = "\(.*\)";$/\1/p' src/devices.rs)}"
[ -n "$TOKEN" ] || { echo "no OTA token found (env OTA_TOKEN or src/devices.rs)"; exit 1; }

echo "building ${EXPECTED_SHA}…"
cargo build --release
espflash save-image --chip esp32c6 \
    target/riscv32imac-esp-espidf/release/harvester /tmp/harvester-ota.bin

echo "pushing to ${HOST}…"
curl -fsS -X POST "http://${HOST}:8183/ota" \
    -H "X-Auth: ${TOKEN}" \
    --data-binary @/tmp/harvester-ota.bin

echo "waiting for reboot…"
sleep 8
for i in $(seq 1 30); do
    RUNNING=$(curl -fsS --max-time 2 "http://${HOST}:8183/metrics" 2>/dev/null \
        | sed -n 's/.*git_sha="\([^"]*\)".*/\1/p' | head -1) || true
    if [ "$RUNNING" = "$EXPECTED_SHA" ]; then
        echo "OK: ${HOST} is running ${RUNNING}"
        exit 0
    fi
    sleep 2
done
echo "FAIL: device did not come back with ${EXPECTED_SHA} (last seen: ${RUNNING:-unreachable})"
echo "note: if the new build is bad, rollback restores the previous slot on its own (§13)"
exit 1
