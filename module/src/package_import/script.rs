//! The script Import Family's half of an apply: reading a plan of Script Artifacts and reconciling
//! `game_script` to exactly the scripts the enabled Packages ship.
//!
//! The shared shell in the parent module decides what every family shares — the operator gate, the
//! payload separator, and the provenance rewrite. What is here is only what is true of scripts, and
//! that is more than it is for a claim family, because scripts differ in two ways that matter.
//!
//! # No base import
//!
//! Every other family's apply is the LAST stage of a reload: the base rows come back from a DBC or
//! a dump, then the Packages' claims go on top. No DBC and no dump holds a Runtime Script, so this
//! family's whole content is the plan. There is nothing to reload first, which is why an orchestrator
//! calls this family on its own rather than behind a reimport.
//!
//! # Whole rows, so conflicts are collisions
//!
//! A Package owns each script it ships outright. Two Packages meeting on one `script_id` or one
//! name is not a merge with a disagreement in it — it is two Packages claiming one identity, and
//! only a human can settle it. `trace_scripts` finds every such collision and this module refuses
//! the whole plan before it writes.
//!
//! # Reconciliation
//!
//! The Package script range is the whole table by construction: the artifact parser refuses a
//! `script_id` outside it, so nothing else can put a row here. Clearing the range and rewriting it
//! is therefore total — a Package that left the enabled set takes its scripts with it, a renamed
//! script cannot leave the old name behind, and applying one plan twice lands the same rows.

use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{
    is_package_script_id, trace_scripts, ScriptArtifact, ScriptTrace, TracedScript,
};

use crate::runtime_script::{with_host, RuntimeScriptHost};
use crate::script_binding::{game_script, Script};

use super::{PlannedPackage, RowCounts, ARTIFACT_SEPARATOR};

/// Apply the whole enabled plan for the script family.
///
/// Refuses before the first write on an artifact that does not parse, two artifacts naming one
/// Package, or any collision between Packages. Then clears the Package script range and rewrites
/// it, so what is in the table afterwards is exactly what the plan said and nothing else.
pub(super) fn apply(ctx: &ReducerContext, packed: &str) -> Result<Vec<PlannedPackage>, String> {
    let plan = ScriptPlan::read(packed)?;

    clear_package_range(ctx);
    for traced in plan.trace.scripts() {
        write_script(ctx, traced)?;
    }

    // The sources on this shard just changed, which is the one moment the compiler cache can be
    // sure it is holding chunks nobody will ask for again. See `RuntimeScriptHost::clear_chunks`.
    with_host(RuntimeScriptHost::clear_chunks);

    Ok(plan.packages)
}

/// Removes every Runtime Script a Package shipped.
///
/// Filtered by the band rather than a whole-table wipe, so this states the same contract every
/// family's clear does: an apply owns the Package range and nothing outside it. The parser makes
/// the range the whole table, so today the filter never spares a row — which is the point. If a
/// row ever appears outside the band it was not put there by a Package, and an apply is not the
/// thing that should decide its fate.
fn clear_package_range(ctx: &ReducerContext) {
    let scripts = ctx.db.game_script();
    let stale: Vec<u32> = scripts
        .iter()
        .filter(|script| is_package_script_id(script.script_id))
        .map(|script| script.script_id)
        .collect();
    for script_id in stale {
        scripts.script_id().delete(script_id);
    }
}

fn write_script(ctx: &ReducerContext, traced: &TracedScript) -> Result<(), String> {
    let script = &traced.script;
    ctx.db
        .game_script()
        .try_insert(Script {
            script_id: script.script_id(),
            name: script.name().to_string(),
            package: traced.package.to_string(),
            source_hash: traced.source_hash.to_string(),
            // Derived here rather than read from the artifact, so it cannot disagree with the Lua
            // it describes. It is also the Host's compiler-cache key.
            content_hash: blake3::hash(script.source().as_bytes())
                .to_hex()
                .to_string(),
            event: script.event().as_str().to_string(),
            priority: script.priority(),
            enabled: script.enabled(),
            source: script.source().to_string(),
        })
        .map_err(|e| {
            format!(
                "`game_script` row {} (`{}`) did not insert: {e}",
                script.script_id(),
                script.name()
            )
        })?;
    Ok(())
}

// ===========================================================================================
//  The plan — pure. Everything that can refuse the payload refuses here, before any write.
// ===========================================================================================

/// Every enabled Package's scripts, merged, and who contributed what.
#[derive(Debug)]
struct ScriptPlan {
    packages: Vec<PlannedPackage>,
    trace: ScriptTrace,
}

impl ScriptPlan {
    /// Reads a payload into a plan, refusing anything that must not reach the table.
    ///
    /// The refusals, in order: an artifact that does not parse, two artifacts naming the same
    /// Package, and any collision between Packages. The shared tracer reports EVERY disagreement,
    /// not just the first, because the operator fixing them wants the whole list in one pass.
    fn read(packed: &str) -> Result<Self, String> {
        let mut artifacts: Vec<ScriptArtifact> = Vec::new();
        for (index, artifact) in packed
            .split(ARTIFACT_SEPARATOR)
            .filter(|line| !line.is_empty())
            .enumerate()
        {
            let parsed = ScriptArtifact::parse(artifact)
                .map_err(|e| format!("Script Artifact {index} in this plan is invalid: {e}"))?;
            artifacts.push(parsed);
        }

        let trace = trace_scripts(&artifacts);
        if !trace.is_clear() {
            let report = trace
                .conflicts()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            return Err(format!(
                "{} Runtime Script conflicts, nothing applied: {report}",
                trace.conflicts().len()
            ));
        }

        let packages = artifacts
            .iter()
            .map(|artifact| PlannedPackage {
                package: artifact.package().to_string(),
                artifact_hash: blake3::hash(artifact.to_canonical_json().as_bytes())
                    .to_hex()
                    .to_string(),
                source_hash: artifact.source_hash().to_string(),
                counts: RowCounts {
                    // A Package never tunes another Package's script, so nothing here is an update.
                    // Every script it ships is a row it owns, disabled ones included: the row is on
                    // the shard either way.
                    inserted_rows: artifact.scripts().len() as u64,
                    ..RowCounts::default()
                },
            })
            .collect();

        Ok(Self { packages, trace })
    }
}

// ===========================================================================================
//  Tests — the pure half. Row WRITING needs a live ReducerContext, which a native test has no
//  way to build (same limit `import_meta`'s tests note); `tests/package_runtime_scripts.rs`
//  covers that rung against a published module.
// ===========================================================================================
#[cfg(test)]
mod tests {
    use super::ScriptPlan;
    use crate::package_import::ARTIFACT_SEPARATOR;

    const HASH_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn script(script_id: u32, name: &str, event: &str, priority: i32, enabled: bool) -> String {
        format!(
            r#"{{"script_id":{script_id},"name":"{name}","event":"{event}","priority":{priority},"enabled":{enabled},"source":"grant_xp(event.actor, 10)"}}"#
        )
    }

    fn artifact(package: &str, scripts: &[String]) -> String {
        format!(
            r#"{{"kind":"script","version":1,"package":"{package}","source_hash":"{HASH_A}","scripts":[{}]}}"#,
            scripts.join(",")
        )
    }

    fn plan(artifacts: &[String]) -> Result<ScriptPlan, String> {
        ScriptPlan::read(&artifacts.join(&ARTIFACT_SEPARATOR.to_string()))
    }

    /// The payload format rests on this: a canonical artifact escapes every control character, so
    /// one line is one artifact. Lua source is the member most likely to hold a newline.
    #[test]
    fn a_canonical_script_artifact_never_contains_the_artifact_separator() {
        let awkward = format!(
            r#"{{"kind":"script","version":1,"package":"example.bolt","source_hash":"{HASH_A}","scripts":[{{"script_id":100001,"name":"bolt.multi","event":"on_login","priority":0,"enabled":true,"source":"local n = 1\nif n > 0 then\n\tgrant_xp(event.actor, n)\nend"}}]}}"#
        );

        let parsed =
            lyracore_package_delta::ScriptArtifact::parse(&awkward).expect("artifact parses");

        assert!(!parsed.to_canonical_json().contains(ARTIFACT_SEPARATOR));
    }

    /// An empty payload is the honest statement "no Package ships a Runtime Script any more", and
    /// it clears the range. It is never sent by accident.
    #[test]
    fn an_empty_payload_is_a_plan_that_ships_nothing() {
        let plan = ScriptPlan::read("").expect("an empty plan is valid");

        assert!(plan.packages.is_empty());
        assert!(plan.trace.scripts().is_empty());
    }

    #[test]
    fn a_plan_records_each_packages_hashes_and_row_count() {
        let plan = plan(&[artifact(
            "example.bolt",
            &[
                script(100_001, "bolt.greet", "on_login", 0, true),
                script(100_002, "bolt.bye", "on_logout", 0, false),
            ],
        )])
        .expect("plan builds");

        assert_eq!(plan.packages.len(), 1);
        let package = &plan.packages[0];
        assert_eq!(package.package, "example.bolt");
        assert_eq!(package.source_hash, HASH_A);
        assert_eq!(
            package.counts.inserted_rows, 2,
            "a disabled script is still a row this Package put on the shard"
        );
        assert_eq!(package.counts.updated_rows, 0);
        assert_ne!(
            package.artifact_hash, package.source_hash,
            "the artifact digest and the source digest are different facts"
        );
    }

    /// The digest is over the CANONICAL bytes, so how an artifact was spelled cannot change it.
    #[test]
    fn the_artifact_hash_ignores_how_the_artifact_was_written() {
        let compact = artifact(
            "example.bolt",
            &[script(100_001, "bolt.greet", "on_login", 0, true)],
        );
        let spaced = compact.replace(r#""kind""#, r#" "kind" "#);

        let from_compact = plan(&[compact]).expect("plan builds");
        let from_spaced = plan(&[spaced]).expect("plan builds");

        assert_eq!(
            from_compact.packages[0].artifact_hash,
            from_spaced.packages[0].artifact_hash
        );
    }

    #[test]
    fn an_artifact_that_does_not_parse_refuses_the_whole_plan() {
        let good = artifact(
            "example.good",
            &[script(100_001, "good.greet", "on_login", 0, true)],
        );

        let refusal =
            plan(&[good, r#"{"kind":"script","version":9}"#.to_owned()]).expect_err("plan refused");

        assert!(refusal.contains("Script Artifact 1"), "{refusal}");
    }

    /// A Package Delta packed into a script plan must be named for what it is, not complained
    /// about member by member — both artifacts live in one directory.
    #[test]
    fn a_package_delta_in_a_script_plan_is_refused_by_name() {
        let delta = concat!(
            r#"{"version":1,"package":"example.bolt","#,
            r#""source_hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","#,
            r#""claims":[{"table":"game_spell","key":{"spell_id":133},"operation":"update","#,
            r#""fields":{"cooldown_ms":{"type":"u32","value":1500}}}]}"#,
        );

        let refusal = plan(&[delta.to_owned()]).expect_err("plan is refused");

        assert!(refusal.contains("a Package Delta"), "{refusal}");
    }

    #[test]
    fn one_package_sent_twice_refuses_the_whole_plan() {
        let once = artifact(
            "example.bolt",
            &[script(100_001, "bolt.greet", "on_login", 0, true)],
        );

        let refusal = plan(&[once.clone(), once]).expect_err("plan is refused");

        assert!(
            refusal.contains("more than one Script Artifact"),
            "{refusal}"
        );
    }

    #[test]
    fn two_packages_shipping_one_script_id_refuse_the_whole_plan() {
        let first = artifact(
            "example.first",
            &[script(100_001, "first.greet", "on_login", 0, true)],
        );
        let second = artifact(
            "example.second",
            &[script(100_001, "second.greet", "on_login", 0, true)],
        );

        let refusal = plan(&[first, second]).expect_err("plan is refused");

        assert!(refusal.contains("Runtime Script conflicts"), "{refusal}");
        assert!(refusal.contains("example.first"), "{refusal}");
        assert!(refusal.contains("example.second"), "{refusal}");
    }

    #[test]
    fn two_packages_shipping_one_script_name_refuse_the_whole_plan() {
        let first = artifact(
            "example.first",
            &[script(100_001, "shared.greet", "on_login", 0, true)],
        );
        let second = artifact(
            "example.second",
            &[script(100_002, "shared.greet", "on_login", 0, true)],
        );

        let refusal = plan(&[first, second]).expect_err("plan is refused");

        assert!(refusal.contains("shared.greet"), "{refusal}");
    }

    /// Several Packages binding to one event is the normal case, not a conflict — that is what
    /// `priority` is for.
    #[test]
    fn two_packages_binding_to_one_event_merge_into_one_plan() {
        let first = artifact(
            "example.first",
            &[script(100_001, "first.greet", "on_login", 10, true)],
        );
        let second = artifact(
            "example.second",
            &[script(100_002, "second.greet", "on_login", 5, true)],
        );

        let plan = plan(&[first, second]).expect("plan builds");

        assert_eq!(plan.trace.scripts().len(), 2);
        assert_eq!(plan.packages.len(), 2);
    }

    /// The written order is the plan order, whatever order the Packages arrived in — two Shards
    /// given one plan write the same rows.
    #[test]
    fn the_plan_is_ordered_by_identifier_however_the_packages_arrived() {
        let alpha = artifact(
            "example.alpha",
            &[script(100_009, "alpha.a", "on_login", 0, true)],
        );
        let zeta = artifact(
            "example.zeta",
            &[script(100_002, "zeta.z", "on_login", 0, true)],
        );

        let forwards = plan(&[alpha.clone(), zeta.clone()]).expect("plan builds");
        let backwards = plan(&[zeta, alpha]).expect("plan builds");

        let ids = |plan: &ScriptPlan| -> Vec<u32> {
            plan.trace
                .scripts()
                .iter()
                .map(|t| t.script.script_id())
                .collect()
        };
        assert_eq!(ids(&forwards), [100_002, 100_009]);
        assert_eq!(ids(&forwards), ids(&backwards));
    }

    /// Every traced script carries the Package that ships it onto the row, so a Shard records who
    /// put each script there.
    #[test]
    fn every_planned_script_carries_its_package_and_source_revision() {
        let plan = plan(&[artifact(
            "example.bolt",
            &[script(100_001, "bolt.greet", "on_login", 0, true)],
        )])
        .expect("plan builds");

        let traced = &plan.trace.scripts()[0];
        assert_eq!(traced.package.as_str(), "example.bolt");
        assert_eq!(traced.source_hash.as_str(), HASH_A);
    }
}
