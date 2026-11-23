//! Cast Weapon Strike Roll Conformance; ADR 0007 derives every probability here from vmangos.

use std::collections::BTreeMap;

use anyhow::{ensure, Context, Result};
use wire_client::decode::{Field, Packet};

use super::strike_rolls::{self, StrikeContext, BLOCK_DRAWS};

pub const SPELL_ID: u32 = 12294;
pub const SPELL_GO: u16 = 0x0132;
pub const SPELL_LOG_MISS: u16 = 0x024b;
const DAMAGE_LOG: u16 = 0x0250;
const CRIT: usize = 3;
pub type Counts = strike_rolls::Counts<6>;

/// The first five are exclusive table outcomes; a partial block overlaps the landed two.
pub const CONTEXT: StrikeContext<6> = StrikeContext {
    name: "weapon_strike",
    stream: "casts",
    opcode: SPELL_GO,
    categories: ["miss", "dodge", "parry", "crit", "hit", "block"],
    expected: [
        [
            (500, 10_000),
            (500, 10_000),
            (500, 10_000),
            (500, 10_000),
            (8000, 10_000),
            (8500 * BLOCK_DRAWS, 10_000 << 32),
        ],
        [
            (800, 10_000),
            (650, 10_000),
            (1400, 10_000),
            (200, 10_000),
            (6950, 10_000),
            (7150 * BLOCK_DRAWS, 10_000 << 32),
        ],
    ],
};

impl Counts {
    /// Counts one cast from the Character's own packets; an error is a gap blocking the sample.
    pub fn observe(&mut self, packets: &[Packet], caster: u64, target: u64) -> Result<()> {
        let own = |packet: &&Packet, opcode: u16| {
            packet.opcode == opcode
                && packet.fields.get("caster") == Some(&Field::Guid(caster))
                && packet.fields.get("spell") == Some(&Field::Unsigned(SPELL_ID.into()))
        };
        let gos: Vec<_> = packets.iter().filter(|p| own(p, SPELL_GO)).collect();
        let logs: Vec<_> = packets.iter().filter(|p| own(p, DAMAGE_LOG)).collect();
        let [go] = gos.as_slice() else {
            anyhow::bail!("{} own SMSG_SPELL_GO packets for one cast", gos.len())
        };
        ensure!(
            !packets.iter().any(|p| own(&p, SPELL_LOG_MISS)),
            "vmangos sends no SMSG_SPELLLOGMISS for a melee spell miss, dodge or parry: {:?}",
            go.fields
        );
        let target_guid = Field::Guid(target);
        let hits = list(go, "hits")?;
        let misses: Vec<_> = list(go, "misses")?
            .iter()
            .filter_map(|miss| match miss {
                Field::Object(miss) if miss.get("guid") == Some(&target_guid) => {
                    Some(miss.get("reason"))
                }
                _ => None,
            })
            .collect();
        let hit = hits.iter().filter(|guid| **guid == target_guid).count();
        let mut blocked = false;
        let category = match (hit, misses.as_slice()) {
            (0, [Some(Field::Unsigned(1))]) => 0,
            (0, [Some(Field::Unsigned(3))]) => 1,
            (0, [Some(Field::Unsigned(4))]) => 2,
            (1, []) => {
                let [log] = logs.as_slice() else {
                    anyhow::bail!("{} damage logs for one landed cast", logs.len())
                };
                ensure!(
                    log.fields.get("target") == Some(&target_guid)
                        && number(log, "school")? == 0
                        && number(log, "absorbed")? == 0
                        && number(log, "periodic")? == 0
                        && log.fields.get("resisted") == Some(&Field::Signed(0)),
                    "ineligible cast damage log: {:?}",
                    log.fields
                );
                blocked = number(log, "blocked")? > 0;
                if number(log, "hit_info")? & 2 != 0 {
                    CRIT
                } else {
                    4
                }
            }
            _ => anyhow::bail!("ineligible or conflicting cast outcome: {:?}", go.fields),
        };
        ensure!(
            category >= CRIT || logs.is_empty(),
            "an avoided cast logged damage"
        );
        ensure!(
            !packets.iter().any(|packet| packet.opcode == 0x014a
                && packet.fields.get("attacker") == Some(&Field::Guid(caster))
                && packet.fields.get("spell") == Some(&Field::Unsigned(SPELL_ID.into()))),
            "vmangos sends no SMSG_ATTACKERSTATEUPDATE for a spell that is not a queued strike"
        );
        self.record(category, blocked);
        Ok(())
    }
}

/// Decodes the caster and spell of SMSG_SPELLLOGMISS: spell u32, caster u64, then the targets.
pub fn spell_log_miss(body: &[u8]) -> Result<Packet> {
    let spell = body.get(..4).context("truncated SMSG_SPELLLOGMISS")?;
    let caster = body.get(4..12).context("truncated SMSG_SPELLLOGMISS")?;
    Ok(Packet {
        opcode: SPELL_LOG_MISS,
        fields: BTreeMap::from([
            (
                "spell".into(),
                Field::Unsigned(u32::from_le_bytes(spell.try_into()?).into()),
            ),
            (
                "caster".into(),
                Field::Guid(u64::from_le_bytes(caster.try_into()?)),
            ),
        ]),
    })
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
    use wire_client::trace::RecordedField;

    use super::*;

    const CASTER: u64 = 7;
    const TARGET: u64 = 9;

    fn go(hit: bool, miss_reason: Option<u64>) -> Packet {
        let misses = miss_reason
            .map(|reason| {
                Field::Object(BTreeMap::from([
                    ("guid".into(), Field::Guid(TARGET)),
                    ("reason".into(), Field::Unsigned(reason)),
                ]))
            })
            .into_iter()
            .collect();
        let hits = hit.then_some(Field::Guid(TARGET)).into_iter().collect();
        Packet {
            opcode: SPELL_GO,
            fields: BTreeMap::from([
                ("caster".into(), Field::Guid(CASTER)),
                ("spell".into(), Field::Unsigned(SPELL_ID.into())),
                ("hits".into(), Field::List(hits)),
                ("misses".into(), Field::List(misses)),
            ]),
        }
    }

    fn log(hit_info: u64, blocked: u64) -> Packet {
        Packet {
            opcode: DAMAGE_LOG,
            fields: BTreeMap::from([
                ("caster".into(), Field::Guid(CASTER)),
                ("target".into(), Field::Guid(TARGET)),
                ("spell".into(), Field::Unsigned(SPELL_ID.into())),
                ("school".into(), Field::Unsigned(0)),
                ("absorbed".into(), Field::Unsigned(0)),
                ("periodic".into(), Field::Unsigned(0)),
                ("resisted".into(), Field::Signed(0)),
                ("blocked".into(), Field::Unsigned(blocked)),
                ("hit_info".into(), Field::Unsigned(hit_info)),
            ]),
        }
    }

    #[test]
    fn the_derived_table_sums_to_one_and_block_overlaps_it() {
        for probabilities in CONTEXT.expected {
            let table: u64 = probabilities[..CONTEXT.block()]
                .iter()
                .map(|(n, _)| n)
                .sum();
            assert_eq!(table, 10_000);
            let (block, denominator) = probabilities[CONTEXT.block()];
            assert!(block < denominator && denominator <= 1 << 53);
        }
    }

    #[test]
    fn a_cast_is_classified_from_its_spell_go_and_damage_log() -> Result<()> {
        let mut counts = Counts::default();
        counts.observe(&[go(false, Some(3))], CASTER, TARGET)?;
        counts.observe(&[go(true, None), log(2, 0)], CASTER, TARGET)?;
        counts.observe(&[go(true, None), log(0, 31)], CASTER, TARGET)?;
        assert_eq!(counts, Counts::new([0, 1, 0, 1, 1, 1]));
        assert_eq!(counts.trials(), 3);
        Ok(())
    }

    #[test]
    fn a_target_listed_as_hit_with_a_miss_log_blocks_the_sample() -> Result<()> {
        let mut counts = Counts::default();
        let miss_log = spell_log_miss(
            &[
                &SPELL_ID.to_le_bytes()[..],
                &CASTER.to_le_bytes(),
                &[0, 1, 0, 0, 0],
                &TARGET.to_le_bytes(),
                &[3],
            ]
            .concat(),
        )?;
        assert!(counts
            .observe(&[go(true, None), miss_log], CASTER, TARGET)
            .is_err());
        assert!(counts.observe(&[go(true, None)], CASTER, TARGET).is_err());
        assert!(counts
            .observe(&[go(false, Some(4)), log(0, 0)], CASTER, TARGET)
            .is_err());
        assert!(counts.observe(&[log(0, 0)], CASTER, TARGET).is_err());
        let difference = CONTEXT.blocked(&counts, 1, "public_2", "listed as hit");
        assert!(CONTEXT.accepts(&difference));
        assert_eq!(difference.checkpoint, "weapon_strike_level_63_public_2");
        Ok(())
    }

    #[test]
    fn the_scoreboard_accepts_only_derived_distribution_signatures() -> Result<()> {
        let mut counts = Counts::new([0, 0, 0, 0, strike_rolls::SAMPLES_PER_LEVEL, 0]);
        let differences = CONTEXT.compare(&counts, 0, "merge")?;
        assert!(differences.iter().all(|d| CONTEXT.accepts(d)));
        assert!(differences
            .iter()
            .any(|d| d.field_path == ["outcomes", "block"]));
        let mut forged = differences[0].clone();
        forged.expected = Some(RecordedField::Unsigned(1));
        assert!(!CONTEXT.accepts(&forged));
        counts.0[4] -= 1;
        assert!(CONTEXT.compare(&counts, 0, "merge").is_err());
        Ok(())
    }
}
