// ABOUTME: harvester-core — every decision in the harvester, host-testable.
// ABOUTME: no_std, allocation-free, float-free, panic-free by lint (SPEC.md §6.1).

#![no_std]
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects
)]

/// Capacity constants (§5.1). Core is allocation-free, so it owns the sizes of
/// its fixed arrays. Behavioural tunables are injected via `Tunables` instead.
pub const MAX_DEVICES: usize = 8;
pub const SPARK_DEPTH: usize = 72;
pub const LOG_LINES: usize = 128;
pub const LOG_LINE_BYTES: usize = 96;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_budget_stays_inside_spec_table() {
        // §6.2 budgets ~15 KB total; Logs dominate at ~12 KB.
        assert_eq!(LOG_LINES * LOG_LINE_BYTES, 12_288);
        assert!(MAX_DEVICES * SPARK_DEPTH * 2 * core::mem::size_of::<i16>() <= 2_304);
    }
}
