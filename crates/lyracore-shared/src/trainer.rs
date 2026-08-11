//! Which characters a trainer will serve (issue #126, the gate half of #116).
//!
//! Lives here rather than in the module because the rule is about to have TWO producers: the module
//! enforces it today at the buy + respec chokepoint (authoritative — a modified client can send the
//! trainer opcodes without ever opening a window), and #127 will apply the SAME rule in the gateway
//! to decide whether to show the trainer window and the gossip "train" option at all. Until #127
//! lands the module is the only caller. Splitting it up front is the [`crate::whisper`] precedent,
//! and drift here is worse than most: a display copy that went PERMISSIVE would advertise a service
//! the module then refuses, and one that went RESTRICTIVE would hide a trainer that works.
//!
//! Pure integers in, bool out — no `ReducerContext`, no database, so the whole truth table is
//! unit-testable right here.

/// `creature_template.TrainerType` (cmangos enum), ingested by #125.
///
/// **`CLASS` is 0, which is also the column's `#[default]`** — so a template that has never been
/// re-imported, and every one of the ~10060 ordinary creatures in the dump that is not a trainer at
/// all, reads `CLASS` without meaning anything by it. This value can therefore never be used on its
/// own to conclude "this is a class trainer"; see [`serves`] for the condition that actually can.
pub mod trainer_type {
    /// Teaches a class's spells — the only type this gate restricts.
    pub const CLASS: u8 = 0;
    /// Riding trainer. Serves every class.
    pub const MOUNTS: u8 = 1;
    /// Profession trainer and the weapon masters. Serves every class.
    pub const TRADESKILLS: u8 = 2;
    /// Hunter pet trainer. Serves every class HERE — vanilla additionally requires the player be a
    /// Hunter, but there is no pet system yet, and some pet trainers carry a nonzero `trainer_class`
    /// (Karrina Mekenda, entry 2879, is type 3 / class 3), so restricting on it would gate a service
    /// that does not exist. Deliberately fail-open; revisit with the pet system.
    pub const PETS: u8 = 3;
}

/// Will a trainer with this `trainer_type`/`trainer_class` serve a character of `player_class`?
///
/// Refuses on THREE conditions together, and every one of them is load-bearing:
///
/// 1. the trainer is a `CLASS` trainer — a profession/riding/pet trainer serves everyone;
/// 2. its `trainer_class` is populated — **this is the one that keeps the gate fail-open.** Because
///    `CLASS` is 0 and 0 is also the ingested column's default, condition 1 alone is true for every
///    creature in a world that has not been re-imported since #125, and for every non-trainer
///    creature even in one that has. Drop this condition and the gate closes the entire world;
/// 3. its class differs from the character's.
///
/// Anything else serves. `player_class` is the packed `UNIT_FIELD_BYTES_0` byte 1 (1 Warrior,
/// 2 Paladin, 3 Hunter, 4 Rogue, 5 Priest, 7 Shaman, 8 Mage, 9 Warlock, 11 Druid) — an ID, not a
/// mask, matching `trainer_class`.
pub const fn serves(player_class: u8, trainer_type: u8, trainer_class: u8) -> bool {
    trainer_type != self::trainer_type::CLASS || trainer_class == 0 || trainer_class == player_class
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every class id the 1.12 client can create. 6 and 10 are absent in vanilla (they became Death
    /// Knight and Monk much later) — deliberately not listed, so the cross-product below is over
    /// REAL classes only.
    const CLASSES: [u8; 9] = [1, 2, 3, 4, 5, 7, 8, 9, 11];

    /// The headline case, and the reported bug (#116): a Warrior at a Paladin trainer.
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
        // Spelled out for the screenshot in #116, so a reader sees the actual reported case.
        assert!(
            !serves(1, trainer_type::CLASS, 2),
            "a Warrior is refused by a Paladin trainer"
        );
        assert!(
            serves(2, trainer_type::CLASS, 2),
            "a Paladin is served by a Paladin trainer"
        );
    }

    /// THE FAIL-OPEN CASE. A world that has not been re-imported since #125 reads `trainer_class = 0`
    /// on every row while `trainer_type` also defaults to 0 (= CLASS) — so this is not a corner, it is
    /// the state of every existing database the moment the schema change publishes. If this test ever
    /// fails, publishing the module locks every player out of every trainer realm-wide.
    #[test]
    fn an_unpopulated_trainer_class_serves_every_class() {
        for &player_class in &CLASSES {
            assert!(
                serves(player_class, trainer_type::CLASS, 0),
                "un-reimported trainer must still serve class {player_class}"
            );
        }
    }

    /// A non-CLASS trainer is never gated — including when it carries a nonzero `trainer_class`,
    /// which is REAL data, not a hypothetical: the pet trainer Karrina Mekenda (2879) ships type 3 /
    /// class 3 in the pinned dump. Gating on `trainer_class` without first checking the type would
    /// make her Hunter-only.
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
                    "trainer type {ty} serves class {player_class}"
                );
                for &trainer_class in &CLASSES {
                    assert!(
                        serves(player_class, ty, trainer_class),
                        "trainer type {ty} (class {trainer_class}) still serves {player_class}"
                    );
                }
            }
        }
    }

    /// The three real trainers #125 verified against the pinned dump, as a regression fence on the
    /// gate reading that ingested data the way the ingest slice intended.
    #[test]
    fn the_dump_verified_trainers_gate_the_way_125_ingested_them() {
        // Brother Sammuel (925) — Paladin trainer, type 0 / class 2.
        assert!(serves(2, 0, 2), "a Paladin trains at Brother Sammuel");
        assert!(!serves(1, 0, 2), "a Warrior does not");
        // Woo Ping (11867) — weapon master, type 2 (TRADESKILLS) / class 0.
        for &c in &CLASSES {
            assert!(serves(c, 2, 0), "every class trains weapons at Woo Ping");
        }
        // Larimaine Purdue (2485) — portal trainer, type 0 / class 8 (Mage). A genuine class
        // trainer, so it gates: mages only. It has no offerings bound, so nothing changes in
        // practice — but the rule must still read it as Mage-only, not as "everyone".
        assert!(serves(8, 0, 8), "a Mage may train portals");
        assert!(!serves(1, 0, 8), "a Warrior may not");
    }

    /// The constants are the cmangos enum, not our invention — a drift guard, since #125's ingest
    /// stores the dump's raw value and this gate compares against these names.
    #[test]
    fn trainer_type_constants_match_the_cmangos_enum() {
        assert_eq!(trainer_type::CLASS, 0);
        assert_eq!(trainer_type::MOUNTS, 1);
        assert_eq!(trainer_type::TRADESKILLS, 2);
        assert_eq!(trainer_type::PETS, 3);
    }
}
