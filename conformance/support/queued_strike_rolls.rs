//! Queued strike Roll Conformance; ADR 0007 derives every probability here from vmangos.

use anyhow::{ensure, Result};
use wire_client::compare::{Difference, DifferenceClass};
use wire_client::decode::{Field, Packet};
use wire_client::scenario::Stream;
use wire_client::trace::RecordedField;

pub const SPELL_ID: u32 = 78;
pub const SAMPLES_PER_LEVEL: u32 = 50_000;
pub const TARGET_LEVELS: [u32; 2] = [60, 63];
pub const DISTRIBUTION_RULE: &str = "queued_strike_distribution";
pub const SAMPLE_RULE: &str = "queued_strike_sample";
/// The first six are exclusive table outcomes; a partial block overlaps the landed three.
pub const CATEGORIES: [&str; 7] = ["miss", "dodge", "parry", "glancing", "crit", "hit", "block"];
const BLOCK: usize = 6;
const FAMILY_ALPHA: f64 = 1e-6;
const TESTS_PER_SUBMISSION: u32 = 7 * 2 * 3;
// `roll_chance_f(5.0)` accepts 214748365 of the 2^32 uniform draws.
const BLOCK_DRAWS: u64 = 214_748_365;

/// Exact probabilities as (numerator, denominator), by target level then category.
pub const EXPECTED: [[(u64, u64); 7]; 2] = [
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
];

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Counts(pub [u32; 7]);

impl Counts {
    pub fn trials(&self) -> u32 {
        self.0[..BLOCK].iter().sum()
    }

    pub fn add(&mut self, other: &Self) {
        for (total, count) in self.0.iter_mut().zip(other.0) {
            *total += count;
        }
    }

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
        self.0[category] += 1;
        self.0[BLOCK] += u32::from(blocked);
        Ok(())
    }
}

fn number(packet: &Packet, name: &str) -> Result<u64> {
    match packet.fields.get(name) {
        Some(Field::Unsigned(value)) => Ok(*value),
        _ => anyhow::bail!("opcode {:#06x} has no {name}", packet.opcode),
    }
}

fn checkpoint(level_index: usize, slot: &str) -> String {
    format!("queued_strike_level_{}_{slot}", TARGET_LEVELS[level_index])
}

fn difference(checkpoint: String, field_path: Vec<String>) -> Difference {
    Difference {
        classification: DifferenceClass::Rule,
        character: None,
        checkpoint,
        stream: Some(Stream::Broadcast("swings".into())),
        packet_index: None,
        field_path,
        opcode: Some(0x014a),
        deviations: vec![],
        rule: String::new(),
        reason: String::new(),
        expected: None,
        actual: None,
    }
}

fn expected_count(level_index: usize, category: usize) -> u64 {
    let (numerator, denominator) = EXPECTED[level_index][category];
    (u128::from(numerator) * u128::from(SAMPLES_PER_LEVEL) / u128::from(denominator)) as u64
}

/// Exact equal-tail binomial tests; this context reserves its own 1e-6 family over 42 tests.
pub fn compare(counts: &Counts, level_index: usize, slot: &str) -> Result<Vec<Difference>> {
    ensure!(
        level_index < 2 && counts.trials() == SAMPLES_PER_LEVEL,
        "incomplete queued strike sample"
    );
    let alpha = FAMILY_ALPHA / f64::from(TESTS_PER_SUBMISSION);
    let mut differences = Vec::new();
    for (category, name) in CATEGORIES.iter().enumerate() {
        let (numerator, denominator) = EXPECTED[level_index][category];
        let actual = counts.0[category];
        let p = wire_client::rolls::binomial_p_ratio_upper(
            SAMPLES_PER_LEVEL,
            actual,
            numerator,
            denominator,
        )?;
        if p <= alpha {
            differences.push(Difference {
                rule: DISTRIBUTION_RULE.into(),
                reason: format!("{actual}/{SAMPLES_PER_LEVEL}, expected {numerator}/{denominator}; equal-tail binomial p upper={p:.12e}; Bonferroni alpha={alpha:.12e}"),
                expected: Some(RecordedField::Unsigned(expected_count(level_index, category))),
                actual: Some(RecordedField::Unsigned(actual.into())),
                ..difference(
                    checkpoint(level_index, slot),
                    vec!["outcomes".into(), (*name).into()],
                )
            });
        }
    }
    Ok(differences)
}

/// The one signature a level and seed emit when a delivery gap stops its sample.
pub fn blocked(counts: &Counts, level_index: usize, slot: &str, reason: &str) -> Difference {
    Difference {
        rule: SAMPLE_RULE.into(),
        reason: format!("full queued strike sample is blocked; rates are unmeasured: {reason}"),
        expected: Some(RecordedField::Unsigned(SAMPLES_PER_LEVEL.into())),
        actual: Some(RecordedField::Unsigned(counts.trials().into())),
        ..difference(checkpoint(level_index, slot), vec!["outcomes".into()])
    }
}

/// Whether the scoreboard should accept a Tier 1 difference as a queued strike measurement.
pub fn accepts(difference: &Difference) -> bool {
    let shape = difference.classification == DifferenceClass::Rule
        && difference.character.is_none()
        && difference.stream == Some(Stream::Broadcast("swings".into()))
        && difference.opcode == Some(0x014a)
        && difference.packet_index.is_none();
    let Some(level_index) = (0..2).find(|level| {
        ["public_1", "public_2", "merge"]
            .iter()
            .any(|slot| difference.checkpoint == checkpoint(*level, slot))
    }) else {
        return false;
    };
    let count = |field: &Option<RecordedField>| match field {
        Some(RecordedField::Unsigned(count)) => Some(*count),
        _ => None,
    };
    if !shape || difference.field_path.first().map(String::as_str) != Some("outcomes") {
        return false;
    }
    if difference.rule == SAMPLE_RULE {
        return difference.field_path.len() == 1
            && count(&difference.expected) == Some(SAMPLES_PER_LEVEL.into())
            && count(&difference.actual).is_some_and(|n| n < SAMPLES_PER_LEVEL.into());
    }
    let category = difference
        .field_path
        .get(1)
        .and_then(|name| CATEGORIES.iter().position(|category| category == name));
    difference.rule == DISTRIBUTION_RULE
        && difference.field_path.len() == 2
        && category.is_some_and(|category| {
            count(&difference.expected) == Some(expected_count(level_index, category))
        })
        && count(&difference.actual).is_some_and(|n| n <= SAMPLES_PER_LEVEL.into())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

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
        for probabilities in EXPECTED {
            let table: u64 = probabilities[..BLOCK].iter().map(|(n, _)| n).sum();
            assert_eq!(table, 10_000);
            let (block, denominator) = probabilities[BLOCK];
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
        assert_eq!(counts, Counts([1, 0, 0, 1, 1, 0, 1]));
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
        let difference = blocked(&counts, 1, "public_2", "no swing state");
        assert!(accepts(&difference));
        assert_eq!(difference.checkpoint, "queued_strike_level_63_public_2");
    }

    #[test]
    fn the_scoreboard_accepts_only_derived_distribution_signatures() -> Result<()> {
        let mut counts = Counts([0, 0, 0, 0, 0, SAMPLES_PER_LEVEL, 0]);
        let differences = compare(&counts, 0, "merge")?;
        assert!(differences.iter().all(accepts));
        assert!(differences
            .iter()
            .any(|d| d.field_path == ["outcomes", "hit"]));
        let mut forged = differences[0].clone();
        forged.expected = Some(RecordedField::Unsigned(1));
        assert!(!accepts(&forged));
        counts.0[5] -= 1;
        assert!(compare(&counts, 0, "merge").is_err());
        Ok(())
    }
}
