use spacetimedb::{table, ReducerContext, Table};

use crate::instance::{game_instance, game_package_instance};
use crate::{game_spell, game_world_entity};

pub(crate) const RESURRECTION_COOLDOWN_MS: u32 = 120_000;

#[table(
    accessor = game_package_combat_rule,
    index(accessor = by_instance, btree(columns = [instance_id]))
)]
pub struct PackageCombatRule {
    #[primary_key]
    pub character_guid: u64,
    pub package_name: String,
    pub instance_id: u64,
    pub essential_spell_ids: Vec<u32>,
    pub resurrect_ready_at_micros: i64,
    #[default(0u32)]
    pub original_faction_template: u32,
}

crate::character_owned!(delete, fn sweep_delete_game_package_combat_rule(ctx, character_guid) {
    ctx.db.game_package_combat_rule().character_guid().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_package_combat_rule());

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn enable(
    ctx: &ReducerContext,
    package_name: &str,
    character_guid: u64,
    instance_id: u64,
    essential_spell_ids: &[u32],
) -> Result<(), String> {
    if !installed(ctx, package_name) {
        return Err("Combat rules require an enabled Package".to_string());
    }
    crate::helpers::character_by_guid(ctx, character_guid)
        .ok_or("Character is unavailable or in Transfer")?;
    let owner = ctx
        .db
        .game_package_instance()
        .instance_id()
        .find(instance_id)
        .ok_or("Combat rules require a Package instance")?;
    let instance = ctx
        .db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .ok_or("Package instance is unavailable")?;
    let entity = crate::helpers::live_entity(ctx, character_guid)?;
    if !entity.is_player()
        || owner.package_name != package_name
        || instance.reset_requested
        || !crate::helpers::in_same_partition(&entity, instance.map_id, instance_id)
        || crate::group::group_of(ctx, character_guid)
            .is_none_or(|member| member.group_id != instance.party_id)
    {
        return Err("Combat rules require admission to the owning Package instance".to_string());
    }
    if essential_spell_ids.len() > 32
        || essential_spell_ids
            .iter()
            .any(|&spell_id| ctx.db.game_spell().spell_id().find(spell_id).is_none())
    {
        return Err("Combat rules require at most 32 existing essential spells".to_string());
    }
    let table = ctx.db.game_package_combat_rule();
    let existing = table.character_guid().find(character_guid);
    if existing
        .as_ref()
        .is_some_and(|rule| rule.package_name != package_name)
    {
        return Err("Combat rules belong to another Package".to_string());
    }
    let resurrect_ready_at_micros = existing
        .as_ref()
        .filter(|rule| rule.instance_id == instance_id)
        .map_or(0, |rule| rule.resurrect_ready_at_micros);
    let rule = PackageCombatRule {
        character_guid,
        package_name: package_name.to_string(),
        instance_id,
        essential_spell_ids: essential_spell_ids.to_vec(),
        resurrect_ready_at_micros,
        original_faction_template: existing.as_ref().map_or(entity.faction_template, |rule| {
            rule.original_faction_template
        }),
    };
    if existing.is_some() {
        table.character_guid().update(rule);
    } else {
        table.insert(rule);
    }
    project_faction(ctx, character_guid, 1);
    Ok(())
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn disable(
    ctx: &ReducerContext,
    package_name: &str,
    character_guid: u64,
) -> Result<(), String> {
    let table = ctx.db.game_package_combat_rule();
    if let Some(rule) = table.character_guid().find(character_guid) {
        if rule.package_name != package_name {
            return Err("Combat rules belong to another Package".to_string());
        }
        remove_rule(ctx, rule);
    }
    Ok(())
}

fn project_faction(ctx: &ReducerContext, character_guid: u64, faction_template: u32) {
    if let Some(mut entity) = ctx.db.game_world_entity().guid().find(character_guid) {
        entity.faction_template = faction_template;
        entity.update(ctx);
    }
    if let Some(mut pet) = crate::creatures::pet_of(ctx, character_guid) {
        pet.faction_template = faction_template;
        pet.update(ctx);
    }
}

fn remove_rule(ctx: &ReducerContext, rule: PackageCombatRule) {
    project_faction(ctx, rule.character_guid, rule.original_faction_template);
    ctx.db
        .game_package_combat_rule()
        .character_guid()
        .delete(rule.character_guid);
}

pub(crate) fn clear_package(ctx: &ReducerContext, package_name: &str) {
    let ident = package_name.replace('-', "_");
    for rule in ctx
        .db
        .game_package_combat_rule()
        .iter()
        .filter(|rule| rule.package_name.replace('-', "_") == ident)
        .collect::<Vec<_>>()
    {
        remove_rule(ctx, rule);
    }
}

pub(crate) fn clear_removed_packages(ctx: &ReducerContext) {
    for rule in ctx
        .db
        .game_package_combat_rule()
        .iter()
        .filter(|rule| !installed(ctx, &rule.package_name))
        .collect::<Vec<_>>()
    {
        remove_rule(ctx, rule);
    }
}

fn installed(ctx: &ReducerContext, package_name: &str) -> bool {
    let ident = package_name.replace('-', "_");
    crate::GAME_PACKAGES
        .iter()
        .any(|package| package.name == ident)
        && crate::package_teardown::runs(ctx, &ident)
}

fn rule(ctx: &ReducerContext, character_guid: u64) -> Option<PackageCombatRule> {
    let rule = ctx
        .db
        .game_package_combat_rule()
        .character_guid()
        .find(character_guid)?;
    crate::helpers::character_by_guid(ctx, character_guid)?;
    if !installed(ctx, &rule.package_name) {
        return None;
    }
    let owner = ctx
        .db
        .game_package_instance()
        .instance_id()
        .find(rule.instance_id)?;
    let instance = ctx
        .db
        .game_instance()
        .instance_id()
        .find(rule.instance_id)?;
    let entity = crate::helpers::live_entity(ctx, character_guid).ok()?;
    (owner.package_name == rule.package_name
        && entity.is_player()
        && crate::helpers::in_same_partition(&entity, instance.map_id, instance.instance_id))
    .then_some(rule)
}

pub(crate) fn active(ctx: &ReducerContext, character_guid: u64) -> bool {
    rule(ctx, character_guid).is_some()
}

pub(crate) fn relocation_allowed(
    ctx: &ReducerContext,
    guid: u64,
    map_id: u32,
    instance_id: u64,
) -> bool {
    let Some(rule) = ctx
        .db
        .game_package_combat_rule()
        .character_guid()
        .find(guid)
    else {
        return true;
    };
    if !installed(ctx, &rule.package_name) {
        return true;
    }
    ctx.db
        .game_instance()
        .instance_id()
        .find(rule.instance_id)
        .is_some_and(|instance| instance.map_id == map_id && instance.instance_id == instance_id)
}

pub(crate) fn require_transfer_allowed(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    if ctx
        .db
        .game_package_combat_rule()
        .character_guid()
        .find(guid)
        .is_some_and(|rule| installed(ctx, &rule.package_name))
    {
        return Err("Leave the Package match before Transfer".into());
    }
    Ok(())
}

pub(crate) fn essential_spell(ctx: &ReducerContext, character_guid: u64, spell_id: u32) -> bool {
    rule(ctx, character_guid).is_some_and(|rule| rule.essential_spell_ids.contains(&spell_id))
}

pub(crate) fn resurrection_gate(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    if rule(ctx, character_guid).is_some_and(|rule| {
        rule.resurrect_ready_at_micros > ctx.timestamp.to_micros_since_unix_epoch()
    }) {
        return Err("resurrection is on its Package cooldown".to_string());
    }
    Ok(())
}

pub(crate) fn commit_resurrection(ctx: &ReducerContext, character_guid: u64) {
    if let Some(mut rule) = rule(ctx, character_guid) {
        rule.resurrect_ready_at_micros = ctx
            .timestamp
            .to_micros_since_unix_epoch()
            .saturating_add(i64::from(RESURRECTION_COOLDOWN_MS) * 1_000);
        ctx.db
            .game_package_combat_rule()
            .character_guid()
            .update(rule);
    }
}

pub(crate) fn clear_instance(ctx: &ReducerContext, instance_id: u64) {
    let table = ctx.db.game_package_combat_rule();
    for rule in table.by_instance().filter(instance_id).collect::<Vec<_>>() {
        remove_rule(ctx, rule);
    }
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_defense_enable_combat_rule(
    ctx: &ReducerContext,
    package_name: String,
    character_guid: u64,
    instance_id: u64,
    essential_spell_ids: Vec<u32>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    enable(
        ctx,
        &package_name,
        character_guid,
        instance_id,
        &essential_spell_ids,
    )
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_defense_disable_combat_rule(
    ctx: &ReducerContext,
    package_name: String,
    character_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    disable(ctx, &package_name, character_guid)
}
