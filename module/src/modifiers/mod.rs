mod resistance;
mod speed;
mod stats;

use lyracore_shared::spell::taxonomy::{
    A_COMBAT_HEALTH_REGEN_PCT, A_MOD_COMBAT, A_MOD_DETECT_RANGE, A_MOD_POWER_REGEN_PCT,
};
use spacetimedb::ReducerContext;

use crate::spell::auras_on;
use crate::Aura;

pub(crate) use resistance::{
    effective_armor, effective_resistance, positive_armor, resistance_bonus, ResistanceItem,
};
pub(crate) use speed::{
    effective_move_speed, holds_in_place, move_speed_with_pct, moves_run_speed, run_speed,
    speed_bonus, swim_speed,
};
pub(crate) use stats::{
    effective_stat, moves_sheet, moves_vitals, sheet_stat_bonus, SHEET_STATS, VITALS_STATS,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct AuraAmount {
    pub(crate) kind: u8,
    pub(crate) parameter: i32,
    pub(crate) amount: i32,
    pub(crate) stacks: u8,
}

impl AuraAmount {
    pub(crate) fn total(self) -> i32 {
        stacked_amount(self.amount, self.stacks)
    }
}

impl From<&Aura> for AuraAmount {
    fn from(aura: &Aura) -> Self {
        Self {
            kind: aura.eff_kind,
            parameter: aura.eff_p0,
            amount: aura.amount,
            stacks: aura.stacks,
        }
    }
}

// Legacy zero-stack rows contribute once.
pub(crate) fn stacked_amount(amount: i32, stacks: u8) -> i32 {
    amount * i32::from(stacks.max(1))
}

pub(crate) fn aura_amounts(
    ctx: &ReducerContext,
    unit_guid: u64,
) -> impl Iterator<Item = AuraAmount> {
    auras_on(ctx, unit_guid).map(|aura| AuraAmount::from(&aura))
}

pub(crate) fn aura_total(
    ctx: &ReducerContext,
    unit_guid: u64,
    pred: impl Fn(&Aura) -> bool,
) -> i32 {
    auras_on(ctx, unit_guid)
        .filter(|aura| pred(aura))
        .map(|aura| AuraAmount::from(&aura).total())
        .sum()
}

pub(crate) fn combat_field_bonus(ctx: &ReducerContext, unit_guid: u64, field: u8) -> i32 {
    aura_total(ctx, unit_guid, |a| {
        a.eff_kind == A_MOD_COMBAT && a.eff_p0 == i32::from(field)
    })
}

pub(crate) fn combat_health_regen_pct(ctx: &ReducerContext, unit_guid: u64) -> i32 {
    aura_total(ctx, unit_guid, |a| a.eff_kind == A_COMBAT_HEALTH_REGEN_PCT)
}

pub(crate) fn detect_range_mod(ctx: &ReducerContext, unit_guid: u64) -> f32 {
    aura_total(ctx, unit_guid, |a| a.eff_kind == A_MOD_DETECT_RANGE) as f32
}

// 1.0 leaves the base regeneration unchanged.
pub(crate) fn power_regen_rate(ctx: &ReducerContext, unit_guid: u64, power_type: u8) -> f64 {
    compound_regen_rate(
        auras_on(ctx, unit_guid)
            .filter(|a| {
                a.eff_kind == A_MOD_POWER_REGEN_PCT
                    && a.eff_p0 == i32::from(power_type)
                    && !crate::spell::is_due_for_expiry(
                        a.eff_kind,
                        a.expires_at,
                        crate::time::now(ctx),
                    )
            })
            .map(|a| AuraAmount::from(&a).total()),
    )
}

// Each aura multiplies the rate, as cmangos `GetTotalAuraMultiplierByMiscValue` does.
fn compound_regen_rate(amounts: impl Iterator<Item = i32>) -> f64 {
    amounts.fold(1.0, |rate, amount| {
        rate * (100.0 + f64::from(amount)).max(0.0) / 100.0
    })
}

#[cfg(test)]
mod tests {
    use super::{compound_regen_rate, stacked_amount};

    #[test]
    fn power_regen_auras_multiply_in_any_order_and_floor_at_zero() {
        assert_eq!(compound_regen_rate([].into_iter()), 1.0);
        assert_eq!(compound_regen_rate([100].into_iter()), 2.0);
        assert_eq!(compound_regen_rate([100, 50].into_iter()), 3.0);
        assert_eq!(compound_regen_rate([-150].into_iter()), 0.0);
        let energy = |amounts: [i32; 3]| (20.0 * compound_regen_rate(amounts.into_iter())) as u32;
        assert_eq!(energy([-25, -10, 100]), 27);
        assert_eq!(energy([100, -25, -10]), 27);
        assert_eq!(energy([-10, 100, -25]), 27);
    }

    #[test]
    fn stacked_amount_compounds_with_stacks() {
        assert_eq!(stacked_amount(-90, 5), -450);
        assert_eq!(stacked_amount(-90, 1), -90);
        assert_eq!(stacked_amount(50, 1), 50);
        assert_eq!(stacked_amount(50, 0), 50);
    }
}
