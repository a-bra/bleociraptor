// ABOUTME: Render layer — produces bytes, never serves them (§6.1). Submodules
// ABOUTME: own one endpoint's body each; http.rs in the firmware owns sockets.

pub mod prometheus;

/// Harvester-level health snapshot, filled in by the firmware and passed to
/// the renderers. Core never probes hardware; these arrive as plain values.
#[derive(Debug, Clone, Copy)]
pub struct Health {
    pub uptime_seconds: u64,
    pub free_heap_bytes: u32,
    pub min_free_heap_bytes: u32,
    /// None while the AP is unreachable — the gauge gaps, like any other.
    pub wifi_rssi_dbm: Option<i8>,
    pub version: &'static str,
    pub git_sha: &'static str,
    pub reboots: RebootCounts,
    /// Reason recorded for the most recent boot (§11.2), for the dashboard.
    pub last_reboot_reason: &'static str,
}

/// All seven `reboot_total{reason}` series, exported unconditionally including
/// zeros (§3): a counter that springs into existence on first increment gives
/// `increase()` nothing to subtract from.
#[derive(Debug, Clone, Copy, Default)]
pub struct RebootCounts {
    pub panic: u32,
    pub wifi: u32,
    pub ble_wedge: u32,
    pub ota: u32,
    pub power: u32,
    pub wdt: u32,
    pub unknown: u32,
}
