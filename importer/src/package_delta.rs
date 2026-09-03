//! The Package Delta stage: read every enabled Package's generated artifacts, report the plan they
//! form, and hand the whole plan to the module in one reducer call.
//!
//! A base import replaces a whole Import Family, so a Package's claims have to be reapplied after
//! it. This stage runs last, once the family's base rows are back and its provenance is stamped.
//!
//! # Where the artifacts live
//!
//! `<enabled packages root>/<package>/data/.generated/*.json`. The path sits INSIDE the Package
//! folder on purpose: `lyracore packages enable`/`disable` moves that folder between `packages/` and
//! `.lyracore/packages-disabled/`, so the artifacts move with it. What this stage can see IS the
//! enabled set — there is no second list to disagree with the Package Inventory.
//!
//! # Check and apply
//!
//! The importer's own convention decides which: without `--apply` this prints the plan and writes
//! nothing, with `--apply` it calls the reducer. The plan printed by a check is the plan an apply
//! sends.
//!
//! # Routing a spatial claim
//!
//! Most claimed tables are global catalogues every Shard loads whole. Two are SPATIAL — a creature
//! spawn and a gameobject spawn — and each names the map it sits on in its own key. A spatial claim
//! belongs to this Shard exactly when this run's World Import Scope owns that map, which is the same
//! fence the base import filters its own spawns through (`creature_row_kept_in_scope`). Routing
//! reads that scope and nothing else: there is no second concept, and no per-Package Shard list to
//! disagree with it.
//!
//! A claim for another Shard's map is DROPPED from this plan, not refused. Refusing it would make a
//! realm of several Shards impossible to import, because every Shard would choke on the maps it does
//! not own. The report names what was routed away, so a check still accounts for every claim.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use lyracore_package_delta::{
    artifact_kind, trace, ArtifactKind, ClaimTrace, Operation, PackageDelta,
};

use crate::world_import_scope::WorldImportScope;
use crate::{call_reducer_args, Args};

/// Where a Package's generated Delta artifacts live, relative to the Package folder.
const GENERATED_DIR: &str = "data/.generated";

/// The reducer that applies a whole family's enabled plan in one transaction.
const APPLY_REDUCER: &str = "apply_package_deltas";

/// One artifact, and the file it came from so a refusal can name it.
#[derive(Debug)]
struct Artifact {
    path: PathBuf,
    delta: PackageDelta,
}

/// Read the enabled Packages' artifacts for `family`, report the plan, and apply it when the caller
/// asked to write.
///
/// Refuses the whole stage on an unreadable root, an invalid artifact, or a Claim Conflict — the
/// module would refuse the same plan, and refusing here names the file.
pub(crate) fn reapply(args: &Args, family: &str, root: &str) -> Result<()> {
    let scope = args.world_import_scope()?;
    let for_family = artifacts_for_family(read_enabled(Path::new(root))?, family)?;
    let (artifacts, routed_away) = routed_to_this_shard(&scope, for_family)?;
    let deltas: Vec<PackageDelta> = artifacts.iter().map(|a| a.delta.clone()).collect();
    let traced = trace(&deltas);

    print!(
        "{}",
        plan_report(root, scope.name(), routed_away, &artifacts, &traced)
    );

    if !traced.is_clear() {
        bail!(
            "{}: {} claim conflicts between enabled Packages — nothing applied",
            family,
            traced.conflicts().len()
        );
    }

    if !args.apply {
        eprintln!(
            "{family}: Package Delta check only — re-run with --apply to reapply these claims."
        );
        return Ok(());
    }

    call_reducer_args(args, APPLY_REDUCER, &[family, &pack(&deltas)])
        .with_context(|| format!("{APPLY_REDUCER}({family})"))?;
    eprintln!(
        "{family}: reapplied {} enabled Package Delta(s) over {} row(s).",
        deltas.len(),
        traced.rows().len()
    );
    Ok(())
}

/// Keep only the claims owned by `family`, preserving the Package and source identities carried by
/// the original artifact. One Package Delta may span several Import Families, but each reducer call
/// applies exactly one family.
fn artifacts_for_family(artifacts: Vec<Artifact>, family: &str) -> Result<Vec<Artifact>> {
    artifacts
        .into_iter()
        .filter_map(|artifact| {
            let claims = artifact
                .delta
                .claims()
                .iter()
                .filter(|claim| claim.table().family() == family)
                .cloned()
                .collect::<Vec<_>>();
            if claims.is_empty() {
                return None;
            }
            Some(
                PackageDelta::new(
                    artifact.delta.package().clone(),
                    artifact.delta.source_hash().clone(),
                    claims,
                )
                .map(|delta| Artifact {
                    path: artifact.path,
                    delta,
                })
                .map_err(anyhow::Error::from),
            )
        })
        .collect()
}

/// Keep only the claims this Shard owns, and count the spatial ones it does not.
///
/// A claim with no map is a global catalogue row and always stays. A spatial claim stays exactly
/// when the World Import Scope owns its map — `contains_map`, not `contains`: the base import's own
/// rule for a forced creature is that a placed entry bypasses bounded geometry but never the map
/// fence, and an authored spawn is placed the same way. A Package left with no claims at all drops
/// out of the plan, the way it does when it claims no table of this family.
fn routed_to_this_shard(
    scope: &WorldImportScope,
    artifacts: Vec<Artifact>,
) -> Result<(Vec<Artifact>, usize)> {
    let mut routed_away = 0usize;
    let mut kept = Vec::new();
    for artifact in artifacts {
        let claims: Vec<_> = artifact
            .delta
            .claims()
            .iter()
            .filter(|claim| match claim.key().map_id() {
                None => true,
                Some(map) => {
                    let owned = scope.contains_map(i64::from(map));
                    routed_away += usize::from(!owned);
                    owned
                }
            })
            .cloned()
            .collect();
        if claims.is_empty() {
            continue;
        }
        kept.push(Artifact {
            path: artifact.path,
            delta: PackageDelta::new(
                artifact.delta.package().clone(),
                artifact.delta.source_hash().clone(),
                claims,
            )?,
        });
    }
    Ok((kept, routed_away))
}

/// The warning a base import prints when the operator did not name an enabled Package root.
///
/// Silence would be the wrong answer twice over: the shard is now running base rows with no Package
/// claims on them, and an empty enabled set is a DIFFERENT statement that would have deleted every
/// row a Package invented. Neither may be guessed.
pub(crate) fn warn_not_reapplied(family: &str) {
    eprintln!(
        "{family}: WARNING — enabled Package Deltas were NOT reapplied. The base rows are back \
         without any Package's claims on them. Re-run with `--packages <enabled packages root>` \
         (normally `packages/`) to reapply them."
    );
}

/// Every enabled Package's artifacts, ordered by Package folder then by file name so the same tree
/// always produces the same plan.
///
/// A missing root is an error, never an empty set: "no Package claims this family" is a statement
/// the operator makes by pointing at a real Package Inventory that happens to hold no artifacts.
fn read_enabled(root: &Path) -> Result<Vec<Artifact>> {
    if !root.is_dir() {
        bail!(
            "enabled packages root `{}` is not a directory — name the directory holding the \
             enabled Packages (normally `packages/`)",
            root.display()
        );
    }

    let mut packages: Vec<PathBuf> = fs::read_dir(root)
        .with_context(|| format!("read {}", root.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_dir())
        .collect();
    packages.sort();

    let mut artifacts = Vec::new();
    for package in packages {
        let generated = package.join(GENERATED_DIR);
        if !generated.is_dir() {
            continue;
        }
        let mut files: Vec<PathBuf> = fs::read_dir(&generated)
            .with_context(|| format!("read {}", generated.display()))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        files.sort();

        for path in files {
            let json =
                fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
            // One Package ships every artifact kind it has into this one directory, so the glob
            // above finds a Script Artifact next to a Package Delta. Skipping it is not leniency:
            // this stage is the CLAIM half of an apply, and the script family has no base import to
            // run behind. `lyracore packages replay` applies that family separately. Anything this
            // router cannot read at all still goes to the parser, which names what is wrong with it.
            if matches!(
                artifact_kind(&json),
                Some(ArtifactKind::Script | ArtifactKind::Other(_))
            ) {
                continue;
            }
            let delta = PackageDelta::parse(&json)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            artifacts.push(Artifact { path, delta });
        }
    }
    Ok(artifacts)
}

/// The artifacts' canonical bytes, one per line — the payload shape `apply_package_deltas` reads.
/// Canonical rather than as-written, so the digest the module records describes what the artifact
/// SAYS rather than how someone spelled it.
fn pack(deltas: &[PackageDelta]) -> String {
    deltas
        .iter()
        .map(PackageDelta::to_canonical_json)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The plan, in the form a check prints and an apply announces: every Package with its file and row
/// counts, then every claimed row, then every conflict.
fn plan_report(
    root: &str,
    scope: &str,
    routed_away: usize,
    artifacts: &[Artifact],
    traced: &ClaimTrace,
) -> String {
    let mut out = format!("\n=== Package Deltas ({root}) ===\n");
    if routed_away > 0 {
        out.push_str(&format!(
            "  {routed_away} spatial claim(s) routed away: scope `{scope}` does not own their map\n"
        ));
    }
    if artifacts.is_empty() {
        out.push_str("  no enabled Package claims this import family\n");
    }
    for artifact in artifacts {
        let counts = artifact.delta.claim_counts();
        out.push_str(&format!(
            "  {:<32} {:>3} updated  {:>3} spells  {:>3} effects   {}\n",
            artifact.delta.package(),
            counts.updated_rows,
            counts.inserted_spells,
            counts.inserted_effects,
            artifact.path.display(),
        ));
    }

    for row in traced.rows() {
        let operation = match row.operation() {
            Operation::Insert => "insert",
            Operation::Update => "update",
        };
        let fields: Vec<String> = row
            .fields()
            .iter()
            .map(|(name, claimed)| format!("{name}={} [{}]", claimed.value, claimed.claimed_by))
            .collect();
        out.push_str(&format!(
            "  {operation} {} {} — {}\n",
            row.table(),
            row.key(),
            fields.join(", ")
        ));
    }

    for conflict in traced.conflicts() {
        out.push_str(&format!("  CONFLICT {conflict}\n"));
    }
    out.push_str("=======================================\n\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_import_scope::WorldImportProfile;

    const HASH_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const PACKAGE_SPELL: u32 = 6_000_001;
    const REAL_SPELL: u32 = 133;

    /// A unique scratch tree per test (std-only; no tempfile dep — mirrors `pack_client.rs`).
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir()
                .join(format!("package-delta-test-{}-{tag}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Scratch(p)
        }
        fn write(&self, rel: &str, text: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn update_artifact(package: &str, spell_id: u32, field: &str, value: u32) -> String {
        format!(
            r#"{{"version":1,"package":"{package}","source_hash":"{HASH_A}","claims":[{{"table":"game_spell","key":{{"spell_id":{spell_id}}},"operation":"update","fields":{{"{field}":{{"type":"u32","value":{value}}}}}}}]}}"#
        )
    }

    /// Eastern Kingdoms, inside the eastern profile's Elwynn slice.
    const EASTERN_MAP: u32 = 0;
    /// Kalimdor, which the eastern profile does not touch.
    const KALIMDOR_MAP: u32 = 1;
    /// Deadmines, the whole map the `instances` profile owns.
    const INSTANCE_MAP: u32 = 36;
    const REAL_CREATURE: u32 = 6;
    const PACKAGE_CREATURE: u32 = 15_000_001;

    fn spawn_claim(map_id: u32, spawn_id: u32) -> String {
        format!(
            r#"{{"table":"game_creature_spawn","key":{{"map_id":{map_id},"entry":{REAL_CREATURE},"spawn_id":{spawn_id}}},"operation":"update","fields":{{"x":{{"type":"f32","value":1.5}}}}}}"#
        )
    }

    /// One Package placing the same creature on two maps: the shape a two-Shard realm produces.
    fn two_map_artifact() -> String {
        format!(
            r#"{{"version":1,"package":"example.placer","source_hash":"{HASH_A}","claims":[{},{}]}}"#,
            spawn_claim(EASTERN_MAP, 15_000_001),
            spawn_claim(KALIMDOR_MAP, 15_000_002),
        )
    }

    fn instance_spawn_artifact() -> String {
        format!(
            r#"{{"version":1,"package":"example.placer","source_hash":"{HASH_A}","claims":[{}]}}"#,
            spawn_claim(INSTANCE_MAP, 15_000_003),
        )
    }

    fn creature_template_artifact() -> String {
        format!(
            r#"{{"version":1,"package":"example.placer","source_hash":"{HASH_A}","claims":[{{"table":"game_creature_template","key":{{"entry":{PACKAGE_CREATURE}}},"operation":"update","fields":{{"level":{{"type":"u32","value":12}}}}}}]}}"#
        )
    }

    /// One Package tuning an imported EventAI line and inventing a summon placement beside it.
    fn creature_ai_artifact() -> String {
        format!(
            r#"{{"version":1,"package":"example.voice","source_hash":"{HASH_A}","claims":[{{"table":"game_creature_ai_broadcast_text","key":{{"id":900}},"operation":"update","fields":{{"male_text":{{"type":"string","value":"You will burn."}}}}}},{{"table":"game_creature_ai_summon","key":{{"id":17000001}},"operation":"insert","fields":{{"x":{{"type":"f32","value":1.5}},"y":{{"type":"f32","value":2.5}},"z":{{"type":"f32","value":3.5}},"orientation":{{"type":"f32","value":0.0}},"lifetime_ms":{{"type":"u32","value":30000}}}}}}]}}"#
        )
    }

    fn mixed_family_artifact(package: &str) -> String {
        format!(
            r#"{{"version":1,"package":"{package}","source_hash":"{HASH_A}","claims":[{{"table":"game_spell","key":{{"spell_id":{REAL_SPELL}}},"operation":"update","fields":{{"cooldown_ms":{{"type":"u32","value":1500}}}}}},{{"table":"game_item_template","key":{{"entry":25}},"operation":"update","fields":{{"name":{{"type":"string","value":"Worn Shortsword"}}}}}}]}}"#
        )
    }

    #[test]
    fn artifacts_are_discovered_inside_each_enabled_package() {
        let t = Scratch::new("discover");
        t.write(
            "zeta/data/.generated/spell.json",
            &update_artifact("example.zeta", REAL_SPELL, "cooldown_ms", 1500),
        );
        t.write(
            "alpha/data/.generated/spell.json",
            &update_artifact("example.alpha", REAL_SPELL, "gcd_ms", 1000),
        );

        let found = read_enabled(&t.0).expect("discovery succeeds");

        let packages: Vec<String> = found
            .iter()
            .map(|a| a.delta.package().to_string())
            .collect();
        assert_eq!(packages, ["example.alpha", "example.zeta"]);
    }

    #[test]
    fn one_family_apply_keeps_only_that_familys_claims() {
        let t = Scratch::new("family-filter");
        t.write(
            "mixed/data/.generated/catalogues.json",
            &mixed_family_artifact("example.mixed"),
        );
        let found = read_enabled(&t.0).expect("discovery succeeds");

        let spell = artifacts_for_family(found, "spell").expect("the spell plan filters");

        assert_eq!(spell.len(), 1);
        assert_eq!(spell[0].delta.claims().len(), 1);
        assert_eq!(spell[0].delta.claims()[0].table().family(), "spell");

        let found = read_enabled(&t.0).expect("discovery succeeds");
        let items = artifacts_for_family(found, "items").expect("the items plan filters");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].delta.claims().len(), 1);
        assert_eq!(items[0].delta.claims()[0].table().family(), "items");

        let found = read_enabled(&t.0).expect("discovery succeeds");
        assert!(artifacts_for_family(found, "quests")
            .expect("an absent family is an empty plan")
            .is_empty());
    }

    /// A Package with no data half is the common case, not a problem.
    #[test]
    fn a_package_with_no_generated_artifacts_contributes_nothing() {
        let t = Scratch::new("rust-only");
        t.write("rust-only/src/mod.rs", "");
        t.write("rust-only/data/notes.md", "not an artifact");

        let found = read_enabled(&t.0).expect("discovery succeeds");

        assert!(found.is_empty());
    }

    /// A Package ships every artifact kind it has into one directory. This stage is the CLAIM half
    /// of an apply, so a Script Artifact next to a Package Delta is not its business — and must not
    /// abort the whole spell import the way an unreadable Package Delta would.
    #[test]
    fn a_script_artifact_beside_a_package_delta_is_skipped_rather_than_refused() {
        let t = Scratch::new("mixed-kinds");
        t.write(
            "bolt/data/.generated/script.json",
            &format!(
                r#"{{"kind":"script","version":1,"package":"example.bolt","source_hash":"{HASH_A}","scripts":[{{"script_id":100001,"name":"bolt.greet","event":"on_login","priority":0,"enabled":true,"source":"grant_xp(event.actor, 1)"}}]}}"#
            ),
        );
        t.write(
            "bolt/data/.generated/spell.json",
            &update_artifact("example.bolt", REAL_SPELL, "cooldown_ms", 1500),
        );

        let found = read_enabled(&t.0).expect("discovery succeeds");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].delta.package().as_str(), "example.bolt");
    }

    /// The router only skips what it can positively identify as another kind. A file no parser can
    /// read is still a refusal that names it.
    #[test]
    fn a_file_no_parser_can_read_still_refuses_the_stage() {
        let t = Scratch::new("unreadable");
        t.write("broken/data/.generated/spell.json", "{ not even valid }");

        let refusal = read_enabled(&t.0).expect_err("discovery is refused");

        assert!(refusal.to_string().contains("spell.json"), "{refusal}");
    }

    /// Only `.generated/` counts. An author's own JSON beside their Datascript is not an artifact.
    #[test]
    fn json_outside_the_generated_directory_is_not_an_artifact() {
        let t = Scratch::new("stray-json");
        t.write("bolt/data/hand-written.json", "{ not even valid }");
        t.write(
            "bolt/data/.generated/spell.json",
            &update_artifact("example.bolt", REAL_SPELL, "cooldown_ms", 1500),
        );

        let found = read_enabled(&t.0).expect("discovery succeeds");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].delta.package().as_str(), "example.bolt");
    }

    /// An empty Package Inventory and an absent one are different statements: the first says no
    /// Package claims this family, the second says the operator never named one.
    #[test]
    fn an_enabled_root_holding_no_packages_is_an_empty_plan() {
        let t = Scratch::new("empty-root");

        let found = read_enabled(&t.0).expect("discovery succeeds");

        assert!(found.is_empty());
    }

    #[test]
    fn a_root_that_is_not_a_directory_is_refused() {
        let t = Scratch::new("absent-root");

        let refusal = read_enabled(&t.0.join("nowhere")).expect_err("discovery is refused");

        assert!(refusal.to_string().contains("not a directory"), "{refusal}");
    }

    #[test]
    fn an_invalid_artifact_refuses_the_stage_and_names_its_file() {
        let t = Scratch::new("invalid");
        t.write("broken/data/.generated/spell.json", r#"{"version":9}"#);

        let refusal = read_enabled(&t.0).expect_err("discovery is refused");

        assert!(
            refusal.to_string().contains("spell.json"),
            "the refusal names the file: {refusal}"
        );
    }

    /// A creature spawn on a map the scope owns stays in the plan; the same Package's spawn on a
    /// map it does not own is routed to the Shard that does.
    #[test]
    fn a_spatial_claim_reaches_only_the_shard_whose_scope_owns_its_map() {
        let t = Scratch::new("routing");
        t.write("placer/data/.generated/creatures.json", &two_map_artifact());
        let found = read_enabled(&t.0).expect("discovery succeeds");
        let creatures = artifacts_for_family(found, "creatures").expect("the creatures plan");

        let eastern = WorldImportScope::canonical(WorldImportProfile::AllianceEastern)
            .expect("eastern profile");
        let (kept, routed_away) =
            routed_to_this_shard(&eastern, creatures).expect("routing succeeds");

        assert_eq!(routed_away, 1);
        assert_eq!(kept.len(), 1);
        let claims = kept[0].delta.claims();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].key().map_id(), Some(EASTERN_MAP));
    }

    /// A global catalogue row states no map, so every Shard loads it however its scope is drawn.
    #[test]
    fn a_global_catalogue_claim_reaches_every_shard() {
        let t = Scratch::new("routing-global");
        t.write(
            "placer/data/.generated/creatures.json",
            &creature_template_artifact(),
        );
        let found = read_enabled(&t.0).expect("discovery succeeds");
        let creatures = artifacts_for_family(found, "creatures").expect("the creatures plan");

        let instances =
            WorldImportScope::canonical(WorldImportProfile::Instances).expect("instances profile");
        let (kept, routed_away) =
            routed_to_this_shard(&instances, creatures).expect("routing succeeds");

        assert_eq!(routed_away, 0);
        assert_eq!(kept.len(), 1, "a template is not spatial");
    }

    /// The EventAI catalogue names no map, so its whole family is global: an `instances` Shard and
    /// an open-world Shard both load every claim. The base import writes these tables the same way,
    /// with global SQL and no map predicate.
    #[test]
    fn every_creature_ai_claim_reaches_every_shard() {
        let t = Scratch::new("routing-creature-ai");
        t.write(
            "voice/data/.generated/creature-ai.json",
            &creature_ai_artifact(),
        );
        let found = read_enabled(&t.0).expect("discovery succeeds");
        let creature_ai = artifacts_for_family(found, "creature-ai").expect("the creature-ai plan");
        assert_eq!(creature_ai[0].delta.claims().len(), 2);

        for profile in [
            WorldImportProfile::Instances,
            WorldImportProfile::AllianceEastern,
        ] {
            let scope = WorldImportScope::canonical(profile).expect("a canonical profile");
            let (kept, routed_away) = routed_to_this_shard(
                &scope,
                artifacts_for_family(
                    read_enabled(&t.0).expect("discovery succeeds"),
                    "creature-ai",
                )
                .expect("the creature-ai plan"),
            )
            .expect("routing succeeds");

            assert_eq!(routed_away, 0, "{profile:?}");
            assert_eq!(kept[0].delta.claims().len(), 2, "{profile:?}");
        }
    }

    /// A Package whose every claim routed away drops out of the plan, the way it does when it
    /// claims no table of the family at all.
    #[test]
    fn a_package_whose_claims_all_route_away_leaves_the_plan() {
        let t = Scratch::new("routing-empty");
        t.write("placer/data/.generated/creatures.json", &two_map_artifact());
        let found = read_enabled(&t.0).expect("discovery succeeds");
        let creatures = artifacts_for_family(found, "creatures").expect("the creatures plan");

        // The instances profile owns Deadmines alone, so neither open-world spawn belongs to it.
        let instances =
            WorldImportScope::canonical(WorldImportProfile::Instances).expect("instances profile");
        let (kept, routed_away) =
            routed_to_this_shard(&instances, creatures).expect("routing succeeds");

        assert_eq!(routed_away, 2);
        assert!(kept.is_empty());
    }

    /// A whole map in the scope owns every claim on it, with no bounded geometry to pass — the base
    /// import's own rule for a forced creature, applied to an authored spawn.
    #[test]
    fn a_whole_map_in_the_scope_owns_every_spawn_on_it() {
        let t = Scratch::new("routing-whole-map");
        t.write(
            "placer/data/.generated/creatures.json",
            &instance_spawn_artifact(),
        );
        let found = read_enabled(&t.0).expect("discovery succeeds");
        let creatures = artifacts_for_family(found, "creatures").expect("the creatures plan");

        let instances =
            WorldImportScope::canonical(WorldImportProfile::Instances).expect("instances profile");
        let (kept, routed_away) =
            routed_to_this_shard(&instances, creatures).expect("routing succeeds");

        assert_eq!(routed_away, 0);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn the_report_accounts_for_the_claims_it_routed_away() {
        let report = plan_report("packages", "alliance-eastern", 2, &[], &trace(&[]));

        assert!(
            report.contains("2 spatial claim(s) routed away"),
            "{report}"
        );
        assert!(report.contains("alliance-eastern"), "{report}");
    }

    #[test]
    fn the_payload_carries_one_canonical_artifact_per_line() {
        let deltas: Vec<PackageDelta> = ["example.alpha", "example.zeta"]
            .iter()
            .map(|p| {
                PackageDelta::parse(&update_artifact(p, REAL_SPELL, "cooldown_ms", 1500))
                    .expect("artifact parses")
            })
            .collect();

        let packed = pack(&deltas);

        let lines: Vec<&str> = packed.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], deltas[0].to_canonical_json());
        assert_eq!(lines[1], deltas[1].to_canonical_json());
    }

    #[test]
    fn packing_nothing_is_an_empty_payload() {
        assert_eq!(pack(&[]), "");
    }

    #[test]
    fn the_report_names_every_package_its_file_and_its_row_counts() {
        let t = Scratch::new("report");
        t.write(
            "bolt/data/.generated/spell.json",
            &update_artifact("example.bolt", REAL_SPELL, "cooldown_ms", 1500),
        );
        let artifacts = read_enabled(&t.0).expect("discovery succeeds");
        let deltas: Vec<PackageDelta> = artifacts.iter().map(|a| a.delta.clone()).collect();

        let report = plan_report("packages", "test", 0, &artifacts, &trace(&deltas));

        assert!(report.contains("example.bolt"), "{report}");
        assert!(report.contains("spell.json"), "{report}");
        assert!(report.contains("1 updated"), "{report}");
        assert!(
            report.contains("update game_spell {spell_id=133} — cooldown_ms=1500 [example.bolt]"),
            "{report}"
        );
    }

    #[test]
    fn the_report_names_every_conflict() {
        let t = Scratch::new("report-conflict");
        t.write(
            "first/data/.generated/spell.json",
            &update_artifact("example.first", REAL_SPELL, "cooldown_ms", 1500),
        );
        t.write(
            "second/data/.generated/spell.json",
            &update_artifact("example.second", REAL_SPELL, "cooldown_ms", 3000),
        );
        let artifacts = read_enabled(&t.0).expect("discovery succeeds");
        let deltas: Vec<PackageDelta> = artifacts.iter().map(|a| a.delta.clone()).collect();

        let report = plan_report("packages", "test", 0, &artifacts, &trace(&deltas));

        assert!(report.contains("CONFLICT"), "{report}");
        assert!(report.contains("example.first"), "{report}");
        assert!(report.contains("example.second"), "{report}");
    }

    /// The check has to be able to describe a Package that invents a spell, not only one that tunes
    /// an existing row.
    #[test]
    fn the_report_describes_an_inserted_package_spell() {
        let whole_row = concat!(
            r#"{"name":{"type":"string","value":"Kindled Bolt"},"power_type":{"type":"u8","value":0},"#,
            r#""cost":{"type":"u32","value":30},"cast_time_ms":{"type":"u32","value":2500},"#,
            r#""gcd_ms":{"type":"u32","value":1500},"cooldown_ms":{"type":"u32","value":0},"#,
            r#""range_yd":{"type":"u32","value":30},"duration_ms":{"type":"u32","value":0},"#,
            r#""school_mask":{"type":"u8","value":4},"dispel_type":{"type":"u8","value":1},"#,
            r#""mechanic":{"type":"u8","value":0},"max_stacks":{"type":"u8","value":1},"#,
            r#""aura_interrupt":{"type":"u16","value":0},"attributes":{"type":"u32","value":0},"#,
            r#""spell_level":{"type":"u8","value":10},"max_level":{"type":"u8","value":60},"#,
            r#""is_negative":{"type":"bool","value":true},"cast_flags":{"type":"u32","value":0},"#,
            r#""stances":{"type":"u8","value":0},"family_name":{"type":"u8","value":3},"#,
            r#""family_flags":{"type":"u64","value":"1"},"proc_flags":{"type":"u32","value":0},"#,
            r#""proc_chance":{"type":"u8","value":0},"proc_charges":{"type":"u8","value":0}}"#,
        );
        let artifact = format!(
            r#"{{"version":1,"package":"example.bolt","source_hash":"{HASH_A}","claims":[{{"table":"game_spell","key":{{"spell_id":{PACKAGE_SPELL}}},"operation":"insert","fields":{whole_row}}}]}}"#
        );
        let t = Scratch::new("report-insert");
        t.write("bolt/data/.generated/spell.json", &artifact);
        let artifacts = read_enabled(&t.0).expect("discovery succeeds");
        let deltas: Vec<PackageDelta> = artifacts.iter().map(|a| a.delta.clone()).collect();

        let report = plan_report("packages", "test", 0, &artifacts, &trace(&deltas));

        assert!(report.contains("1 spells"), "{report}");
        assert!(
            report.contains("insert game_spell {spell_id=6000001}"),
            "{report}"
        );
        assert!(report.contains(r#"name="Kindled Bolt""#), "{report}");
    }
}
