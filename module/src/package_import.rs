//! Applying Package Deltas to the spell tables, and the provenance that records what was applied.
//!
//! A base import replaces a whole Import Family, so a Package's row edits cannot be a one-shot edit
//! — the next reload would silently revert them. They are an artifact that replays as the last
//! stage of the family's import. This module is that stage's durable half: one operator-gated
//! reducer that takes the WHOLE enabled plan and applies it in one transaction.
//!
//! Whole-plan, not per-Package, on purpose. Two Packages may claim different columns of one row, so
//! the merged picture is the only correct unit of work — and a plan that fails halfway would leave
//! the shard running a set of Packages nobody chose. `lyracore_package_delta::trace` produces the
//! merged picture and every disagreement; this module refuses on a disagreement before it writes.
//!
//! # Reconciliation
//!
//! A base reimport clears only real spell identifiers, so rows a Package INVENTED (the Package
//! spell range) survive it untouched. Nothing else would ever remove them, which is why this
//! reducer clears the whole Package spell range before it applies: a Package that left the enabled
//! set takes its invented rows with it, with no bookkeeping to disagree with the shard.
//!
//! Rows a Package only TUNED are not restored here. Disabling a Package means replaying the base
//! import and then applying the remaining Packages — there is no pre-image to roll back to, which
//! is also why a Claim can never delete a row.

use spacetimedb::{reducer, table, ReducerContext, Table, Timestamp};

use lyracore_package_delta::{
    is_package_spell_id, trace, ClaimCounts, FieldValue, Operation, PackageDelta, PrimaryKey,
    Table as ClaimTable, TracedRow,
};

use crate::helpers::require_operator;
use crate::import_meta::game_import_meta;
use crate::spell::{game_spell, game_spell_effect, Spell, SpellEffect};

/// The one Import Family whose Package Deltas this build applies. The Package Delta schema names
/// `game_spell` and `game_spell_effect` and nothing else, so any other family has no artifact shape
/// to read yet.
const SPELL_FAMILY: &str = "spell";

/// Separates the artifacts inside one `apply_package_deltas` payload. A Package Delta's canonical
/// form escapes every control character, so no artifact can contain this byte and no artifact needs
/// quoting to travel next to another.
const ARTIFACT_SEPARATOR: char = '\n';

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
}

/// Apply the complete set of enabled Package Deltas for one Import Family.
///
/// One reducer call is one transaction, so the whole plan lands or none of it does. An invalid
/// artifact, a Claim Conflict, or an update whose target row is absent fails the call before any
/// row is touched, leaving neither spell rows nor provenance behind.
///
/// `packed`: the artifacts' canonical JSON, one per line. The caller sends the WHOLE enabled set,
/// including none at all — an empty payload is the honest statement "no Package claims this family
/// any more" and clears the Package spell range accordingly. The importer never sends an empty
/// payload by accident; it refuses to run this stage at all unless the operator named the enabled
/// Package root.
#[reducer]
pub fn apply_package_deltas(
    ctx: &ReducerContext,
    family: String,
    packed: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    if family != SPELL_FAMILY {
        return Err(format!(
            "import family `{family}` has no Package Delta schema; this build applies \
             `{SPELL_FAMILY}` only"
        ));
    }

    let plan = ApplyPlan::read(&packed)?;
    check_update_targets(ctx, &plan)?;

    clear_package_spell_range(ctx);
    for row in &plan.rows {
        write_row(ctx, row)?;
    }
    stamp_provenance(ctx, &family, &plan);
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

/// True when a traced update would land on a Package-range row that no enabled Package invents.
///
/// The Package spell range is cleared on every apply, so such a row is gone by the time the write
/// pass runs: the Package that owns it is not enabled, and the one tuning it is claiming a row that
/// does not exist. A base spell is different — the base import puts it there.
fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    row.operation() == Operation::Update && is_package_spell_id(row.key().spell_id())
}

// ===========================================================================================
//  Row building — pure. The Claim schema and these setters are one contract; the tests below
//  fail if a claimable column has no setter here.
// ===========================================================================================

/// A Package spell before any claim has been applied to it. Every claimable column is overwritten
/// by the insert that follows, so these values never survive; they exist because the row's shape
/// has no `Default`.
fn blank_spell(spell_id: u32) -> Spell {
    Spell {
        spell_id,
        name: String::new(),
        power_type: 0,
        cost: 0,
        cast_time_ms: 0,
        gcd_ms: 0,
        cooldown_ms: 0,
        range_yd: 0,
        duration_ms: 0,
        school_mask: 0,
        dispel_type: 0,
        mechanic: 0,
        max_stacks: 0,
        aura_interrupt: 0,
        attributes: 0,
        spell_level: 0,
        max_level: 0,
        is_negative: false,
        cast_flags: 0,
        stances: 0,
        family_name: 0,
        family_flags: 0,
        proc_flags: 0,
        proc_chance: 0,
        proc_charges: 0,
    }
}

/// A Package spell effect before any claim has been applied to it. `id` is the derived packed key,
/// never authored.
fn blank_spell_effect(spell_id: u32, effect_index: u8) -> SpellEffect {
    SpellEffect {
        id: lyracore_package_delta::packed_spell_effect_id(spell_id, effect_index),
        spell_id,
        effect_index,
        kind: 0,
        base_points: 0,
        die_sides: 0,
        per_level: 0.0,
        period_ms: 0,
        target: 0,
        radius_yd: 0.0,
        chain_targets: 0,
        trigger_spell: 0,
        effect_mechanic: 0,
        p0: 0,
        p0_kind: 0,
        p1: 0,
        script_id: 0,
        enters_combat: false,
    }
}

fn apply_spell_field(spell: &mut Spell, field: &str, value: &FieldValue) -> Result<(), String> {
    match field {
        "name" => spell.name = as_str(field, value)?,
        "power_type" => spell.power_type = as_u8(field, value)?,
        "cost" => spell.cost = as_u32(field, value)?,
        "cast_time_ms" => spell.cast_time_ms = as_u32(field, value)?,
        "gcd_ms" => spell.gcd_ms = as_u32(field, value)?,
        "cooldown_ms" => spell.cooldown_ms = as_u32(field, value)?,
        "range_yd" => spell.range_yd = as_u32(field, value)?,
        "duration_ms" => spell.duration_ms = as_u32(field, value)?,
        "school_mask" => spell.school_mask = as_u8(field, value)?,
        "dispel_type" => spell.dispel_type = as_u8(field, value)?,
        "mechanic" => spell.mechanic = as_u8(field, value)?,
        "max_stacks" => spell.max_stacks = as_u8(field, value)?,
        "aura_interrupt" => spell.aura_interrupt = as_u16(field, value)?,
        "attributes" => spell.attributes = as_u32(field, value)?,
        "spell_level" => spell.spell_level = as_u8(field, value)?,
        "max_level" => spell.max_level = as_u8(field, value)?,
        "is_negative" => spell.is_negative = as_bool(field, value)?,
        "cast_flags" => spell.cast_flags = as_u32(field, value)?,
        "stances" => spell.stances = as_u8(field, value)?,
        "family_name" => spell.family_name = as_u8(field, value)?,
        "family_flags" => spell.family_flags = as_u64(field, value)?,
        "proc_flags" => spell.proc_flags = as_u32(field, value)?,
        "proc_chance" => spell.proc_chance = as_u8(field, value)?,
        "proc_charges" => spell.proc_charges = as_u8(field, value)?,
        other => return Err(format!("`game_spell` has no claimable column `{other}`")),
    }
    Ok(())
}

fn apply_effect_field(
    effect: &mut SpellEffect,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "kind" => effect.kind = as_u8(field, value)?,
        "base_points" => effect.base_points = as_i32(field, value)?,
        "die_sides" => effect.die_sides = as_i32(field, value)?,
        "per_level" => effect.per_level = as_f32(field, value)?,
        "period_ms" => effect.period_ms = as_u32(field, value)?,
        "target" => effect.target = as_u8(field, value)?,
        "radius_yd" => effect.radius_yd = as_f32(field, value)?,
        "chain_targets" => effect.chain_targets = as_u8(field, value)?,
        "trigger_spell" => effect.trigger_spell = as_u32(field, value)?,
        "effect_mechanic" => effect.effect_mechanic = as_u8(field, value)?,
        "p0" => effect.p0 = as_i32(field, value)?,
        "p0_kind" => effect.p0_kind = as_u8(field, value)?,
        "p1" => effect.p1 = as_i32(field, value)?,
        "script_id" => effect.script_id = as_u32(field, value)?,
        "enters_combat" => effect.enters_combat = as_bool(field, value)?,
        other => {
            return Err(format!(
                "`game_spell_effect` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

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

fn built_spell(row: &TracedRow) -> Result<Spell, String> {
    check_insert_is_whole(row)?;
    let mut spell = blank_spell(row.key().spell_id());
    for (field, claimed) in row.fields() {
        apply_spell_field(&mut spell, field, &claimed.value)?;
    }
    Ok(spell)
}

fn built_spell_effect(row: &TracedRow) -> Result<SpellEffect, String> {
    check_insert_is_whole(row)?;
    let PrimaryKey::SpellEffect {
        spell_id,
        effect_index,
    } = row.key()
    else {
        return Err(format!(
            "`game_spell_effect` row {} has no effect index",
            row.key()
        ));
    };
    let mut effect = blank_spell_effect(spell_id, effect_index);
    for (field, claimed) in row.fields() {
        apply_effect_field(&mut effect, field, &claimed.value)?;
    }
    Ok(effect)
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
fn check_update_targets(ctx: &ReducerContext, plan: &ApplyPlan) -> Result<(), String> {
    for row in &plan.rows {
        if row.operation() != Operation::Update {
            continue;
        }
        if updates_an_uninvented_package_row(row) {
            return Err(format!(
                "`{}` row {}: an enabled Package tunes it, but no enabled Package inserts it",
                row.table(),
                row.key()
            ));
        }
        let present = match row.table() {
            ClaimTable::Spell => ctx
                .db
                .game_spell()
                .spell_id()
                .find(row.key().spell_id())
                .is_some(),
            ClaimTable::SpellEffect => ctx.db.game_spell_effect().id().find(row.row_id()).is_some(),
        };
        if !present {
            return Err(format!(
                "`{}` row {} is not in this shard; the base import has to run before its Package \
                 Deltas",
                row.table(),
                row.key()
            ));
        }
    }
    Ok(())
}

/// Removes every row a Package invented, so a Package that left the enabled set takes its rows with
/// it. A whole-table pass over both spell tables, once per import, right after the base import
/// rewrote them anyway.
fn clear_package_spell_range(ctx: &ReducerContext) {
    let effects = ctx.db.game_spell_effect();
    let stale_effects: Vec<u64> = effects
        .iter()
        .filter(|effect| is_package_spell_id(effect.spell_id))
        .map(|effect| effect.id)
        .collect();
    for id in stale_effects {
        effects.id().delete(id);
    }

    let spells = ctx.db.game_spell();
    let stale_spells: Vec<u32> = spells
        .iter()
        .filter(|spell| is_package_spell_id(spell.spell_id))
        .map(|spell| spell.spell_id)
        .collect();
    for spell_id in stale_spells {
        spells.spell_id().delete(spell_id);
    }
}

fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    match (row.table(), row.operation()) {
        (ClaimTable::Spell, Operation::Insert) => {
            ctx.db
                .game_spell()
                .try_insert(built_spell(row)?)
                .map_err(|e| format!("`game_spell` row {} did not insert: {e}", row.key()))?;
        }
        (ClaimTable::Spell, Operation::Update) => {
            let spells = ctx.db.game_spell();
            let mut spell = spells
                .spell_id()
                .find(row.key().spell_id())
                .ok_or_else(|| format!("`game_spell` row {} vanished mid-apply", row.key()))?;
            for (field, claimed) in row.fields() {
                apply_spell_field(&mut spell, field, &claimed.value)?;
            }
            spells.spell_id().update(spell);
        }
        (ClaimTable::SpellEffect, Operation::Insert) => {
            ctx.db
                .game_spell_effect()
                .try_insert(built_spell_effect(row)?)
                .map_err(|e| {
                    format!("`game_spell_effect` row {} did not insert: {e}", row.key())
                })?;
        }
        (ClaimTable::SpellEffect, Operation::Update) => {
            let effects = ctx.db.game_spell_effect();
            let mut effect = effects.id().find(row.row_id()).ok_or_else(|| {
                format!("`game_spell_effect` row {} vanished mid-apply", row.key())
            })?;
            for (field, claimed) in row.fields() {
                apply_effect_field(&mut effect, field, &claimed.value)?;
            }
            effects.id().update(effect);
        }
    }
    Ok(())
}

/// Rewrites this family's provenance wholesale, so the table always describes the Packages the
/// shard is running now rather than every Package it ever ran.
fn stamp_provenance(ctx: &ReducerContext, family: &str, plan: &ApplyPlan) {
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
        });
    }
}

// ===========================================================================================
//  Tests — the pure half. Row WRITING needs a live ReducerContext, which a native test has no
//  way to build (same limit `import_meta`'s tests note); the wire suite covers that rung.
// ===========================================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_package_delta::{Column, FieldType};

    const HASH_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const PACKAGE_SPELL: u32 = 6_000_001;
    const REAL_SPELL: u32 = 133;

    const WHOLE_SPELL_ROW: &str = r#"{
        "name": { "type": "string", "value": "Kindled Bolt" },
        "power_type": { "type": "u8", "value": 0 },
        "cost": { "type": "u32", "value": 30 },
        "cast_time_ms": { "type": "u32", "value": 2500 },
        "gcd_ms": { "type": "u32", "value": 1500 },
        "cooldown_ms": { "type": "u32", "value": 8000 },
        "range_yd": { "type": "u32", "value": 30 },
        "duration_ms": { "type": "u32", "value": 0 },
        "school_mask": { "type": "u8", "value": 4 },
        "dispel_type": { "type": "u8", "value": 1 },
        "mechanic": { "type": "u8", "value": 0 },
        "max_stacks": { "type": "u8", "value": 1 },
        "aura_interrupt": { "type": "u16", "value": 3 },
        "attributes": { "type": "u32", "value": 0 },
        "spell_level": { "type": "u8", "value": 10 },
        "max_level": { "type": "u8", "value": 60 },
        "is_negative": { "type": "bool", "value": true },
        "cast_flags": { "type": "u32", "value": 0 },
        "stances": { "type": "u8", "value": 0 },
        "family_name": { "type": "u8", "value": 3 },
        "family_flags": { "type": "u64", "value": "18446744073709551615" },
        "proc_flags": { "type": "u32", "value": 0 },
        "proc_chance": { "type": "u8", "value": 0 },
        "proc_charges": { "type": "u8", "value": 0 }
    }"#;

    const WHOLE_EFFECT_ROW: &str = r#"{
        "kind": { "type": "u8", "value": 2 },
        "base_points": { "type": "i32", "value": 120 },
        "die_sides": { "type": "i32", "value": 10 },
        "per_level": { "type": "f32", "value": 1.5 },
        "period_ms": { "type": "u32", "value": 0 },
        "target": { "type": "u8", "value": 1 },
        "radius_yd": { "type": "f32", "value": 0.0 },
        "chain_targets": { "type": "u8", "value": 0 },
        "trigger_spell": { "type": "u32", "value": 0 },
        "effect_mechanic": { "type": "u8", "value": 0 },
        "p0": { "type": "i32", "value": 0 },
        "p0_kind": { "type": "u8", "value": 255 },
        "p1": { "type": "i32", "value": 0 },
        "script_id": { "type": "u32", "value": 0 },
        "enters_combat": { "type": "bool", "value": false }
    }"#;

    /// Wraps claims in an artifact envelope, on ONE line. The fixtures above are indented so a
    /// reader can follow them; an artifact in a payload never is.
    fn artifact(package: &str, claims: &str) -> String {
        let claims: String = claims.lines().map(str::trim).collect();
        format!(
            r#"{{"version":1,"package":"{package}","source_hash":"{HASH_A}","claims":[{claims}]}}"#
        )
    }

    fn spell_claim(spell_id: u32, operation: &str, fields: &str) -> String {
        format!(
            r#"{{"table":"game_spell","key":{{"spell_id":{spell_id}}},"operation":"{operation}","fields":{fields}}}"#
        )
    }

    fn effect_claim(spell_id: u32, effect_index: u8, operation: &str, fields: &str) -> String {
        format!(
            r#"{{"table":"game_spell_effect","key":{{"spell_id":{spell_id},"effect_index":{effect_index}}},"operation":"{operation}","fields":{fields}}}"#
        )
    }

    /// A value of the column's declared type, so a test can claim any column without spelling out
    /// what it holds.
    fn some_value(column: Column) -> FieldValue {
        match column.ty {
            FieldType::U8 => FieldValue::U8(7),
            FieldType::U16 => FieldValue::U16(7),
            FieldType::U32 => FieldValue::U32(7),
            FieldType::U64 => FieldValue::U64(7),
            FieldType::I32 => FieldValue::I32(-7),
            FieldType::F32 => FieldValue::F32(7.5),
            FieldType::Bool => FieldValue::Bool(true),
            FieldType::Str => FieldValue::Str("seven".to_owned()),
        }
    }

    fn plan(artifacts: &[String]) -> Result<ApplyPlan, String> {
        ApplyPlan::read(&artifacts.join(&ARTIFACT_SEPARATOR.to_string()))
    }

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

    /// The Package spell range is cleared on every apply, so tuning a Package spell nobody enables
    /// is a plan that names a row which will not exist.
    #[test]
    fn tuning_a_package_spell_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &spell_claim(
                PACKAGE_SPELL,
                "update",
                r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_base_spell_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &spell_claim(
                REAL_SPELL,
                "update",
                r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn an_inserted_spell_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &spell_claim(PACKAGE_SPELL, "insert", WHOLE_SPELL_ROW),
        )])
        .expect("plan builds");

        let spell = built_spell(&plan.rows[0]).expect("row builds");

        assert_eq!(spell.spell_id, PACKAGE_SPELL);
        assert_eq!(spell.name, "Kindled Bolt");
        assert_eq!(spell.cooldown_ms, 8000);
        assert_eq!(spell.aura_interrupt, 3);
        assert_eq!(spell.family_flags, u64::MAX);
        assert!(spell.is_negative);
    }

    /// The packed key is derived from the spell and the effect index, never authored, so the built
    /// row must carry the same value `game_aura.effect_id` is reproducible from.
    #[test]
    fn an_inserted_effect_derives_its_packed_key_from_the_spell_and_the_index() {
        let plan = plan(&[artifact(
            "example.bolt",
            &effect_claim(PACKAGE_SPELL, 2, "insert", WHOLE_EFFECT_ROW),
        )])
        .expect("plan builds");

        let effect = built_spell_effect(&plan.rows[0]).expect("row builds");

        assert_eq!(effect.id, (u64::from(PACKAGE_SPELL) << 2) | 2);
        assert_eq!(effect.spell_id, PACKAGE_SPELL);
        assert_eq!(effect.effect_index, 2);
        assert_eq!(effect.base_points, 120);
        assert!((effect.per_level - 1.5).abs() < f32::EPSILON);
        assert_eq!(effect.p0_kind, 255);
    }

    /// The Claim schema and the setters above are one contract. A column the schema declares but no
    /// setter names would fail an apply against a live shard and nowhere else.
    #[test]
    fn every_claimable_spell_column_has_a_setter() {
        let mut spell = blank_spell(PACKAGE_SPELL);
        for column in ClaimTable::Spell.columns() {
            apply_spell_field(&mut spell, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_spell` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn every_claimable_effect_column_has_a_setter() {
        let mut effect = blank_spell_effect(PACKAGE_SPELL, 0);
        for column in ClaimTable::SpellEffect.columns() {
            apply_effect_field(&mut effect, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_spell_effect` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut spell = blank_spell(PACKAGE_SPELL);

        let refusal = apply_spell_field(&mut spell, "cooldown_ms", &FieldValue::U8(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("cooldown_ms"), "{refusal}");
    }
}
