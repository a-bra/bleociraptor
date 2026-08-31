#!/usr/bin/env bash
# ABOUTME: Post-flash smoke test (§17.3's automatable subset) — run after every
# ABOUTME: OTA push. Usage: scripts/e2e.sh <host> [expected_sha]
set -euo pipefail

HOST="${1:?usage: e2e.sh <host> [expected_sha]}"
BASE="http://${HOST}:8183"
FAILED=0
check() { # check <name> <cmd...>
    local name="$1"; shift
    if "$@" >/dev/null 2>&1; then echo "  ok: $name"; else echo "FAIL: $name"; FAILED=1; fi
}

echo "== $BASE =="
check "/healthz answers ok" bash -c "curl -fsS --max-time 5 $BASE/healthz | grep -qx ok"
check "/status has the Python shape" bash -c \
    "curl -fsS $BASE/status | grep -qE '^\{\"device_timeout_seconds\": [0-9]+, \"devices_active\": [0-9]+\}$'"
check "/metrics parses (build_info present)" bash -c \
    "curl -fsS $BASE/metrics | grep -q '^ble_harvester_build_info{'"
check "every family declares TYPE exactly once" bash -c \
    "curl -fsS $BASE/metrics | grep '^# TYPE' | sort | uniq -d | wc -l | grep -qx 0"
check "/api/readings is valid JSON" bash -c \
    "curl -fsS $BASE/api/readings | python3 -m json.tool"
check "/api/history is valid JSON with depth 72" bash -c \
    "curl -fsS $BASE/api/history | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d[\"depth\"]==72'"
check "/logs answers" bash -c "curl -fsS $BASE/logs -o /dev/null"
check "/ota rejects a missing token" bash -c \
    "test \$(curl -s -o /dev/null -w '%{http_code}' -X POST $BASE/ota) = 401"
check "dashboard / serves gzip" bash -c \
    "curl -fsS --compressed $BASE/ | grep -qi '<html'"

if [ -n "${2:-}" ]; then
    check "running git_sha is $2" bash -c \
        "curl -fsS $BASE/metrics | grep -q 'git_sha=\"$2\"'"
fi

# Devices present within 2 min of boot (§17.3 line 1) — only meaningful if
# sensors are in range; report, don't fail.
SEEN=$(curl -fsS "$BASE/metrics" | grep -c '^ble_sensor_seen{.*} 1$' || true)
echo "  info: ${SEEN} device(s) currently online"

if command -v promtool >/dev/null; then
    check "promtool accepts the exposition" bash -c \
        "curl -fsS $BASE/metrics | promtool check metrics"
else
    echo "  skip: promtool not installed"
fi

[ "$FAILED" = 0 ] && echo "== all checks passed ==" || { echo "== FAILURES =="; exit 1; }
