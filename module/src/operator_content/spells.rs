use std::collections::BTreeSet;

use lyracore_package_delta::ids::{
    is_fixture_reserved_spell_id, is_fixture_reserved_trainer_id, is_operator_spell_id,
    is_package_spell_id, is_package_trainer_id, packed_spell_effect_id,
};
use lyracore_shared::spell::taxonomy::{EffectKind, P_RAW, T_SCRIPTED};
use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table};

use crate::creatures::{game_creature_ai_spell_metadata, CreatureAiSpellMetadata};
use crate::items::game_item_template;
use crate::spell::{
    game_spell, game_spell_effect, game_spell_reagent, Spell, SpellEffect, SpellReagent,
};
use crate::trainer::{game_trainer_spell, TrainerSpell};

#[derive(SpacetimeType, Clone)]
pub struct SpellSourceField {
    pub name: String,
    pub value: String,
}

#[table(accessor = game_operator_spell_source)]
pub struct OperatorSpellSource {
    #[primary_key]
    pub spell_id: u32,
    pub fields: Vec<SpellSourceField>,
}

#[derive(SpacetimeType)]
pub struct SpellEdit {
    pub create: bool,
    pub spell: Spell,
    pub effects: Vec<SpellEffect>,
    pub reagents: Vec<SpellReagent>,
    pub trainers: Vec<TrainerSpell>,
    pub metadata: Vec<CreatureAiSpellMetadata>,
    pub source: Vec<SpellSourceField>,
}

#[reducer]
pub fn operator_apply_spell_edits(
    ctx: &ReducerContext,
    edits: Vec<SpellEdit>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if edits.len() > 64 {
        return Err("a batch holds at most 64 spell edits".into());
    }
    let mut ids = BTreeSet::new();
    for edit in &edits {
        let id = edit.spell.spell_id;
        if !ids.insert(id) {
            return Err(format!("duplicate spell {id}"));
        }
    }
    for edit in &edits {
        check_edit(ctx, edit, &ids)
            .map_err(|reason| format!("spell {}: {reason}", edit.spell.spell_id))?;
    }
    for edit in edits {
        let id = edit.spell.spell_id;
        ctx.db.game_spell_effect().by_spell().delete(id);
        ctx.db.game_spell_reagent().by_spell().delete(id);
        ctx.db.game_trainer_spell().by_spell().delete(id);
        ctx.db
            .game_creature_ai_spell_metadata()
            .spell_id()
            .delete(id);
        ctx.db.game_spell().spell_id().delete(id);
        ctx.db.game_spell().insert(edit.spell);
        for row in edit.effects {
            ctx.db.game_spell_effect().insert(row);
        }
        for row in edit.reagents {
            ctx.db.game_spell_reagent().insert(row);
        }
        for row in edit.trainers {
            ctx.db.game_trainer_spell().insert(row);
        }
        for row in edit.metadata {
            ctx.db.game_creature_ai_spell_metadata().insert(row);
        }
        ctx.db.game_operator_spell_source().spell_id().delete(id);
        ctx.db
            .game_operator_spell_source()
            .insert(OperatorSpellSource {
                spell_id: id,
                fields: edit.source,
            });
    }
    for id in ids {
        super::log_change(ctx, &format!("saved spell {id}"));
    }
    Ok(())
}

fn check_edit(ctx: &ReducerContext, edit: &SpellEdit, ids: &BTreeSet<u32>) -> Result<(), String> {
    let spell = &edit.spell;
    let id = spell.spell_id;
    if id == 0 || is_package_spell_id(id) || is_fixture_reserved_spell_id(id) {
        return Err("the spell is zero, Package-owned or Fixture-Reserved".into());
    }
    let exists = ctx.db.game_spell().spell_id().find(id).is_some();
    if edit.create {
        if !is_operator_spell_id(id) {
            return Err("new spells need an id in the Operator Spell Range".into());
        }
        if exists {
            return Err("the new spell id is already used; choose another id".into());
        }
    } else if !exists {
        return Err("the spell does not exist".into());
    }
    if spell.name.trim().is_empty() || spell.name.len() > 255 || spell.name.contains('\0') {
        return Err("a spell needs a name of 1 through 255 bytes without NUL".into());
    }
    if spell.school_mask == 0
        || spell.school_mask & !0x7f != 0
        || !matches!(spell.power_type, 0..=4 | 254 | 255)
        || spell.custom_flags & !lyracore_shared::spell_custom::KNOWN != 0
    {
        return Err("invalid school, power type or custom flags".into());
    }
    check_children(ctx, edit, ids)?;
    check_trainers(ctx, edit)?;
    check_source(edit)
}

fn check_children(
    ctx: &ReducerContext,
    edit: &SpellEdit,
    ids: &BTreeSet<u32>,
) -> Result<(), String> {
    let id = edit.spell.spell_id;
    if edit.effects.len() > 4
        || edit.reagents.len() > 8
        || edit.trainers.len() > 256
        || edit.metadata.len() > 1
    {
        return Err("too many spell effects, reagents, trainer offers or AI metadata rows".into());
    }
    let spell_exists = |target| {
        target == 0
            || ids.contains(&target)
            || ctx.db.game_spell().spell_id().find(target).is_some()
    };
    let mut effects = BTreeSet::new();
    for row in &edit.effects {
        if row.spell_id != id
            || row.effect_index > 3
            || row.id != packed_spell_effect_id(id, row.effect_index)
            || !effects.insert(row.id)
        {
            return Err("invalid or duplicate effect key".into());
        }
        if EffectKind::try_from(row.kind).is_err()
            || row.target > T_SCRIPTED
            || (row.p0_kind > 16 && row.p0_kind != P_RAW)
            || ![
                row.per_level,
                row.radius_yd,
                row.points_per_combo,
                row.bonus_coefficient,
            ]
            .iter()
            .all(|n| n.is_finite())
            || row.radius_yd < 0.0
            || row.bonus_coefficient < -1.0
        {
            return Err("invalid effect kind, target, parameter or magnitude".into());
        }
        if !spell_exists(row.trigger_spell) {
            return Err(format!(
                "trigger spell {} does not exist",
                row.trigger_spell
            ));
        }
    }
    let mut reagents = BTreeSet::new();
    for row in &edit.reagents {
        if row.spell_id != id
            || row.id >> 3 != u64::from(id)
            || !reagents.insert(row.id)
            || row.count == 0
            || row.item_entry == 0
        {
            return Err("invalid or duplicate reagent".into());
        }
        if ctx
            .db
            .game_item_template()
            .entry()
            .find(row.item_entry)
            .is_none()
        {
            return Err(format!("reagent item {} does not exist", row.item_entry));
        }
    }
    for row in &edit.metadata {
        if row.spell_id != id {
            return Err("AI metadata belongs to another spell".into());
        }
    }
    Ok(())
}

fn check_trainers(ctx: &ReducerContext, edit: &SpellEdit) -> Result<(), String> {
    let id = edit.spell.spell_id;
    let mut trainers = BTreeSet::new();
    for row in ctx.db.game_trainer_spell().by_spell().filter(id) {
        if is_package_trainer_id(row.id) || is_fixture_reserved_trainer_id(row.id) {
            return Err("trainer offer is Package-owned or Fixture-Reserved".into());
        }
    }
    for row in &edit.trainers {
        let old = ctx.db.game_trainer_spell().id().find(row.id);
        if row.spell_id != id
            || !trainers.insert(row.id)
            || old.is_none_or(|old| old.spell_id != id || old.trainer_entry != row.trainer_entry)
        {
            return Err("trainer offers must retain their existing spell and trainer keys".into());
        }
    }
    Ok(())
}

fn check_source(edit: &SpellEdit) -> Result<(), String> {
    let spell = &edit.spell;
    let id = spell.spell_id;
    if edit.source.len() > 256
        || edit
            .source
            .iter()
            .map(|f| f.name.len() + f.value.len())
            .sum::<usize>()
            > 64 * 1024
    {
        return Err("spell source exceeds 256 fields or 64 KiB".into());
    }
    let mut fields = BTreeSet::new();
    for field in &edit.source {
        if field.name.is_empty()
            || !fields.insert(&field.name)
            || field.name.contains('\0')
            || field.value.contains('\0')
        {
            return Err("invalid or duplicate spell source field".into());
        }
    }
    let field = |name| {
        edit.source
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.value.as_str())
    };
    if field("entry").and_then(|v| v.parse::<u32>().ok()) != Some(id)
        || field("name") != Some(spell.name.as_str())
        || field("build")
            .and_then(|v| v.parse::<u32>().ok())
            .is_none_or(|build| build > 5875)
    {
        return Err(
            "spell source entry and name must match the compiled spell, with a build through 5875"
                .into(),
        );
    }
    Ok(())
}
