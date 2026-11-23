//! Queued strike Roll Conformance; ADR 0007 derives every probability here from vmangos.

use anyhow::{ensure, Result};
use wire_client::decode::{Field, Packet};

use super::strike_rolls::{self, StrikeContext, BLOCK_DRAWS};

pub const SPELL_ID: u32 = 78;
pub type Counts = strike_rolls::Counts<7>;

/// The first six are exclusive table outcomes; a partial block overlaps the landed three.
pub const CONTEXT: StrikeContext<7> = StrikeContext {
    name: "queued_strike",
    stream: "swings",
    opcode: 0x014a,
    categories: ["miss", "dodge", "parry", "glancing", "crit", "hit", "block"],
    expected: [
        [
            (500, 10_000),
            (500, 10_000),
            (500, 10_000),
            (1000, 10_000),
            (500, 10_000),
            (7000, 10_000),
            (8500 * BLOCK_DRAWS, 10_000 << 32),
        ],
        [
            (800, 10_000),
            (650, 10_000),
            (1400, 10_000),
            (4000, 10_000),
            (200, 10_000),
            (2950, 10_000),
            (7150 * BLOCK_DRAWS, 10_000 << 32),
        ],
    ],
};

impl Counts {
    /// Counts one strike from the Character's own packets; an error is a gap blocking the sample.
    pub fn observe(&mut self, packets: &[Packet], attacker: u64, target: u64) -> Result<()> {
        let own =
            |packet: &&Packet, key: &str| packet.fields.get(key) == Some(&Field::Guid(attacker));
        let swings: Vec<_> = packets
            .iter()
            .filter(|packet| packet.opcode == 0x014a && own(packet, "attacker"))
            .collect();
        let logs: Vec<_> = packets
            .iter()
            .filter(|packet| packet.opcode == 0x0250 && own(packet, "caster"))
            .collect();
        let [swing] = swings.as_slice() else {
            anyhow::bail!(
                "{} own SMSG_ATTACKERSTATEUPDATE packets carried one queued strike",
                swings.len()
            )
        };
        ensure!(
            swing.fields.get("victim") == Some(&Field::Guid(target))
                && number(swing, "spell")? == u64::from(SPELL_ID),
            "the swing did not carry the queued strike at its target: {:?}",
            swing.fields
        );
        let flags = number(swing, "hit_info")?;
        let state = number(swing, "victim_state")?;
        ensure!(
            flags & (0x4 | 0x8000) == 0
                && number(swing, "absorbed")? == 0
                && number(swing, "resisted")? == 0,
            "ineligible offhand, crushing, absorbed or resisted strike: {:?}",
            swing.fields
        );
        let category = match (state, flags & (0x10 | 0x80 | 0x4000)) {
            (0, 0x10) => 0,
            (2, 0) => 1,
            (3, 0) => 2,
            (1, 0x4000) => 3,
            (1, 0x80) => 4,
            (1, 0) => 5,
            _ => anyhow::bail!(
                "ineligible or conflicting strike outcome: {:?}",
                swing.fields
            ),
        };
        let blocked = flags & 0x800 != 0;
        ensure!(
            category >= 3 || !blocked,
            "an avoided strike was blocked: {:?}",
            swing.fields
        );
        if category < 3 {
            ensure!(logs.is_empty(), "an avoided strike logged damage");
        } else {
            let [log] = logs.as_slice() else {
                anyhow::bail!("{} damage logs for one landed strike", logs.len())
            };
            ensure!(
                log.fields.get("target") == Some(&Field::Guid(target))
                    && number(log, "spell")? == u64::from(SPELL_ID)
                    && number(log, "absorbed")? == 0
                    && number(log, "periodic")? == 0
                    && log.fields.get("resisted") == Some(&Field::Signed(0)),
                "ineligible strike damage log: {:?}",
                log.fields
            );
            ensure!(
                (number(log, "hit_info")? & 2 != 0) == (category == 4)
                    && (number(log, "blocked")? > 0) == blocked,
                "the damage log disagrees with the swing: {:?} {:?}",
                swing.fields,
                log.fields
            );
        }
        self.record(category, blocked);
        Ok(())
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

    const ATTACKER: u64 = 7;
    const TARGET: u64 = 9;

    fn swing(hit_info: u64, victim_state: u64, spell: u32) -> Packet {
        Packet {
            opcode: 0x014a,
            fields: BTreeMap::from([
                ("attacker".into(), Field::Guid(ATTACKER)),
                ("victim".into(), Field::Guid(TARGET)),
                ("hit_info".into(), Field::Unsigned(hit_info)),
                ("victim_state".into(), Field::Unsigned(victim_state)),
                ("absorbed".into(), Field::Unsigned(0)),
                ("resisted".into(), Field::Unsigned(0)),
                ("spell".into(), Field::Unsigned(spell.into())),
            ]),
        }
    }

    fn log(hit_info: u64, blocked: u64) -> Packet {
        Packet {
            opcode: 0x0250,
            fields: BTreeMap::from([
                ("caster".into(), Field::Guid(ATTACKER)),
                ("target".into(), Field::Guid(TARGET)),
                ("spell".into(), Field::Unsigned(SPELL_ID.into())),
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
    fn a_strike_is_classified_from_its_swing_and_damage_log() -> Result<()> {
        let mut counts = Counts::default();
        counts.observe(&[swing(0x10, 0, SPELL_ID)], ATTACKER, TARGET)?;
        counts.observe(
            &[swing(0x4000 | 0x800, 1, SPELL_ID), log(0, 30)],
            ATTACKER,
            TARGET,
        )?;
        counts.observe(&[swing(0x80, 1, SPELL_ID), log(2, 0)], ATTACKER, TARGET)?;
        assert_eq!(counts, Counts::new([1, 0, 0, 1, 1, 0, 1]));
        assert_eq!(counts.trials(), 3);
        Ok(())
    }

    #[test]
    fn a_strike_without_its_swing_state_blocks_the_sample() {
        let mut counts = Counts::default();
        assert!(counts.observe(&[log(0, 0)], ATTACKER, TARGET).is_err());
        assert!(counts
            .observe(&[swing(0, 1, 0), log(0, 0)], ATTACKER, TARGET)
            .is_err());
        assert!(counts
            .observe(&[swing(0x80, 1, SPELL_ID), log(0, 0)], ATTACKER, TARGET)
            .is_err());
        let difference = CONTEXT.blocked(&counts, 1, "public_2", "no swing state");
        assert!(CONTEXT.accepts(&difference));
        assert_eq!(difference.checkpoint, "queued_strike_level_63_public_2");
    }

    #[test]
    fn the_scoreboard_accepts_only_derived_distribution_signatures() -> Result<()> {
        let mut counts = Counts::new([0, 0, 0, 0, 0, strike_rolls::SAMPLES_PER_LEVEL, 0]);
        let differences = CONTEXT.compare(&counts, 0, "merge")?;
        assert!(differences.iter().all(|d| CONTEXT.accepts(d)));
        assert!(differences
            .iter()
            .any(|d| d.field_path == ["outcomes", "hit"]));
        let mut forged = differences[0].clone();
        forged.expected = Some(RecordedField::Unsigned(1));
        assert!(!CONTEXT.accepts(&forged));
        counts.0[5] -= 1;
        assert!(CONTEXT.compare(&counts, 0, "merge").is_err());
        Ok(())
    }
}
