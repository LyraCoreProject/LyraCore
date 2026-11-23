//! Statistics shared by the weapon Roll Conformance contexts; ADR 0007 derives every probability.

use anyhow::{ensure, Result};
use serde::{Serialize, Serializer};
use wire_client::compare::{Difference, DifferenceClass};
use wire_client::scenario::Stream;
use wire_client::trace::RecordedField;

pub const SAMPLES_PER_LEVEL: u32 = 50_000;
pub const TARGET_LEVELS: [u32; 2] = [60, 63];
const SLOTS: [&str; 3] = ["public_1", "public_2", "merge"];
const FAMILY_ALPHA: f64 = 1e-6;
// `roll_chance_f(5.0)` accepts 214748365 of the 2^32 uniform draws.
pub const BLOCK_DRAWS: u64 = 214_748_365;

/// Outcome counts; the last category is a partial block that overlaps the landed table outcomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Counts<const N: usize>(pub [u32; N]);

impl<const N: usize> Default for Counts<N> {
    fn default() -> Self {
        Self([0; N])
    }
}

impl<const N: usize> Serialize for Counts<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_struct("Counts", &self.0[..])
    }
}

impl<const N: usize> Counts<N> {
    pub const fn new(counts: [u32; N]) -> Self {
        Self(counts)
    }

    pub fn trials(&self) -> u32 {
        self.0[..N - 1].iter().sum()
    }

    pub fn add(&mut self, other: &Self) {
        for (total, count) in self.0.iter_mut().zip(other.0) {
            *total += count;
        }
    }

    pub fn record(&mut self, category: usize, blocked: bool) {
        self.0[category] += 1;
        self.0[N - 1] += u32::from(blocked);
    }
}

/// One weapon context: its name, delivery stream and opcode, categories and exact rates.
pub struct StrikeContext<const N: usize> {
    pub name: &'static str,
    pub stream: &'static str,
    pub opcode: u16,
    pub categories: [&'static str; N],
    /// Exact probabilities as (numerator, denominator), by target level then category.
    pub expected: [[(u64, u64); N]; 2],
}

impl<const N: usize> StrikeContext<N> {
    pub const fn block(&self) -> usize {
        N - 1
    }

    pub fn label(&self) -> String {
        self.name.replace('_', " ")
    }

    pub fn slug(&self) -> String {
        self.name.replace('_', "-")
    }

    fn distribution_rule(&self) -> String {
        format!("{}_distribution", self.name)
    }

    fn sample_rule(&self) -> String {
        format!("{}_sample", self.name)
    }

    fn checkpoint(&self, level_index: usize, slot: &str) -> String {
        format!("{}_level_{}_{slot}", self.name, TARGET_LEVELS[level_index])
    }

    fn difference(&self, checkpoint: String, field_path: Vec<String>) -> Difference {
        Difference {
            classification: DifferenceClass::Rule,
            character: None,
            checkpoint,
            stream: Some(Stream::Broadcast(self.stream.into())),
            packet_index: None,
            field_path,
            opcode: Some(self.opcode),
            deviations: vec![],
            rule: String::new(),
            reason: String::new(),
            expected: None,
            actual: None,
        }
    }

    fn expected_count(&self, level_index: usize, category: usize) -> u64 {
        let (numerator, denominator) = self.expected[level_index][category];
        (u128::from(numerator) * u128::from(SAMPLES_PER_LEVEL) / u128::from(denominator)) as u64
    }

    /// Exact equal-tail binomial tests; each context reserves its own 1e-6 family over its
    /// categories × levels × slots.
    pub fn compare(
        &self,
        counts: &Counts<N>,
        level_index: usize,
        slot: &str,
    ) -> Result<Vec<Difference>> {
        ensure!(
            level_index < TARGET_LEVELS.len() && counts.trials() == SAMPLES_PER_LEVEL,
            "incomplete {} sample",
            self.label()
        );
        let tests = N * TARGET_LEVELS.len() * SLOTS.len();
        let alpha = FAMILY_ALPHA / tests as f64;
        let mut differences = Vec::new();
        for (category, name) in self.categories.iter().enumerate() {
            let (numerator, denominator) = self.expected[level_index][category];
            let actual = counts.0[category];
            let p = wire_client::rolls::binomial_p_ratio_upper(
                SAMPLES_PER_LEVEL,
                actual,
                numerator,
                denominator,
            )?;
            if p <= alpha {
                differences.push(Difference {
                    rule: self.distribution_rule(),
                    reason: format!("{actual}/{SAMPLES_PER_LEVEL}, expected {numerator}/{denominator}; equal-tail binomial p upper={p:.12e}; Bonferroni alpha={alpha:.12e}"),
                    expected: Some(RecordedField::Unsigned(self.expected_count(level_index, category))),
                    actual: Some(RecordedField::Unsigned(actual.into())),
                    ..self.difference(
                        self.checkpoint(level_index, slot),
                        vec!["outcomes".into(), (*name).into()],
                    )
                });
            }
        }
        Ok(differences)
    }

    /// The one signature a level and seed emit when a delivery gap stops its sample.
    pub fn blocked(
        &self,
        counts: &Counts<N>,
        level_index: usize,
        slot: &str,
        reason: &str,
    ) -> Difference {
        Difference {
            rule: self.sample_rule(),
            reason: format!(
                "full {} sample is blocked; rates are unmeasured: {reason}",
                self.label()
            ),
            expected: Some(RecordedField::Unsigned(SAMPLES_PER_LEVEL.into())),
            actual: Some(RecordedField::Unsigned(counts.trials().into())),
            ..self.difference(self.checkpoint(level_index, slot), vec!["outcomes".into()])
        }
    }

    /// Whether the scoreboard should accept a Tier 1 difference as a measurement of this context.
    pub fn accepts(&self, difference: &Difference) -> bool {
        let shape = difference.classification == DifferenceClass::Rule
            && difference.character.is_none()
            && difference.stream == Some(Stream::Broadcast(self.stream.into()))
            && difference.opcode == Some(self.opcode)
            && difference.packet_index.is_none();
        let Some(level_index) = (0..TARGET_LEVELS.len()).find(|level| {
            SLOTS
                .iter()
                .any(|slot| difference.checkpoint == self.checkpoint(*level, slot))
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
        if difference.rule == self.sample_rule() {
            return difference.field_path.len() == 1
                && count(&difference.expected) == Some(SAMPLES_PER_LEVEL.into())
                && count(&difference.actual).is_some_and(|n| n < SAMPLES_PER_LEVEL.into());
        }
        let category = difference
            .field_path
            .get(1)
            .and_then(|name| self.categories.iter().position(|category| category == name));
        difference.rule == self.distribution_rule()
            && difference.field_path.len() == 2
            && category.is_some_and(|category| {
                count(&difference.expected) == Some(self.expected_count(level_index, category))
            })
            && count(&difference.actual).is_some_and(|n| n <= SAMPLES_PER_LEVEL.into())
    }
}
