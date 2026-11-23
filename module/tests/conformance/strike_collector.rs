use std::sync::Barrier;

use anyhow::{ensure, Context, Result};
use lyracore_test_support::{core_root, Standalone};
use wire_client::compare::Difference;
use wire_client::rolls::PUBLIC_SEEDS;
use wire_client::scenario::Scenario;

use super::strike_rolls::{Counts, StrikeContext};

pub(super) const CMSG_CAST_SPELL: u32 = 0x12e;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum Sample<const N: usize> {
    Complete { counts: Counts<N> },
    Blocked { counts: Counts<N>, reason: String },
}

impl<const N: usize> Sample<N> {
    pub(super) fn counts(&self) -> &Counts<N> {
        match self {
            Self::Complete { counts } | Self::Blocked { counts, .. } => counts,
        }
    }
}

pub(super) struct Preparation<'a> {
    pub(super) ready: &'a Barrier,
    pub(super) arrived: bool,
}

impl Drop for Preparation<'_> {
    fn drop(&mut self) {
        if !self.arrived {
            self.ready.wait();
        }
    }
}

pub(super) fn cast_request(spell: u32, target: u64) -> Vec<u8> {
    let mut body = spell.to_le_bytes().to_vec();
    body.extend(0x0002u16.to_le_bytes());
    let guid = target.to_le_bytes();
    body.push(
        guid.iter()
            .enumerate()
            .filter(|(_, byte)| **byte != 0)
            .fold(0u8, |mask, (index, _)| mask | 1 << index),
    );
    body.extend(guid.iter().filter(|byte| **byte != 0));
    body
}

/// Runs one shard of a level's Characters; every collector reaches the barrier once.
pub(super) type Collect<const N: usize> =
    fn(&Standalone, &str, &Scenario, u64, usize, usize, &Barrier) -> Result<Sample<N>>;

pub(super) struct Measurement<const N: usize> {
    pub(super) context: &'static StrikeContext<N>,
    pub(super) shards: &'static str,
    pub(super) collect: Collect<N>,
}

impl<const N: usize> Measurement<N> {
    fn measure(&self, scenario: &Scenario, seed: u64, suffix: &str) -> Result<[Sample<N>; 2]> {
        let (collect, label) = (self.collect, self.context.label());
        let mut shard = Standalone::start_persistent(&format!("{}-{suffix}", self.context.slug()));
        let names = super::rolls::fresh_shards(&mut shard, self.shards, 4);
        let ready = Barrier::new(4);
        let parts =
            std::thread::scope(|scope| -> Result<Vec<Sample<N>>> {
                let mut jobs = Vec::new();
                for level in 0..2 {
                    for part in 0..2 {
                        let shard = &shard;
                        let name = &names[level * 2 + part];
                        let ready = &ready;
                        jobs.push(scope.spawn(move || {
                            collect(shard, name, scenario, seed, level, part, ready)
                        }));
                    }
                }
                jobs.into_iter()
                    .map(|job| {
                        job.join()
                            .map_err(|_| anyhow::anyhow!("{label} collector panicked"))?
                    })
                    .collect()
            })?;
        Ok(std::array::from_fn(|level| {
            let pair = &parts[level * 2..level * 2 + 2];
            let mut counts = pair[0].counts().clone();
            counts.add(pair[1].counts());
            let blockers: Vec<_> = pair
                .iter()
                .filter_map(|sample| match sample {
                    Sample::Blocked { reason, .. } => Some(reason.as_str()),
                    Sample::Complete { .. } => None,
                })
                .collect();
            if blockers.is_empty() {
                Sample::Complete { counts }
            } else {
                Sample::Blocked {
                    counts,
                    reason: blockers.join("; "),
                }
            }
        }))
    }

    fn compare_samples(&self, samples: &[Sample<N>; 2], slot: &str) -> Result<Vec<Difference>> {
        let mut differences = Vec::new();
        for (level, sample) in samples.iter().enumerate() {
            match sample {
                Sample::Complete { counts } => {
                    differences.extend(self.context.compare(counts, level, slot)?);
                }
                Sample::Blocked { counts, reason } => {
                    differences.push(self.context.blocked(counts, level, slot, reason));
                }
            }
        }
        Ok(differences)
    }

    /// Collects both public seeds; their rule signatures join `white_melee_outcomes`.
    pub(super) fn public_seeds(&self, scenario: &Scenario) -> Result<Vec<Difference>> {
        let (slug, label) = (self.context.slug(), self.context.label());
        let mut measurements = Vec::new();
        for seed in PUBLIC_SEEDS {
            measurements.push(self.measure(scenario, seed, "public")?);
        }
        let report = core_root().join(format!(".lyracore/conformance/{slug}-rates.json"));
        std::fs::create_dir_all(
            report
                .parent()
                .with_context(|| format!("{label} report parent"))?,
        )?;
        std::fs::write(&report, serde_json::to_vec_pretty(&measurements)?)?;
        let complete = measurements
            .iter()
            .flatten()
            .all(|sample| matches!(sample, Sample::Complete { .. }));
        if complete && std::env::var_os("LYRACORE_ROLL_REPEATABILITY").is_some() {
            ensure!(
                self.measure(scenario, PUBLIC_SEEDS[0], "repeat")? == measurements[0],
                "same seed changed delivered {label} counts"
            );
        }
        if complete {
            ensure!(
                measurements[0] != measurements[1],
                "different seed did not change delivered {label} counts"
            );
        }
        let mut differences = Vec::new();
        for (slot, samples) in measurements.iter().enumerate() {
            differences.extend(self.compare_samples(samples, &format!("public_{}", slot + 1))?);
        }
        Ok(differences)
    }

    pub(super) fn merge_seed(&self, scenario: &Scenario, seed: u64) -> Result<Vec<Difference>> {
        let samples = self.measure(scenario, seed, "merge")?;
        println!(
            "{} merge {seed:#018x}: {}",
            self.context.label(),
            serde_json::to_string(&samples)?
        );
        self.compare_samples(&samples, "merge")
    }
}
