use lyracore_shared::packing::power_type;

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

    /// Adds `amount` to the unit's pool of `power_type`, up to its max; a unit without one gains none.
    pub(crate) fn gain_power(&mut self, power_type: u8, amount: u32) {
        let (current, max) = if self.power_type() == power_type {
            (&mut self.power, self.max_power)
        } else if self.reserve_power_type == power_type {
            (&mut self.reserve_power, self.reserve_max_power)
        } else {
            return;
        };
        *current = current.saturating_add(amount).min(max);
    }

    /// The power a cost in `power_type` can spend: the Reserve Pool's when it holds that power
    /// type, else the shown pool's.
    pub(crate) fn spendable_power(&self, power_type: u8) -> u32 {
        match self.reserve_pool() {
            Some(reserve) if reserve.power_type == power_type => reserve.current,
            _ => self.power,
        }
    }

    pub(crate) fn spend_power(&mut self, power_type: u8, amount: u32) {
        let pool = if self.reserve_power_type == power_type {
            &mut self.reserve_power
        } else {
            &mut self.power
        };
        *pool = pool.saturating_sub(amount);
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
