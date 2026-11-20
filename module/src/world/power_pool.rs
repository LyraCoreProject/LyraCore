use lyracore_shared::packing::power_type;
use lyracore_shared::spell::taxonomy::P_POWER_TYPE;

use super::WorldEntity;

/// `WorldEntity.reserve_power_type` of a unit that holds no Reserve Pool.
pub(crate) const NO_RESERVE_POOL: u8 = u8::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PowerPool {
    pub power_type: u8,
    pub current: u32,
    pub max: u32,
}

/// Whether `power_type` names a pool the Module keeps: mana, rage, focus, energy or happiness.
pub(crate) fn is_power_type(power_type: u8) -> bool {
    power_type <= power_type::HAPPINESS
}

/// vmangos `Unit::GetCreatePowers` for a Character: a pool a Form shows starts empty at this max.
fn form_pool_max(power_type: u8) -> u32 {
    match power_type {
        power_type::RAGE => 1000,
        power_type::ENERGY => 100,
        _ => 0,
    }
}

impl WorldEntity {
    pub(crate) fn shown_pool(&self) -> PowerPool {
        PowerPool {
            power_type: self.power_type(),
            current: self.power,
            max: self.max_power,
        }
    }

    pub(crate) fn reserve_pool(&self) -> Option<PowerPool> {
        (self.reserve_power_type != NO_RESERVE_POOL).then_some(PowerPool {
            power_type: self.reserve_power_type,
            current: self.reserve_power,
            max: self.reserve_max_power,
        })
    }

    /// The pool of the unit's own power type, held as the Reserve Pool while a Form shows another.
    pub(crate) fn own_pool(&self) -> PowerPool {
        self.reserve_pool().unwrap_or_else(|| self.shown_pool())
    }

    pub(crate) fn set_own_pool(&mut self, current: u32, max: u32) {
        if self.reserve_pool().is_some() {
            self.reserve_power = current;
            self.reserve_max_power = max;
        } else {
            self.power = current;
            self.max_power = max;
        }
    }

    /// The unit's pool of `power_type`, shown or reserved.
    pub(crate) fn pool(&self, power_type: u8) -> Option<PowerPool> {
        [Some(self.shown_pool()), self.reserve_pool()]
            .into_iter()
            .flatten()
            .find(|pool| pool.power_type == power_type)
    }

    fn pool_mut(&mut self, power_type: u8) -> Option<(&mut u32, u32)> {
        if self.power_type() == power_type {
            Some((&mut self.power, self.max_power))
        } else if self.reserve_power_type == power_type {
            Some((&mut self.reserve_power, self.reserve_max_power))
        } else {
            None
        }
    }

    /// Adds `amount` to the unit's pool of `power_type`, up to its max; a unit without one gains none.
    pub(crate) fn gain_power(&mut self, power_type: u8, amount: u32) {
        if let Some((current, max)) = self.pool_mut(power_type) {
            *current = current.saturating_add(amount).min(max);
        }
    }

    /// The pool an energize reaches: the one its `P_POWER_TYPE` parameter names, else the shown
    /// pool. None when the unit holds no such pool or its max is 0, as vmangos
    /// `Spell::EffectEnergize` skips it.
    pub(crate) fn energized_pool(&self, p0_kind: u8, p0: i32) -> Option<PowerPool> {
        let pool = if p0_kind == P_POWER_TYPE {
            self.pool(u8::try_from(p0).ok()?)?
        } else {
            self.shown_pool()
        };
        (pool.max > 0).then_some(pool)
    }

    /// Sets each pool the unit holds, shown or reserved, to `current(pool)`, capped at its max.
    pub(crate) fn set_each_pool(&mut self, current: impl Fn(PowerPool) -> u32) {
        let shown = self.shown_pool();
        self.power = current(shown).min(shown.max);
        if let Some(reserve) = self.reserve_pool() {
            self.reserve_power = current(reserve).min(reserve.max);
        }
    }

    /// The power a cost in `power_type` can spend: its pool's, shown or reserved, and 0 for a pool
    /// the unit lacks. A health cost can spend all but the last point, as vmangos
    /// `Spell::CheckPower` refuses it at or below the cost.
    pub(crate) fn spendable_power(&self, power_type: u8) -> u32 {
        if power_type == power_type::HEALTH {
            return self.health.saturating_sub(1);
        }
        self.pool(power_type).map_or(0, |pool| pool.current)
    }

    /// Pays a cost from the pool of its power type, or from health for a health cost, never below
    /// 1; a unit without the pool pays nothing.
    pub(crate) fn spend_power(&mut self, power_type: u8, amount: u32) {
        if power_type == power_type::HEALTH {
            self.health = self.health.saturating_sub(amount).max(1);
        } else if let Some((current, _)) = self.pool_mut(power_type) {
            *current = current.saturating_sub(amount);
        }
    }

    /// Shows the unit's pool of `power_type`. The unit's own pool comes back from reserve; any
    /// other pool is a Form's, starts empty and ends when another pool is shown.
    pub(crate) fn show_pool(&mut self, power_type: u8) {
        if self.power_type() == power_type {
            return;
        }
        let own = self.own_pool();
        if power_type == own.power_type {
            self.power = own.current;
            self.max_power = own.max;
            self.reserve_power_type = NO_RESERVE_POOL;
            self.reserve_power = 0;
            self.reserve_max_power = 0;
        } else {
            self.reserve_power_type = own.power_type;
            self.reserve_power = own.current;
            self.reserve_max_power = own.max;
            self.power = 0;
            self.max_power = form_pool_max(power_type);
        }
        self.unit_bytes_0 = (self.unit_bytes_0 & 0x00FF_FFFF) | u32::from(power_type) << 24;
    }
}

/// vmangos `Player::ResurrectPlayer`: mana and energy return at `percent` of their max, rage at
/// zero, and any other pool keeps its value.
pub(crate) fn resurrected_power(pool: PowerPool, percent: u32) -> u32 {
    match pool.power_type {
        power_type::RAGE => 0,
        power_type::MANA | power_type::ENERGY => {
            (u64::from(pool.max) * u64::from(percent) / 100) as u32
        }
        _ => pool.current,
    }
}

/// vmangos `Player::ResurrectUsingRequestData`: mana returns at `percent` of its max, energy full,
/// rage at zero, and any other pool keeps its value.
pub(crate) fn request_resurrected_power(pool: PowerPool, percent: u32) -> u32 {
    match pool.power_type {
        power_type::MANA => resurrected_power(pool, percent),
        power_type::ENERGY => pool.max,
        _ => resurrected_power(pool, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::packing::{power_type::*, unit_bytes_0};

    const DRUID: u8 = 11;

    fn druid(mana: u32, max_mana: u32) -> WorldEntity {
        let mut unit = crate::helpers::tests::entity(7, 0, 0, 0, 0);
        unit.unit_bytes_0 = unit_bytes_0(4, DRUID, 0, MANA);
        unit.power = mana;
        unit.max_power = max_mana;
        unit
    }

    #[test]
    fn a_form_pool_reserves_the_own_pool_and_starts_empty() {
        let mut unit = druid(700, 1000);
        unit.show_pool(RAGE);
        assert_eq!(
            (unit.shown_pool(), unit.reserve_pool()),
            (
                PowerPool {
                    power_type: RAGE,
                    current: 0,
                    max: 1000
                },
                Some(PowerPool {
                    power_type: MANA,
                    current: 700,
                    max: 1000
                })
            )
        );
        assert_eq!(unit.class(), DRUID, "only the power type byte changes");
    }

    #[test]
    fn showing_the_own_pool_again_returns_it_and_ends_the_form_pool() {
        let mut unit = druid(700, 1000);
        unit.show_pool(ENERGY);
        unit.power = 60;
        unit.show_pool(MANA);
        assert_eq!(
            (unit.shown_pool(), unit.reserve_pool()),
            (
                PowerPool {
                    power_type: MANA,
                    current: 700,
                    max: 1000
                },
                None
            )
        );
    }

    #[test]
    fn switching_between_form_pools_keeps_the_own_pool_in_reserve() {
        let mut unit = druid(700, 1000);
        unit.show_pool(RAGE);
        unit.power = 300;
        unit.show_pool(ENERGY);
        assert_eq!(
            (unit.shown_pool(), unit.own_pool()),
            (
                PowerPool {
                    power_type: ENERGY,
                    current: 0,
                    max: 100
                },
                PowerPool {
                    power_type: MANA,
                    current: 700,
                    max: 1000
                }
            )
        );
    }

    #[test]
    fn mana_gains_and_costs_reach_the_reserve_pool() {
        let mut unit = druid(700, 1000);
        unit.show_pool(RAGE);
        unit.gain_power(MANA, 500);
        unit.spend_power(MANA, 200);
        assert_eq!(unit.spendable_power(MANA), 800);
        assert_eq!(unit.power, 0, "rage is untouched");
        unit.gain_power(ENERGY, 50);
        assert_eq!((unit.power, unit.reserve_power), (0, 800), "no energy pool");
    }

    #[test]
    fn an_energize_reaches_the_pool_it_names_and_skips_a_pool_without_a_max() {
        use lyracore_shared::spell::taxonomy::P_PCT_MAX_POWER;
        let mut unit = druid(700, 1000);
        unit.show_pool(RAGE);
        let reached = |p0_kind, p0| unit.energized_pool(p0_kind, p0).map(|pool| pool.power_type);
        assert_eq!(reached(P_POWER_TYPE, i32::from(MANA)), Some(MANA));
        assert_eq!(reached(P_POWER_TYPE, i32::from(ENERGY)), None);
        assert_eq!(reached(P_POWER_TYPE, -1), None);
        assert_eq!(
            reached(P_PCT_MAX_POWER, 0),
            Some(RAGE),
            "no named pool: the shown one"
        );
        let without_mana = druid(0, 0);
        assert_eq!(
            without_mana.energized_pool(P_POWER_TYPE, i32::from(MANA)),
            None
        );
    }

    #[test]
    fn resurrection_restores_every_pool_by_its_power_type() {
        let mut unit = druid(100, 1000);
        unit.show_pool(RAGE);
        unit.power = 300;
        unit.set_each_pool(|pool| resurrected_power(pool, 50));
        assert_eq!((unit.power, unit.reserve_power), (0, 500));
    }

    #[test]
    fn a_cost_in_a_pool_the_unit_lacks_cannot_be_spent_or_paid() {
        let mut unit = druid(700, 1000);
        assert_eq!(unit.spendable_power(RAGE), 0);
        assert_eq!(unit.spendable_power(MANA), 700);
        unit.spend_power(RAGE, 50);
        unit.show_pool(RAGE);
        unit.power = 300;
        assert_eq!(unit.spendable_power(ENERGY), 0);
        unit.spend_power(ENERGY, 50);
        assert_eq!((unit.power, unit.reserve_power), (300, 700));
    }

    #[test]
    fn a_health_cost_spends_all_but_the_last_point_of_health() {
        let mut unit = druid(700, 1000);
        unit.health = 60;
        assert_eq!(unit.spendable_power(HEALTH), 59);
        unit.spend_power(HEALTH, 45);
        assert_eq!((unit.health, unit.power), (15, 700));
        unit.spend_power(HEALTH, 45);
        assert_eq!(unit.health, 1);
    }

    #[test]
    fn a_resurrect_request_restores_mana_by_percent_energy_full_and_rage_none() {
        let pool = |power_type| PowerPool {
            power_type,
            current: 40,
            max: 200,
        };
        assert_eq!(request_resurrected_power(pool(MANA), 35), 70);
        assert_eq!(request_resurrected_power(pool(ENERGY), 35), 200);
        assert_eq!(request_resurrected_power(pool(RAGE), 35), 0);
        assert_eq!(request_resurrected_power(pool(FOCUS), 35), 40);
    }

    #[test]
    fn own_pool_writes_follow_the_pool_into_reserve() {
        let mut unit = druid(700, 1000);
        unit.show_pool(RAGE);
        unit.set_own_pool(650, 1200);
        assert_eq!((unit.power, unit.max_power), (0, 1000));
        assert_eq!(
            unit.own_pool(),
            PowerPool {
                power_type: MANA,
                current: 650,
                max: 1200
            }
        );
    }
}
