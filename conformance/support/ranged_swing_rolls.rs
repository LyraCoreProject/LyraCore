//! Ranged swing Roll Conformance; ADR 0007 derives every probability here from vmangos.

use anyhow::{ensure, Result};
use wire_client::decode::{Field, Packet};

use super::strike_rolls::{self, StrikeContext, BLOCK_DRAWS};

pub const SPELL_GO: u16 = 0x0132;
pub const SPELL_LOG_MISS: u16 = 0x024b;
const ATTACKER_STATE_UPDATE: u16 = 0x014a;
const DAMAGE_LOG: u16 = 0x0250;
const MISS: usize = 0;
const CRIT: usize = 1;
const HIT: usize = 2;
pub type Counts = strike_rolls::Counts<4>;

/// One ranged shot on the ranged attack table: its spell, its damage log's school and its context.
pub struct RangedSwing {
    pub spell: u32,
    pub school: u64,
    pub context: StrikeContext<4>,
}

/// Miss, crit and hit are exclusive table outcomes; a partial block overlaps the landed two.
/// A warrior's gun, bow, crossbow and thrown weapon share this table at weapon skill 300.
const WARRIOR_RANGED_TABLE: [[(u64, u64); 4]; 2] = [
    [
        (500, 10_000),
        (500, 10_000),
        (9000, 10_000),
        (9500 * BLOCK_DRAWS, 10_000 << 32),
    ],
    [
        (800, 10_000),
        (200, 10_000),
        (9000, 10_000),
        (9200 * BLOCK_DRAWS, 10_000 << 32),
    ],
];

pub const AUTO_SHOT: RangedSwing = RangedSwing {
    spell: 75,
    school: 0,
    context: StrikeContext {
        name: "auto_shot",
        stream: "casts",
        opcode: SPELL_GO,
        categories: ["miss", "crit", "hit", "block"],
        expected: WARRIOR_RANGED_TABLE,
    },
};

/// Throw is a cast ranged Weapon Strike; it rolls the same table and block as Auto Shot.
pub const THROW: RangedSwing = RangedSwing {
    spell: 2764,
    school: 0,
    context: StrikeContext {
        name: "ranged_weapon_strike",
        stream: "casts",
        opcode: SPELL_GO,
        categories: ["miss", "crit", "hit", "block"],
        expected: WARRIOR_RANGED_TABLE,
    },
};

/// A wand's Shoot has magic damage class, so vmangos never rolls its partial block.
pub const WAND_SHOT: RangedSwing = RangedSwing {
    spell: 5019,
    school: 6,
    context: StrikeContext {
        name: "wand_shot",
        stream: "casts",
        opcode: SPELL_GO,
        categories: ["miss", "crit", "hit", "block"],
        expected: [
            [(500, 10_000), (800, 10_000), (8700, 10_000), (0, 10_000)],
            [(800, 10_000), (500, 10_000), (8700, 10_000), (0, 10_000)],
        ],
    },
};

/// Whether a shooter's landed shot still waits for its damage log; one shot flies at a time.
#[derive(Debug, Default)]
pub struct Flight {
    landed: bool,
}

impl Flight {
    pub fn is_empty(&self) -> bool {
        !self.landed
    }
}

impl RangedSwing {
    /// Counts a batch of own packets up to `quota` shots; an error is a gap blocking the sample.
    pub fn observe(
        &self,
        flight: &mut Flight,
        counts: &mut Counts,
        packets: &[Packet],
        (shooter, target): (u64, u64),
        quota: u32,
    ) -> Result<()> {
        let shooter_guid = Field::Guid(shooter);
        let own = |packet: &Packet| {
            packet.fields.get("caster") == Some(&shooter_guid)
                && packet.fields.get("spell") == Some(&Field::Unsigned(self.spell.into()))
        };
        for packet in packets {
            if counts.trials() == quota {
                break;
            }
            match packet.opcode {
                SPELL_GO if own(packet) => {
                    ensure!(
                        flight.is_empty(),
                        "a shot fired before the landed shot's damage log: {:?}",
                        packet.fields
                    );
                    if landed(packet, target)? {
                        flight.landed = true;
                    } else {
                        counts.record(MISS, false);
                    }
                }
                DAMAGE_LOG if own(packet) => {
                    ensure!(!flight.is_empty(), "a damage log without a landed shot");
                    let (crit, blocked) = self.landing(packet, target)?;
                    flight.landed = false;
                    counts.record(if crit { CRIT } else { HIT }, blocked);
                }
                SPELL_LOG_MISS if own(packet) => anyhow::bail!(
                    "vmangos names a ranged miss in its SMSG_SPELL_GO, never in SMSG_SPELLLOGMISS"
                ),
                ATTACKER_STATE_UPDATE if packet.fields.get("attacker") == Some(&shooter_guid) => {
                    anyhow::bail!(
                        "vmangos sends no SMSG_ATTACKERSTATEUPDATE for a ranged swing: {:?}",
                        packet.fields
                    )
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The crit flag and partial block of a landed shot's damage log.
    fn landing(&self, log: &Packet, target: u64) -> Result<(bool, bool)> {
        let resisted = match log.fields.get("resisted") {
            Some(Field::Signed(resisted)) => *resisted,
            _ => anyhow::bail!("the damage log has no resisted amount"),
        };
        ensure!(
            log.fields.get("target") == Some(&Field::Guid(target))
                && number(log, "school")? == self.school
                && number(log, "absorbed")? == 0
                && number(log, "periodic")? == 0
                && (resisted == 0 || (self.school != 0 && resisted > 0)),
            "ineligible ranged swing damage log: {:?}",
            log.fields
        );
        Ok((
            number(log, "hit_info")? & 2 != 0,
            number(log, "blocked")? > 0,
        ))
    }
}

/// Whether the GO lists the target as hit; a miss is reason 1, and nothing else may roll.
fn landed(go: &Packet, target: u64) -> Result<bool> {
    let target_guid = Field::Guid(target);
    let hits = list(go, "hits")?
        .iter()
        .filter(|guid| **guid == target_guid)
        .count();
    let misses: Vec<_> = list(go, "misses")?
        .iter()
        .filter_map(|miss| match miss {
            Field::Object(miss) if miss.get("guid") == Some(&target_guid) => {
                Some(miss.get("reason"))
            }
            _ => None,
        })
        .collect();
    match (hits, misses.as_slice()) {
        (0, [Some(Field::Unsigned(1))]) => Ok(false),
        (1, []) => Ok(true),
        _ => anyhow::bail!("ineligible or conflicting shot outcome: {:?}", go.fields),
    }
}

fn list<'a>(packet: &'a Packet, name: &str) -> Result<&'a [Field]> {
    match packet.fields.get(name) {
        Some(Field::List(values)) => Ok(values),
        _ => anyhow::bail!("opcode {:#06x} has no {name}", packet.opcode),
    }
}

fn number(packet: &Packet, name: &str) -> Result<u64> {
    match packet.fields.get(name) {
        Some(Field::Unsigned(value)) => Ok(*value),
        _ => anyhow::bail!("opcode {:#06x} has no {name}", packet.opcode),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use wire_client::trace::RecordedField;

    use super::*;

    const SHOOTER: u64 = 7;
    const TARGET: u64 = 9;
    const PAIR: (u64, u64) = (SHOOTER, TARGET);

    fn go(miss_reason: Option<u64>) -> Packet {
        let misses = miss_reason
            .map(|reason| {
                Field::Object(BTreeMap::from([
                    ("guid".into(), Field::Guid(TARGET)),
                    ("reason".into(), Field::Unsigned(reason)),
                ]))
            })
            .into_iter()
            .collect();
        let hits = miss_reason
            .is_none()
            .then_some(Field::Guid(TARGET))
            .into_iter()
            .collect();
        Packet {
            opcode: SPELL_GO,
            fields: BTreeMap::from([
                ("caster".into(), Field::Guid(SHOOTER)),
                ("spell".into(), Field::Unsigned(AUTO_SHOT.spell.into())),
                ("hits".into(), Field::List(hits)),
                ("misses".into(), Field::List(misses)),
            ]),
        }
    }

    fn log(hit_info: u64, blocked: u64, resisted: i64) -> Packet {
        Packet {
            opcode: DAMAGE_LOG,
            fields: BTreeMap::from([
                ("caster".into(), Field::Guid(SHOOTER)),
                ("target".into(), Field::Guid(TARGET)),
                ("spell".into(), Field::Unsigned(AUTO_SHOT.spell.into())),
                ("school".into(), Field::Unsigned(0)),
                ("absorbed".into(), Field::Unsigned(0)),
                ("periodic".into(), Field::Unsigned(0)),
                ("resisted".into(), Field::Signed(resisted)),
                ("blocked".into(), Field::Unsigned(blocked)),
                ("hit_info".into(), Field::Unsigned(hit_info)),
            ]),
        }
    }

    #[test]
    fn the_derived_tables_sum_to_one_and_block_overlaps_them() {
        for swing in [&AUTO_SHOT, &WAND_SHOT, &THROW] {
            for probabilities in swing.context.expected {
                let table: u64 = probabilities[..swing.context.block()]
                    .iter()
                    .map(|(n, _)| n)
                    .sum();
                assert_eq!(table, 10_000);
                let (block, denominator) = probabilities[swing.context.block()];
                assert!(block < denominator && denominator <= 1 << 53);
            }
        }
    }

    #[test]
    fn a_landed_shot_is_counted_when_its_damage_log_arrives_in_a_later_batch() -> Result<()> {
        let (mut flight, mut counts) = (Flight::default(), Counts::default());
        AUTO_SHOT.observe(&mut flight, &mut counts, &[go(Some(1)), go(None)], PAIR, 9)?;
        assert_eq!((counts.trials(), flight.is_empty()), (1, false));
        AUTO_SHOT.observe(
            &mut flight,
            &mut counts,
            &[log(2, 0, 0), go(None), log(0, 12, 0)],
            PAIR,
            9,
        )?;
        assert!(flight.is_empty());
        assert_eq!(counts, Counts::new([1, 1, 1, 1]));
        Ok(())
    }

    #[test]
    fn counting_stops_at_the_quota() -> Result<()> {
        let (mut flight, mut counts) = (Flight::default(), Counts::default());
        AUTO_SHOT.observe(
            &mut flight,
            &mut counts,
            &[go(Some(1)), go(None), log(0, 0, 0)],
            PAIR,
            1,
        )?;
        assert_eq!(
            (counts, flight.is_empty()),
            (Counts::new([1, 0, 0, 0]), true)
        );
        Ok(())
    }

    #[test]
    fn a_shot_without_its_damage_log_or_with_a_melee_swing_blocks_the_sample() {
        let swing = Packet {
            opcode: ATTACKER_STATE_UPDATE,
            fields: BTreeMap::from([("attacker".into(), Field::Guid(SHOOTER))]),
        };
        for packets in [
            vec![log(0, 0, 0)],
            vec![go(None), go(None)],
            vec![go(Some(3))],
            vec![go(None), log(0, 0, 5)],
            vec![swing],
        ] {
            let (mut flight, mut counts) = (Flight::default(), Counts::default());
            assert!(AUTO_SHOT
                .observe(&mut flight, &mut counts, &packets, PAIR, 9)
                .is_err());
        }
        let difference = AUTO_SHOT
            .context
            .blocked(&Counts::default(), 1, "public_2", "no log");
        assert!(AUTO_SHOT.context.accepts(&difference));
        assert_eq!(difference.checkpoint, "auto_shot_level_63_public_2");
    }

    #[test]
    fn the_scoreboard_accepts_only_derived_distribution_signatures() -> Result<()> {
        let counts = Counts::new([0, 0, strike_rolls::SAMPLES_PER_LEVEL, 0]);
        let differences = AUTO_SHOT.context.compare(&counts, 0, "merge")?;
        assert!(differences.iter().all(|d| AUTO_SHOT.context.accepts(d)));
        assert!(!differences.iter().any(|d| WAND_SHOT.context.accepts(d)));
        assert!(!differences.iter().any(|d| THROW.context.accepts(d)));
        let throws = THROW.context.compare(&counts, 1, "public_1")?;
        assert!(throws.iter().all(|d| THROW.context.accepts(d)));
        assert!(!throws.iter().any(|d| AUTO_SHOT.context.accepts(d)));
        assert!(differences
            .iter()
            .any(|d| d.field_path == ["outcomes", "block"]));
        let mut forged = differences[0].clone();
        forged.expected = Some(RecordedField::Unsigned(1));
        assert!(!AUTO_SHOT.context.accepts(&forged));
        let blocked_wand = Counts::new([2500, 4000, 43_500, 1]);
        let wand = WAND_SHOT.context.compare(&blocked_wand, 0, "merge")?;
        assert_eq!(wand.len(), 1);
        assert_eq!(wand[0].field_path, ["outcomes", "block"]);
        Ok(())
    }
}
