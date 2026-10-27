mod casts;
mod creature_ai;
mod creatures;
#[cfg(test)]
mod fixtures;
mod gameobjects;
mod globals;
mod gossip;
mod items;
mod loot;
mod quest;
mod script;
mod spell;
mod spellmeta;
mod trainers;

use spacetimedb::{reducer, table, ReducerContext, Table, Timestamp};

use lyracore_package_delta::{
    trace, ClaimCounts, FieldValue, Operation, PackageDelta, TracedRow, CAST_FAMILY,
    CREATURE_AI_FAMILY, CREATURE_FAMILY, GAMEOBJECT_FAMILY, GLOBALS_FAMILY, GOSSIP_FAMILY,
    ITEM_FAMILY, LOOT_FAMILY, QUEST_FAMILY, SCRIPT_FAMILY, SPELLMETA_FAMILY, SPELL_FAMILY,
    TRAINER_FAMILY,
};

use crate::helpers::require_operator;
use crate::import_meta::game_import_meta;

// Canonical artifacts escape control characters, so this byte cannot occur inside one.
const ARTIFACT_SEPARATOR: char = '\n';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Claims(ClaimFamily),
    Script,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaimFamily {
    Spell,
    Item,
    Quest,
    Loot,
    Cast,
    Trainer,
    Gossip,
    Globals,
    Spellmeta,
    Creatures,
    Gameobjects,
    CreatureAi,
}

impl Family {
    const ALL: &'static [Self] = &[
        Self::Claims(ClaimFamily::Spell),
        Self::Claims(ClaimFamily::Item),
        Self::Claims(ClaimFamily::Quest),
        Self::Claims(ClaimFamily::Loot),
        Self::Claims(ClaimFamily::Cast),
        Self::Claims(ClaimFamily::Trainer),
        Self::Claims(ClaimFamily::Gossip),
        Self::Claims(ClaimFamily::Globals),
        Self::Claims(ClaimFamily::Spellmeta),
        Self::Claims(ClaimFamily::Creatures),
        Self::Claims(ClaimFamily::Gameobjects),
        Self::Claims(ClaimFamily::CreatureAi),
        Self::Script,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Claims(family) => family.as_str(),
            Self::Script => SCRIPT_FAMILY,
        }
    }

    fn parse(name: &str) -> Result<Self, String> {
        Self::ALL
            .iter()
            .copied()
            .find(|family| family.as_str() == name)
            .ok_or_else(|| {
                format!(
                    "import family `{name}` has no Package artifact schema; this build \
                     applies {}",
                    known_families()
                )
            })
    }
}

impl ClaimFamily {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Spell => SPELL_FAMILY,
            Self::Item => ITEM_FAMILY,
            Self::Quest => QUEST_FAMILY,
            Self::Loot => LOOT_FAMILY,
            Self::Cast => CAST_FAMILY,
            Self::Trainer => TRAINER_FAMILY,
            Self::Gossip => GOSSIP_FAMILY,
            Self::Globals => GLOBALS_FAMILY,
            Self::Spellmeta => SPELLMETA_FAMILY,
            Self::Creatures => CREATURE_FAMILY,
            Self::Gameobjects => GAMEOBJECT_FAMILY,
            Self::CreatureAi => CREATURE_AI_FAMILY,
        }
    }

    fn update_target(self, ctx: &ReducerContext, row: &TracedRow) -> UpdateTarget {
        match self {
            Self::Spell => spell::update_target(ctx, row),
            Self::Item => items::update_target(ctx, row),
            Self::Quest => quest::update_target(ctx, row),
            Self::Loot => loot::update_target(ctx, row),
            Self::Cast => casts::update_target(ctx, row),
            Self::Trainer => trainers::update_target(ctx, row),
            Self::Gossip => gossip::update_target(ctx, row),
            Self::Globals => globals::update_target(ctx, row),
            Self::Spellmeta => spellmeta::update_target(ctx, row),
            Self::Creatures => creatures::update_target(ctx, row),
            Self::Gameobjects => gameobjects::update_target(ctx, row),
            Self::CreatureAi => creature_ai::update_target(ctx, row),
        }
    }

    fn clear_package_range(self, ctx: &ReducerContext) {
        match self {
            Self::Spell => spell::clear_package_range(ctx),
            Self::Item => items::clear_package_range(ctx),
            Self::Quest => quest::clear_package_range(ctx),
            Self::Loot => loot::clear_package_range(ctx),
            Self::Cast => casts::clear_package_range(ctx),
            Self::Trainer => trainers::clear_package_range(ctx),
            Self::Gossip => gossip::clear_package_range(ctx),
            Self::Globals => globals::clear_package_range(ctx),
            Self::Spellmeta => spellmeta::clear_package_range(ctx),
            Self::Creatures => creatures::clear_package_range(ctx),
            Self::Gameobjects => gameobjects::clear_package_range(ctx),
            Self::CreatureAi => creature_ai::clear_package_range(ctx),
        }
    }

    fn write_row(self, ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
        match self {
            Self::Spell => spell::write_row(ctx, row),
            Self::Item => items::write_row(ctx, row),
            Self::Quest => quest::write_row(ctx, row),
            Self::Loot => loot::write_row(ctx, row),
            Self::Cast => casts::write_row(ctx, row),
            Self::Trainer => trainers::write_row(ctx, row),
            Self::Gossip => gossip::write_row(ctx, row),
            Self::Globals => globals::write_row(ctx, row),
            Self::Spellmeta => spellmeta::write_row(ctx, row),
            Self::Creatures => creatures::write_row(ctx, row),
            Self::Gameobjects => gameobjects::write_row(ctx, row),
            Self::CreatureAi => creature_ai::write_row(ctx, row),
        }
    }

    fn check_references(self, ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
        match self {
            Self::Spell | Self::Item => Ok(()),
            Self::Quest => quest::check_references(ctx, rows),
            Self::Loot => loot::check_references(ctx, rows),
            Self::Cast => casts::check_references(ctx, rows),
            Self::Trainer => trainers::check_references(ctx, rows),
            Self::Gossip => gossip::check_references(ctx, rows),
            Self::Globals => globals::check_references(ctx, rows),
            Self::Spellmeta => spellmeta::check_references(ctx, rows),
            Self::Creatures => creatures::check_references(ctx, rows),
            Self::Gameobjects => gameobjects::check_references(ctx, rows),
            Self::CreatureAi => creature_ai::check_references(ctx, rows),
        }
    }
}

fn known_families() -> String {
    let quoted: Vec<String> = Family::ALL
        .iter()
        .map(|family| format!("`{}`", family.as_str()))
        .collect();
    match quoted.split_last() {
        None => "no import family in this build".to_owned(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdateTarget {
    Present,
    Absent,
    Uninvented,
}

#[table(accessor = game_package_import)]
pub struct PackageImport {
    #[primary_key]
    pub id: String,
    pub family: String,
    pub package: String,
    pub artifact_hash: String,
    pub source_hash: String,
    pub updated_rows: u64,
    pub inserted_spells: u64,
    pub inserted_effects: u64,
    pub base_source_sha: String,
    pub applied_at: Timestamp,
    #[default(0u64)]
    pub inserted_rows: u64,
}

#[reducer]
pub fn apply_package_deltas(
    ctx: &ReducerContext,
    family: String,
    packed: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    let family = Family::parse(&family)?;

    let packages = match family {
        Family::Claims(claims) => apply_claims(ctx, claims, &packed)?,
        Family::Script => script::apply(ctx, &packed)?,
    };

    stamp_provenance(ctx, family, &packages);
    Ok(())
}

fn apply_claims(
    ctx: &ReducerContext,
    family: ClaimFamily,
    packed: &str,
) -> Result<Vec<PlannedPackage>, String> {
    let plan = ApplyPlan::read(packed)?;
    check_claims_belong_to(family, &plan.rows)?;
    check_update_targets(ctx, family, &plan)?;
    family.check_references(ctx, &plan.rows)?;

    family.clear_package_range(ctx);
    for row in &plan.rows {
        family.write_row(ctx, row)?;
    }
    if family == ClaimFamily::Gameobjects {
        // Reconcile only after every spawn and template edit has finished.
        crate::go_collider::rebuild_go_colliders(ctx)?;
    }
    Ok(plan.packages)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedPackage {
    package: String,
    artifact_hash: String,
    source_hash: String,
    counts: RowCounts,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RowCounts {
    updated_rows: u64,
    inserted_rows: u64,
    inserted_spells: u64,
    inserted_effects: u64,
}

impl From<ClaimCounts> for RowCounts {
    fn from(counts: ClaimCounts) -> Self {
        Self {
            updated_rows: counts.updated_rows,
            inserted_rows: counts.inserted_rows,
            inserted_spells: counts.inserted_spells,
            inserted_effects: counts.inserted_effects,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ApplyPlan {
    packages: Vec<PlannedPackage>,
    rows: Vec<TracedRow>,
}

impl ApplyPlan {
    fn read(packed: &str) -> Result<Self, String> {
        let mut deltas: Vec<PackageDelta> = Vec::new();
        for (index, artifact) in packed
            .split(ARTIFACT_SEPARATOR)
            .filter(|line| !line.is_empty())
            .enumerate()
        {
            let delta = PackageDelta::parse(artifact)
                .map_err(|e| format!("Package Delta {index} in this plan is invalid: {e}"))?;
            if let Some(seen) = deltas.iter().find(|d| d.package() == delta.package()) {
                return Err(format!(
                    "package `{}` appears twice in this plan",
                    seen.package()
                ));
            }
            deltas.push(delta);
        }

        let traced = trace(&deltas);
        if !traced.is_clear() {
            let report = traced
                .conflicts()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            return Err(format!(
                "{} claim conflicts, nothing applied: {report}",
                traced.conflicts().len()
            ));
        }

        let packages = deltas
            .iter()
            .map(|delta| PlannedPackage {
                package: delta.package().to_string(),
                artifact_hash: blake3::hash(delta.to_canonical_json().as_bytes())
                    .to_hex()
                    .to_string(),
                source_hash: delta.source_hash().to_string(),
                counts: delta.claim_counts().into(),
            })
            .collect();

        Ok(Self {
            packages,
            rows: traced.rows().to_vec(),
        })
    }
}

fn check_claims_belong_to(family: ClaimFamily, rows: &[TracedRow]) -> Result<(), String> {
    for row in rows {
        if row.table().family() != family.as_str() {
            return Err(format!(
                "`{}` row {} belongs to the `{}` Import Family, not `{}`",
                row.table(),
                row.key(),
                row.table().family(),
                family.as_str()
            ));
        }
    }
    Ok(())
}

fn check_insert_is_whole(row: &TracedRow) -> Result<(), String> {
    let missing: Vec<&str> = row
        .table()
        .columns()
        .iter()
        .map(|column| column.name)
        .filter(|name| !row.fields().contains_key(*name) && !row.table().column_has_default(name))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "`{}` row {} is inserted without {}",
            row.table(),
            row.key(),
            missing.join(", ")
        ))
    }
}

fn as_u8(field: &str, value: &FieldValue) -> Result<u8, String> {
    match value {
        FieldValue::U8(n) => Ok(*n),
        other => Err(wrong_type(field, other)),
    }
}

fn as_u16(field: &str, value: &FieldValue) -> Result<u16, String> {
    match value {
        FieldValue::U16(n) => Ok(*n),
        other => Err(wrong_type(field, other)),
    }
}

fn as_u32(field: &str, value: &FieldValue) -> Result<u32, String> {
    match value {
        FieldValue::U32(n) => Ok(*n),
        other => Err(wrong_type(field, other)),
    }
}

fn as_u64(field: &str, value: &FieldValue) -> Result<u64, String> {
    match value {
        FieldValue::U64(n) => Ok(*n),
        other => Err(wrong_type(field, other)),
    }
}

fn as_i32(field: &str, value: &FieldValue) -> Result<i32, String> {
    match value {
        FieldValue::I32(n) => Ok(*n),
        other => Err(wrong_type(field, other)),
    }
}

fn as_f32(field: &str, value: &FieldValue) -> Result<f32, String> {
    match value {
        FieldValue::F32(n) => Ok(*n),
        other => Err(wrong_type(field, other)),
    }
}

fn as_bool(field: &str, value: &FieldValue) -> Result<bool, String> {
    match value {
        FieldValue::Bool(b) => Ok(*b),
        other => Err(wrong_type(field, other)),
    }
}

fn as_str(field: &str, value: &FieldValue) -> Result<String, String> {
    match value {
        FieldValue::Str(s) => Ok(s.clone()),
        other => Err(wrong_type(field, other)),
    }
}

fn wrong_type(field: &str, value: &FieldValue) -> String {
    format!(
        "column `{field}` was claimed as `{}`, which is not its type",
        value.field_type()
    )
}

// Resolve every missing-row Refusal before the first write.
fn check_update_targets(
    ctx: &ReducerContext,
    family: ClaimFamily,
    plan: &ApplyPlan,
) -> Result<(), String> {
    for row in &plan.rows {
        if row.operation() != Operation::Update {
            continue;
        }
        match family.update_target(ctx, row) {
            UpdateTarget::Present => {}
            UpdateTarget::Uninvented => {
                return Err(format!(
                    "`{}` row {}: an enabled Package tunes it, but no enabled Package inserts it",
                    row.table(),
                    row.key()
                ))
            }
            UpdateTarget::Absent => {
                return Err(format!(
                    "`{}` row {} is not in this shard; the base import has to run before its \
                     Package Deltas",
                    row.table(),
                    row.key()
                ))
            }
        }
    }
    Ok(())
}

fn stamp_provenance(ctx: &ReducerContext, family: Family, packages: &[PlannedPackage]) {
    let family = family.as_str();
    let base_source_sha = ctx
        .db
        .game_import_meta()
        .family()
        .find(family.to_string())
        .map_or_else(String::new, |meta| meta.source_sha);

    let imports = ctx.db.game_package_import();
    let stale: Vec<String> = imports
        .iter()
        .filter(|row| row.family == family)
        .map(|row| row.id)
        .collect();
    for id in stale {
        imports.id().delete(id);
    }

    for planned in packages {
        imports.insert(PackageImport {
            id: format!("{family}/{}", planned.package),
            family: family.to_string(),
            package: planned.package.clone(),
            artifact_hash: planned.artifact_hash.clone(),
            source_hash: planned.source_hash.clone(),
            updated_rows: planned.counts.updated_rows,
            inserted_spells: planned.counts.inserted_spells,
            inserted_effects: planned.counts.inserted_effects,
            base_source_sha: base_source_sha.clone(),
            applied_at: ctx.timestamp,
            inserted_rows: planned.counts.inserted_rows,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{
        artifact, creature_spell_claim, effect_claim, graveyard_zone_claim, item_claim,
        npc_text_claim, pickpocket_loot_claim, plan, quest_claim, spell_claim, spell_learn_claim,
        trainer_spell_claim, HASH_A, PACKAGE_CREATURE_SPELL, PACKAGE_GLOBALS, PACKAGE_GOSSIP,
        PACKAGE_ITEM, PACKAGE_LOOT, PACKAGE_QUEST, PACKAGE_SPELL, PACKAGE_SPELL_LEARN,
        PACKAGE_TRAINER_SPELL, REAL_SPELL, WHOLE_CREATURE_SPELL_ROW, WHOLE_EFFECT_ROW,
        WHOLE_GRAVEYARD_ZONE_ROW, WHOLE_ITEM_ROW, WHOLE_NPC_TEXT_ROW, WHOLE_PICKPOCKET_LOOT_ROW,
        WHOLE_QUEST_ROW, WHOLE_SPELL_LEARN_ROW, WHOLE_SPELL_ROW, WHOLE_TRAINER_SPELL_ROW,
    };
    use super::{check_claims_belong_to, ApplyPlan, ClaimFamily, Family, ARTIFACT_SEPARATOR};
    use lyracore_package_delta::PackageDelta;

    #[test]
    fn a_canonical_artifact_never_contains_the_artifact_separator() {
        let awkward = spell_claim(
            PACKAGE_SPELL,
            "insert",
            &WHOLE_SPELL_ROW.replace("Kindled Bolt", r#"Kindled\nBolt\t\"quoted\""#),
        );

        let delta =
            PackageDelta::parse(&artifact("example.bolt", &awkward)).expect("artifact parses");

        assert!(!delta.to_canonical_json().contains(ARTIFACT_SEPARATOR));
    }

    #[test]
    fn an_empty_payload_is_a_plan_that_claims_nothing() {
        let plan = ApplyPlan::read("").expect("an empty plan is valid");

        assert!(plan.packages.is_empty());
        assert!(plan.rows.is_empty());
    }

    #[test]
    fn a_plan_records_each_packages_hashes_and_row_counts() {
        let claims = [
            spell_claim(PACKAGE_SPELL, "insert", WHOLE_SPELL_ROW),
            effect_claim(PACKAGE_SPELL, 0, "insert", WHOLE_EFFECT_ROW),
            spell_claim(
                REAL_SPELL,
                "update",
                r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
            ),
        ]
        .join(",");

        let plan = plan(&[artifact("example.bolt", &claims)]).expect("plan builds");

        assert_eq!(plan.packages.len(), 1);
        let package = &plan.packages[0];
        assert_eq!(package.package, "example.bolt");
        assert_eq!(package.source_hash, HASH_A);
        assert_eq!(package.counts.inserted_spells, 1);
        assert_eq!(package.counts.inserted_effects, 1);
        assert_eq!(package.counts.updated_rows, 1);
        assert_ne!(
            package.artifact_hash, package.source_hash,
            "the artifact digest and the source digest are different facts"
        );
    }

    #[test]
    fn the_artifact_hash_ignores_how_the_artifact_was_written() {
        let claims = spell_claim(
            REAL_SPELL,
            "update",
            r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
        );
        let compact = artifact("example.bolt", &claims);
        let spaced = compact.replace(':', " : ");

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
            &spell_claim(
                REAL_SPELL,
                "update",
                r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
            ),
        );

        let refusal = plan(&[good, "{\"version\":9}".to_owned()]).expect_err("plan is refused");

        assert!(refusal.contains("Package Delta 1"), "{refusal}");
    }

    #[test]
    fn two_packages_claiming_one_column_refuse_the_whole_plan() {
        let first = artifact(
            "example.first",
            &spell_claim(
                REAL_SPELL,
                "update",
                r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
            ),
        );
        let second = artifact(
            "example.second",
            &spell_claim(
                REAL_SPELL,
                "update",
                r#"{"cooldown_ms":{"type":"u32","value":3000}}"#,
            ),
        );

        let refusal = plan(&[first, second]).expect_err("plan is refused");

        assert!(refusal.contains("claim conflicts"), "{refusal}");
        assert!(refusal.contains("example.first"), "{refusal}");
        assert!(refusal.contains("example.second"), "{refusal}");
    }

    #[test]
    fn one_package_sent_twice_refuses_the_whole_plan() {
        let once = artifact(
            "example.bolt",
            &spell_claim(
                REAL_SPELL,
                "update",
                r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
            ),
        );

        let refusal = plan(&[once.clone(), once]).expect_err("plan is refused");

        assert!(refusal.contains("appears twice"), "{refusal}");
    }

    #[test]
    fn two_packages_tuning_different_columns_merge_into_one_row() {
        let first = artifact(
            "example.first",
            &spell_claim(
                REAL_SPELL,
                "update",
                r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
            ),
        );
        let second = artifact(
            "example.second",
            &spell_claim(
                REAL_SPELL,
                "update",
                r#"{"gcd_ms":{"type":"u32","value":1000}}"#,
            ),
        );

        let plan = plan(&[first, second]).expect("plan builds");

        assert_eq!(plan.rows.len(), 1);
        assert_eq!(plan.rows[0].fields().len(), 2);
    }

    #[test]
    fn an_import_family_with_no_artifact_schema_is_refused_by_name() {
        let refusal = Family::parse("terrain").expect_err("an unsupported family is refused");

        assert!(refusal.contains("`terrain`"), "{refusal}");
        assert!(refusal.contains("`spell`"), "{refusal}");
        assert!(refusal.contains("`items`"), "{refusal}");
        assert!(refusal.contains("`quests`"), "{refusal}");
        assert!(refusal.contains("`loot`"), "{refusal}");
        assert!(refusal.contains("`casts`"), "{refusal}");
        assert!(refusal.contains("`trainers`"), "{refusal}");
        assert!(refusal.contains("`gossip`"), "{refusal}");
        assert!(refusal.contains("`globals`"), "{refusal}");
        assert!(refusal.contains("`spellmeta`"), "{refusal}");
        assert!(refusal.contains("`creatures`"), "{refusal}");
        assert!(refusal.contains("`gameobjects`"), "{refusal}");
        assert!(refusal.contains("`creature-ai`"), "{refusal}");
        assert!(refusal.contains("`script`"), "{refusal}");
    }

    #[test]
    fn every_family_this_build_applies_resolves_from_its_name() {
        assert_eq!(
            Family::parse("spell"),
            Ok(Family::Claims(ClaimFamily::Spell))
        );
        assert_eq!(
            Family::parse("items"),
            Ok(Family::Claims(ClaimFamily::Item))
        );
        assert_eq!(
            Family::parse("quests"),
            Ok(Family::Claims(ClaimFamily::Quest))
        );
        assert_eq!(Family::parse("loot"), Ok(Family::Claims(ClaimFamily::Loot)));
        assert_eq!(
            Family::parse("casts"),
            Ok(Family::Claims(ClaimFamily::Cast))
        );
        assert_eq!(
            Family::parse("trainers"),
            Ok(Family::Claims(ClaimFamily::Trainer))
        );
        assert_eq!(
            Family::parse("gossip"),
            Ok(Family::Claims(ClaimFamily::Gossip))
        );
        assert_eq!(
            Family::parse("globals"),
            Ok(Family::Claims(ClaimFamily::Globals))
        );
        assert_eq!(
            Family::parse("spellmeta"),
            Ok(Family::Claims(ClaimFamily::Spellmeta))
        );
        assert_eq!(Family::parse("script"), Ok(Family::Script));
        for family in Family::ALL {
            assert_eq!(Family::parse(family.as_str()), Ok(*family));
        }
    }

    #[test]
    fn no_two_families_answer_to_one_name() {
        let mut names: Vec<&str> = Family::ALL.iter().map(|f| f.as_str()).collect();
        names.sort_unstable();
        let unique = names.len();
        names.dedup();
        assert_eq!(names.len(), unique);
    }

    #[test]
    fn a_spell_plan_claims_only_spell_family_tables() {
        let claims = [
            spell_claim(PACKAGE_SPELL, "insert", WHOLE_SPELL_ROW),
            effect_claim(PACKAGE_SPELL, 0, "insert", WHOLE_EFFECT_ROW),
        ]
        .join(",");
        let plan = plan(&[artifact("example.bolt", &claims)]).expect("plan builds");

        assert_eq!(
            check_claims_belong_to(ClaimFamily::Spell, &plan.rows),
            Ok(())
        );
    }

    #[test]
    fn an_item_plan_claims_only_item_family_tables() {
        let plan = plan(&[artifact(
            "example.bolt",
            &item_claim(PACKAGE_ITEM, "insert", WHOLE_ITEM_ROW),
        )])
        .expect("plan builds");

        assert_eq!(
            check_claims_belong_to(ClaimFamily::Item, &plan.rows),
            Ok(())
        );
        assert!(check_claims_belong_to(ClaimFamily::Spell, &plan.rows).is_err());
    }

    #[test]
    fn a_quest_plan_claims_only_quest_family_tables() {
        let plan = plan(&[artifact(
            "example.bolt",
            &quest_claim(PACKAGE_QUEST, "insert", WHOLE_QUEST_ROW),
        )])
        .expect("plan builds");

        assert_eq!(
            check_claims_belong_to(ClaimFamily::Quest, &plan.rows),
            Ok(())
        );
        assert!(check_claims_belong_to(ClaimFamily::Item, &plan.rows).is_err());
    }

    #[test]
    fn a_loot_plan_claims_only_loot_family_tables() {
        let plan = plan(&[artifact(
            "example.bolt",
            &pickpocket_loot_claim(PACKAGE_LOOT, "insert", WHOLE_PICKPOCKET_LOOT_ROW),
        )])
        .expect("plan builds");

        assert_eq!(
            check_claims_belong_to(ClaimFamily::Loot, &plan.rows),
            Ok(())
        );
        assert!(check_claims_belong_to(ClaimFamily::Item, &plan.rows).is_err());
    }

    #[test]
    fn a_cast_plan_claims_only_cast_family_tables() {
        let plan = plan(&[artifact(
            "example.bolt",
            &creature_spell_claim(PACKAGE_CREATURE_SPELL, "insert", WHOLE_CREATURE_SPELL_ROW),
        )])
        .expect("plan builds");

        assert_eq!(
            check_claims_belong_to(ClaimFamily::Cast, &plan.rows),
            Ok(())
        );
        assert!(check_claims_belong_to(ClaimFamily::Loot, &plan.rows).is_err());
    }

    #[test]
    fn a_trainer_plan_claims_only_trainer_family_tables() {
        let plan = plan(&[artifact(
            "example.bolt",
            &trainer_spell_claim(PACKAGE_TRAINER_SPELL, "insert", WHOLE_TRAINER_SPELL_ROW),
        )])
        .expect("plan builds");

        assert_eq!(
            check_claims_belong_to(ClaimFamily::Trainer, &plan.rows),
            Ok(())
        );
        assert!(check_claims_belong_to(ClaimFamily::Cast, &plan.rows).is_err());
    }

    #[test]
    fn a_gossip_plan_claims_only_gossip_family_tables() {
        let plan = plan(&[artifact(
            "example.bolt",
            &npc_text_claim(PACKAGE_GOSSIP as u32, "insert", WHOLE_NPC_TEXT_ROW),
        )])
        .expect("plan builds");

        assert_eq!(
            check_claims_belong_to(ClaimFamily::Gossip, &plan.rows),
            Ok(())
        );
        assert!(check_claims_belong_to(ClaimFamily::Globals, &plan.rows).is_err());
    }

    #[test]
    fn a_globals_plan_claims_only_globals_family_tables() {
        let plan = plan(&[artifact(
            "example.bolt",
            &graveyard_zone_claim(PACKAGE_GLOBALS, "insert", WHOLE_GRAVEYARD_ZONE_ROW),
        )])
        .expect("plan builds");

        assert_eq!(
            check_claims_belong_to(ClaimFamily::Globals, &plan.rows),
            Ok(())
        );
        assert!(check_claims_belong_to(ClaimFamily::Gossip, &plan.rows).is_err());
    }

    #[test]
    fn a_spell_metadata_plan_claims_only_spellmeta_family_tables() {
        let plan = plan(&[artifact(
            "example.bolt",
            &spell_learn_claim(PACKAGE_SPELL_LEARN, "insert", WHOLE_SPELL_LEARN_ROW),
        )])
        .expect("plan builds");

        assert_eq!(
            check_claims_belong_to(ClaimFamily::Spellmeta, &plan.rows),
            Ok(())
        );
        assert!(check_claims_belong_to(ClaimFamily::Spell, &plan.rows).is_err());
    }
}
