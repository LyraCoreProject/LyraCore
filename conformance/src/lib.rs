mod schema;

pub use schema::{
    Behavior, DataDependentField, DataStream, DataValueType, DeliberateDeviation, RealClient, Rung,
    SourceReference, TestEvidence,
};

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

#[derive(Debug)]
pub struct FeatureLedger {
    behaviors: Vec<Behavior>,
}

impl FeatureLedger {
    pub fn load(features: &Path) -> Result<Self> {
        let mut sources = Vec::new();
        for system in
            fs::read_dir(features).with_context(|| format!("read {}", features.display()))?
        {
            let system = system?;
            ensure!(
                system.file_type()?.is_dir(),
                "expected a system directory: {}",
                system.path().display()
            );
            for file in fs::read_dir(system.path())? {
                let file = file?;
                ensure!(
                    file.file_type()?.is_file()
                        && file.path().extension().is_some_and(|ext| ext == "toml"),
                    "expected a Behavior TOML file: {}",
                    file.path().display()
                );
                let path = file.path();
                sources.push((
                    path.strip_prefix(features)?
                        .to_str()
                        .context("non-UTF-8 ledger path")?
                        .to_owned(),
                    fs::read_to_string(&path)
                        .with_context(|| format!("read {}", path.display()))?,
                ));
            }
        }
        Self::from_sources(
            sources
                .iter()
                .map(|(path, source)| (path.as_str(), source.as_str())),
        )
    }

    // Paths are relative to conformance/features; all entries are checked together.
    pub fn from_sources<'a>(sources: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<Self> {
        let mut behaviors = Vec::new();
        for (path, source) in sources {
            let behavior: Behavior =
                toml::from_str(source).with_context(|| format!("parse {path}"))?;
            behavior
                .validate()
                .with_context(|| format!("validate {path}"))?;
            ensure!(
                path == format!("{}/{}.toml", behavior.system, behavior.id),
                "{path}: expected {}/{}.toml",
                behavior.system,
                behavior.id
            );
            behaviors.push(behavior);
        }
        ensure!(!behaviors.is_empty(), "Feature Ledger is empty");
        behaviors.sort_by(|left, right| left.id.cmp(&right.id));
        let mut by_id = BTreeMap::new();
        for behavior in &behaviors {
            ensure!(
                by_id.insert(behavior.id.as_str(), behavior).is_none(),
                "duplicate Behavior id: {}",
                behavior.id
            );
        }
        let mut done = BTreeSet::new();
        for behavior in &behaviors {
            visit_dependencies(&behavior.id, &by_id, &mut BTreeSet::new(), &mut done)?;
        }
        Ok(Self { behaviors })
    }

    pub fn behaviors(&self) -> &[Behavior] {
        &self.behaviors
    }

    pub fn validate_scenarios<'a>(
        &self,
        scenarios: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<()> {
        let mut catalogue = BTreeMap::new();
        for (id, behavior) in scenarios {
            ensure!(!id.trim().is_empty(), "empty scenario id in catalogue");
            ensure!(
                catalogue.insert(id, behavior).is_none(),
                "duplicate scenario id in catalogue: {id}"
            );
        }
        for behavior in &self.behaviors {
            for id in &behavior.scenarios {
                ensure!(
                    catalogue.contains_key(id.as_str()),
                    "{} names an absent scenario: {id}",
                    behavior.id
                );
                ensure!(
                    catalogue[id.as_str()] == behavior.id,
                    "scenario {id} names Behavior {}, but {} lists it",
                    catalogue[id.as_str()],
                    behavior.id
                );
            }
        }
        Ok(())
    }
}

fn visit_dependencies<'a>(
    id: &'a str,
    by_id: &BTreeMap<&str, &'a Behavior>,
    visiting: &mut BTreeSet<&'a str>,
    done: &mut BTreeSet<&'a str>,
) -> Result<()> {
    if done.contains(id) {
        return Ok(());
    }
    ensure!(visiting.insert(id), "Behavior dependency cycle at {id}");
    let Some(behavior) = by_id.get(id) else {
        bail!("unknown Behavior dependency: {id}");
    };
    for dependency in &behavior.dependencies {
        visit_dependencies(dependency, by_id, visiting, done)
            .with_context(|| format!("{} depends on {dependency}", behavior.id))?;
    }
    visiting.remove(id);
    done.insert(id);
    Ok(())
}
