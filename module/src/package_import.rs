//! Applying Package Deltas to one Import Family's tables, and the provenance that records what was
//! applied.
//!
//! A base import replaces a whole Import Family, so a Package's row edits cannot be a one-shot edit
//! — the next reload would silently revert them. They are an artifact that replays as the last
//! stage of the family's import. This module is that stage's durable half: one operator-gated
//! reducer that takes the WHOLE enabled plan for one family and applies it in one transaction.
//!
//! Whole-plan, not per-Package, on purpose. Two Packages may claim different columns of one row, so
//! the merged picture is the only correct unit of work — and a plan that fails halfway would leave
//! the shard running a set of Packages nobody chose. `lyracore_package_delta::trace` produces the
//! merged picture and every disagreement; this module refuses on a disagreement before it writes.
//!
//! # Family dispatch
//!
//! What every family shares lives here: reading the plan, refusing a conflict, refusing an update
//! whose target row will not be there, the order the durable pass runs in, and the provenance
//! rewrite. What one family owns lives in its own module: the setters for its tables, where its
//! Package-invented rows live, and how to find one row of it. [`Family`] is the only place the two
//! meet, and its matches carry no wildcard, so a family that arrives without an implementation does
//! not compile.
//!
//! # Reconciliation
//!
//! A base reimport clears only real identifiers, so rows a Package INVENTED (its family's Package
//! identifier band) survive it untouched. Nothing else would ever remove them, which is why this
//! reducer clears the whole band before it applies: a Package that left the enabled set takes its
//! invented rows with it, with no bookkeeping to disagree with the shard.
//!
//! Rows a Package only TUNED are not restored here. Disabling a Package means replaying the base
//! import and then applying the remaining Packages — there is no pre-image to roll back to, which
//! is also why a Claim can never delete a row.

#[cfg(test)]
mod fixtures;
mod items;
mod spell;

use spacetimedb::{reducer, table, ReducerContext, Table, Timestamp};

use lyracore_package_delta::{
    trace, ClaimCounts, FieldValue, Operation, PackageDelta, TracedRow, ITEM_FAMILY, SPELL_FAMILY,
};

use crate::helpers::require_operator;
use crate::import_meta::game_import_meta;

/// Separates the artifacts inside one `apply_package_deltas` payload. A Package Delta's canonical
/// form escapes every control character, so no artifact can contain this byte and no artifact needs
/// quoting to travel next to another.
const ARTIFACT_SEPARATOR: char = '\n';

/// An Import Family this build can apply Package Deltas for.
///
/// One variant per family the Package Delta schema names tables for, so a family with no artifact
/// shape to read cannot be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// `game_spell` and `game_spell_effect`.
    Spell,
    /// `game_item_template`.
    Item,
}

impl Family {
    /// Every family this build applies, in the order a refusal lists them.
    const ALL: &'static [Self] = &[Self::Spell, Self::Item];

    /// The family name the importer stamps and the reducer takes.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Spell => SPELL_FAMILY,
            Self::Item => ITEM_FAMILY,
        }
    }

    /// Resolves the reducer's `family` argument.
    ///
    /// # Errors
    /// Names the family that was asked for and the ones this build carries, because the caller is
    /// an operator running an import stage for a family whose schema has not landed yet.
    fn parse(name: &str) -> Result<Self, String> {
        Self::ALL
            .iter()
            .copied()
            .find(|family| family.as_str() == name)
            .ok_or_else(|| {
                format!(
                    "import family `{name}` has no Package Delta schema; this build applies {}",
                    known_families()
                )
            })
    }

    /// What this shard holds for the row an `update` claim names.
    fn update_target(self, ctx: &ReducerContext, row: &TracedRow) -> UpdateTarget {
        match self {
            Self::Spell => spell::update_target(ctx, row),
            Self::Item => items::update_target(ctx, row),
        }
    }

    /// Removes every row this family's Packages invented, so a Package that left the enabled set
    /// takes its rows with it.
    fn clear_package_range(self, ctx: &ReducerContext) {
        match self {
            Self::Spell => spell::clear_package_range(ctx),
            Self::Item => items::clear_package_range(ctx),
        }
    }

    /// Writes one merged row into this family's tables.
    fn write_row(self, ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
        match self {
            Self::Spell => spell::write_row(ctx, row),
            Self::Item => items::write_row(ctx, row),
        }
    }
}

/// The whole family catalogue as a prose list — "`a`", "`a` or `b`", "`a`, `b` or `c`" — so a
/// refusal reads naturally whatever `Family::ALL`'s length is. Mirrors
/// `lyracore_package_delta::error`'s `known_tables`.
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

/// What a shard holds for the row an `update` claim names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdateTarget {
    /// The row is there, so the update lands on it.
    Present,
    /// The row is not in this shard at all.
    Absent,
    /// The row sits in this family's Package identifier band and no enabled Package inserts it.
    /// The band is cleared on every apply, so the row is gone by the time the write pass runs.
    Uninvented,
}

/// What one Package contributed to the last apply of one Import Family.
///
/// One row per (family, Package), keyed by both joined — SpacetimeDB keys on one column, and a
/// Package appears once per family it claims in. Rewritten wholesale on every apply, like
/// `game_import_meta`: it answers "what Packages is this shard running right now", not "what did it
/// ever run".
///
/// NOT public, for the same reason `game_import_meta` is not: nothing subscribes it, so it needs no
/// gateway binding. The importer writes it through the reducer below and an operator reads it with
/// `spacetime sql`.
#[table(accessor = game_package_import)]
pub struct PackageImport {
    /// `<family>/<package>`.
    #[primary_key]
    pub id: String,
    /// The Import Family this Package claimed in — the same name `game_import_meta.family` uses.
    pub family: String,
    /// The Package identity the artifact carried.
    pub package: String,
    /// BLAKE3 digest of the artifact's CANONICAL bytes, so two artifacts that say the same thing
    /// hash the same however they were written. Distinct from `source_hash`.
    pub artifact_hash: String,
    /// The digest of the Datascript source the artifact was generated from, carried verbatim from
    /// the artifact. Distinct from `artifact_hash`: this one identifies the SOURCE, that one
    /// identifies the generated artifact.
    pub source_hash: String,
    /// Rows this Package changed but does not own.
    pub updated_rows: u64,
    /// `game_spell` rows this Package invented.
    pub inserted_spells: u64,
    /// `game_spell_effect` rows this Package invented.
    pub inserted_effects: u64,
    /// The base import generation these claims sit on: `game_import_meta.source_sha` for this
    /// family, read at apply. Empty when the family has never been stamped on this shard (a seeded
    /// development shard), which is a fact worth recording rather than a reason to refuse.
    pub base_source_sha: String,
    pub applied_at: Timestamp,
    /// Rows this Package invented, across every table its family owns — the family-generic
    /// successor to `inserted_spells`/`inserted_effects` above. Those two keep their names (a
    /// durable column is not renamed for a later family); a non-spell family reports through this
    /// column alone and leaves the legacy pair at zero, and the spell writer fills this one too, as
    /// the sum of its own two. END-appended + `#[default(0u64)]` → additive auto-migration.
    #[default(0u64)]
    pub inserted_rows: u64,
}

/// Apply the complete set of enabled Package Deltas for one Import Family.
///
/// One reducer call is one transaction, so the whole plan lands or none of it does. An unknown
/// family, an invalid artifact, a Claim Conflict, or an update whose target row is absent fails the
/// call before any row is touched, leaving neither claimed rows nor provenance behind.
///
/// `packed`: the artifacts' canonical JSON, one per line. The caller sends the WHOLE enabled set,
/// including none at all — an empty payload is the honest statement "no Package claims this family
/// any more" and clears the family's Package identifier band accordingly. The importer never sends
/// an empty payload by accident; it refuses to run this stage at all unless the operator named the
/// enabled Package root.
#[reducer]
pub fn apply_package_deltas(
    ctx: &ReducerContext,
    family: String,
    packed: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    let family = Family::parse(&family)?;

    let plan = ApplyPlan::read(&packed)?;
    check_claims_belong_to(family, &plan.rows)?;
    check_update_targets(ctx, family, &plan)?;

    family.clear_package_range(ctx);
    for row in &plan.rows {
        family.write_row(ctx, row)?;
    }
    stamp_provenance(ctx, family, &plan);
    Ok(())
}

// ===========================================================================================
//  The plan — pure. Everything that can refuse the payload refuses here, before any write.
// ===========================================================================================

/// One Package's contribution to a plan, in the shape its provenance row records.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedPackage {
    package: String,
    artifact_hash: String,
    source_hash: String,
    counts: ClaimCounts,
}

/// The merged picture of every enabled Package's claims, and who contributed what.
#[derive(Debug, Clone, PartialEq)]
struct ApplyPlan {
    packages: Vec<PlannedPackage>,
    rows: Vec<TracedRow>,
}

impl ApplyPlan {
    /// Reads a payload into a plan, refusing anything that must not reach the tables.
    ///
    /// The refusals, in order: an artifact that does not parse, two artifacts naming the same
    /// Package, and any Claim Conflict between Packages. A conflict reports EVERY disagreement, not
    /// just the first — the operator fixing them wants the whole list in one pass.
    ///
    /// The payload says nothing about which family it is for; that is the caller's argument, and
    /// [`check_claims_belong_to`] holds the two together.
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
                counts: delta.claim_counts(),
            })
            .collect();

        Ok(Self {
            packages,
            rows: traced.rows().to_vec(),
        })
    }
}

/// Refuses a plan that claims a table this call's family does not own.
///
/// One apply reloads one family, so a claim on another family's table would be applied out of turn
/// and then reverted by that family's own import. Every table names its owner, so this is the whole
/// check.
fn check_claims_belong_to(family: Family, rows: &[TracedRow]) -> Result<(), String> {
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

// ===========================================================================================
//  Row building — pure, and shared by every family's setters below.
// ===========================================================================================

/// An insert must carry every claimable column of its table. The artifact parser already refuses a
/// partial insert, so this catches only a schema that moved under a stored artifact.
fn check_insert_is_whole(row: &TracedRow) -> Result<(), String> {
    let missing: Vec<&str> = row
        .table()
        .columns()
        .iter()
        .map(|column| column.name)
        .filter(|name| !row.fields().contains_key(*name))
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

// ===========================================================================================
//  The durable shell — the only part that touches a ReducerContext.
// ===========================================================================================

/// Refuses a plan whose updates name rows that will not be there.
///
/// Runs before the first write, so a plan that names a missing row changes nothing at all.
fn check_update_targets(
    ctx: &ReducerContext,
    family: Family,
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

/// Rewrites this family's provenance wholesale, so the table always describes the Packages the
/// shard is running now rather than every Package it ever ran.
fn stamp_provenance(ctx: &ReducerContext, family: Family, plan: &ApplyPlan) {
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

    for planned in &plan.packages {
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

// ===========================================================================================
//  Tests — the shared half. Row WRITING needs a live ReducerContext, which a native test has no
//  way to build (same limit `import_meta`'s tests note); the wire suite covers that rung.
// ===========================================================================================
#[cfg(test)]
mod tests {
    use super::fixtures::{
        artifact, effect_claim, item_claim, plan, spell_claim, HASH_A, PACKAGE_ITEM, PACKAGE_SPELL,
        REAL_SPELL, WHOLE_EFFECT_ROW, WHOLE_ITEM_ROW, WHOLE_SPELL_ROW,
    };
    use super::{check_claims_belong_to, ApplyPlan, Family, ARTIFACT_SEPARATOR};
    use lyracore_package_delta::PackageDelta;

    /// The payload format rests on this: a canonical artifact escapes every control character, so
    /// one line is one artifact and no quoting is needed to put two of them next to each other.
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

    /// The digest is over the CANONICAL bytes, so how an artifact was spelled cannot change it.
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

    /// Two Packages tuning different columns of one spell are the reason the plan is traced as a
    /// whole rather than applied Package by Package.
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
    fn an_import_family_with_no_delta_schema_is_refused_by_name() {
        let refusal = Family::parse("quests").expect_err("an unsupported family is refused");

        assert!(refusal.contains("`quests`"), "{refusal}");
        assert!(refusal.contains("applies `spell` or `items`"), "{refusal}");
    }

    #[test]
    fn the_spell_and_item_families_are_the_ones_this_build_applies() {
        assert_eq!(Family::parse("spell"), Ok(Family::Spell));
        assert_eq!(Family::Spell.as_str(), "spell");
        assert_eq!(Family::parse("items"), Ok(Family::Item));
        assert_eq!(Family::Item.as_str(), "items");
    }

    /// Every claimable table belongs to the spell family today, so a spell plan is in scope whole.
    #[test]
    fn a_spell_plan_claims_only_spell_family_tables() {
        let claims = [
            spell_claim(PACKAGE_SPELL, "insert", WHOLE_SPELL_ROW),
            effect_claim(PACKAGE_SPELL, 0, "insert", WHOLE_EFFECT_ROW),
        ]
        .join(",");
        let plan = plan(&[artifact("example.bolt", &claims)]).expect("plan builds");

        assert_eq!(check_claims_belong_to(Family::Spell, &plan.rows), Ok(()));
    }

    /// An items plan is checked against the items family, not the spell family it happens to sit
    /// beside in this build's catalogue.
    #[test]
    fn an_item_plan_claims_only_item_family_tables() {
        let plan = plan(&[artifact(
            "example.bolt",
            &item_claim(PACKAGE_ITEM, "insert", WHOLE_ITEM_ROW),
        )])
        .expect("plan builds");

        assert_eq!(check_claims_belong_to(Family::Item, &plan.rows), Ok(()));
        assert!(check_claims_belong_to(Family::Spell, &plan.rows).is_err());
    }
}
