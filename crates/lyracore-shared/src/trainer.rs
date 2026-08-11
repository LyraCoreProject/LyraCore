//! Which characters a trainer will serve.
//!
//! Shared because the rule has two producers that must not drift: the module enforces it on the buy
//! and the respec, and the gateway applies the same rule to decide whether to show the window and
//! the gossip option at all. A display copy that drifted permissive would advertise a service the
//! module then refuses; one that drifted restrictive would hide a trainer that works.

/// `creature_template.TrainerType`.
///
/// `CLASS` is 0, which is also the ingested column's default, so most creatures read `CLASS` without
/// meaning anything by it. This value alone can never identify a class trainer — see [`serves`].
pub mod trainer_type {
    /// Teaches a class's spells — the only type this gate restricts.
    pub const CLASS: u8 = 0;
    /// Riding trainer. Serves every class.
    pub const MOUNTS: u8 = 1;
    /// Profession trainer and the weapon masters. Serves every class.
    pub const TRADESKILLS: u8 = 2;
    /// Hunter pet trainer. Serves every class here: vanilla also requires a Hunter, but some pet
    /// trainers carry a class id and there is no pet system to gate, so restricting would be worse.
    pub const PETS: u8 = 3;
}

/// Will this trainer serve a character of `player_class`?
///
/// Refuses only when all three hold: the trainer is a `CLASS` trainer, its `trainer_class` is
/// populated, and it differs from the character's. The populated check is what keeps the gate
/// fail-open — `CLASS` is 0 and so is the column's default, so without it every trainer in a world
/// that has not been re-imported would refuse everyone.
///
/// Both ids are class IDs (1 Warrior, 2 Paladin, 3 Hunter, 4 Rogue, 5 Priest, 7 Shaman, 8 Mage,
/// 9 Warlock, 11 Druid), never masks.
pub const fn serves(player_class: u8, trainer_type: u8, trainer_class: u8) -> bool {
    trainer_type != self::trainer_type::CLASS || trainer_class == 0 || trainer_class == player_class
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every class the 1.12 client can create. 6 and 10 do not exist in vanilla.
    const CLASSES: [u8; 9] = [1, 2, 3, 4, 5, 7, 8, 9, 11];

    #[test]
    fn a_class_trainer_serves_its_own_class_and_refuses_every_other() {
        for &trainer_class in &CLASSES {
            for &player_class in &CLASSES {
                assert_eq!(
                    serves(player_class, trainer_type::CLASS, trainer_class),
                    player_class == trainer_class,
                    "class trainer {trainer_class} vs player {player_class}"
                );
            }
        }
        // The reported case: a Warrior at a Paladin trainer.
        assert!(!serves(1, trainer_type::CLASS, 2));
        assert!(serves(2, trainer_type::CLASS, 2));
    }

    /// The fail-open case. Every row of a world that has not been re-imported reads class 0 while
    /// type also defaults to 0, so if this fails, publishing locks every player out of every trainer.
    #[test]
    fn an_unpopulated_trainer_class_serves_every_class() {
        for &player_class in &CLASSES {
            assert!(
                serves(player_class, trainer_type::CLASS, 0),
                "un-reimported trainer must still serve class {player_class}"
            );
        }
    }

    /// A non-CLASS trainer is never gated, including one carrying a class id — real data, not
    /// hypothetical: the pet trainer Karrina Mekenda ships type 3 with class 3.
    #[test]
    fn non_class_trainer_types_serve_every_class_even_with_a_populated_trainer_class() {
        for ty in [
            trainer_type::MOUNTS,
            trainer_type::TRADESKILLS,
            trainer_type::PETS,
        ] {
            for &player_class in &CLASSES {
                assert!(
                    serves(player_class, ty, 0),
                    "type {ty} serves {player_class}"
                );
                for &trainer_class in &CLASSES {
                    assert!(
                        serves(player_class, ty, trainer_class),
                        "type {ty} (class {trainer_class}) still serves {player_class}"
                    );
                }
            }
        }
    }

    /// Three real trainers verified against the pinned dump, as a fence on the gate reading the
    /// ingested data the way the importer writes it.
    #[test]
    fn the_dump_verified_trainers_gate_as_ingested() {
        // Brother Sammuel (925), Paladin trainer: type 0 / class 2.
        assert!(serves(2, 0, 2));
        assert!(!serves(1, 0, 2));
        // Woo Ping (11867), weapon master: type 2 / class 0.
        for &c in &CLASSES {
            assert!(serves(c, 2, 0), "every class trains weapons at Woo Ping");
        }
        // Larimaine Purdue (2485), portal trainer: a genuine class trainer, so mages only.
        assert!(serves(8, 0, 8));
        assert!(!serves(1, 0, 8));
    }

    /// Drift guard: the ingest stores the dump's raw value, so these names must match the source enum.
    #[test]
    fn trainer_type_constants_match_the_source_enum() {
        assert_eq!(trainer_type::CLASS, 0);
        assert_eq!(trainer_type::MOUNTS, 1);
        assert_eq!(trainer_type::TRADESKILLS, 2);
        assert_eq!(trainer_type::PETS, 3);
    }
}
