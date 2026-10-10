//! Talent reset pricing shared by the Module and the confirmation dialog.

pub fn respec_cost_copper(respec_count: u32) -> u32 {
    match respec_count.min(10) {
        0 => 10_000,
        1 => 50_000,
        n => 100_000 + 50_000 * (n - 2),
    }
}
