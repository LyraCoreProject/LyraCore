pub(crate) const MOVE_TICK_MICROS: i64 = 500_000;

pub(crate) const GLOBAL_TICK_INSTANCE: u64 = u64::MAX;

pub(crate) const TICK_SLICES: u32 = 4;

pub(crate) const SLICED_TICK_MICROS: i64 = MOVE_TICK_MICROS / TICK_SLICES as i64;
