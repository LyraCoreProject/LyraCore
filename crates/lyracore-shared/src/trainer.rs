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

/// `SkillLine.dbc` id of Riding — the skill a [`trainer_type::MOUNTS`] trainer teaches, and the line a
/// mount spell's `SkillLineAbility` row names. Shared for the same no-drift reason as [`serves`]: the
/// module grants the rank on the buy, and the gateway needs to know a riding offering teaches a SKILL so
/// it does not echo the offering's marker id back as a learned spell.
pub const RIDING_SKILL_LINE: u32 = 762;

/// Why the Module refused a trainer Durable Request. The tag is the whole reducer error text, so
/// neither tier matches on human prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrainerRefusal {
    /// The trainer will not serve this interaction at all: missing, not a trainer, on another map,
    /// out of range, refusing the buyer's class, or asked for riding while teaching something else.
    Unavailable,
    /// The trainer's offering list does not carry that spell.
    NotOffered,
    AlreadyKnown,
    LevelTooLow,
    /// A rank purchase whose `game_spell_chain` predecessor is not known yet.
    PreviousRankMissing,
    NotEnoughMoney,
}

impl TrainerRefusal {
    pub const ALL: [Self; 6] = [
        Self::Unavailable,
        Self::NotOffered,
        Self::AlreadyKnown,
        Self::LevelTooLow,
        Self::PreviousRankMissing,
        Self::NotEnoughMoney,
    ];

    pub fn as_tag(self) -> &'static str {
        match self {
            Self::Unavailable => "trainer:unavailable",
            Self::NotOffered => "trainer:not_offered",
            Self::AlreadyKnown => "trainer:already_known",
            Self::LevelTooLow => "trainer:level_too_low",
            Self::PreviousRankMissing => "trainer:previous_rank_missing",
            Self::NotEnoughMoney => "trainer:not_enough_money",
        }
    }

    pub fn parse_tag(tag: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|refusal| refusal.as_tag() == tag)
    }
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
    fn every_refusal_tag_round_trips() {
        for refusal in TrainerRefusal::ALL {
            assert_eq!(TrainerRefusal::parse_tag(refusal.as_tag()), Some(refusal));
        }
        assert_eq!(TrainerRefusal::parse_tag("trainer:"), None);
        assert_eq!(
            TrainerRefusal::parse_tag("gw_trainer_buy reducer timed out after 10s"),
            None
        );
    }

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

    /// Fail-open: if this breaks, an un-reimported world locks every player out of every trainer.
    #[test]
    fn an_unpopulated_trainer_class_serves_every_class() {
        for &player_class in &CLASSES {
            assert!(
                serves(player_class, trainer_type::CLASS, 0),
                "un-reimported trainer must still serve class {player_class}"
            );
        }
    }

    /// A populated class id on a non-CLASS trainer is real data (pet trainers ship one), not a
    /// reason to gate.
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

    /// The gate must read the columns the way the importer writes them.
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
