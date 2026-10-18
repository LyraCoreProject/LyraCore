use spacetimedb::{ReducerContext, Table};

use crate::tables::{
    game_aura, game_character, game_character_buyback, game_character_talent, game_item_instance,
    game_player_skill, game_player_spell, game_school_lockout, game_spell, game_spell_cd,
    game_spell_cooldown, game_talent, game_talent_tab, game_world_entity, CharacterTalent,
};

// This erases match power; callers restrict it to Characters on a dedicated mode realm.
pub(crate) fn reset_loadout(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    crate::package_party::require_authority(ctx)?;
    let character = crate::helpers::character_by_guid(ctx, guid).ok_or("Missing Character")?;
    crate::trade::cancel_trade_for(ctx, guid);
    crate::combat::disengage(ctx, guid);
    crate::spell::interrupt_cast(ctx, guid);
    crate::spell::break_channel(ctx, guid);
    crate::creatures::despawn_pets(ctx, guid);
    clear_hunter_pet(ctx, guid);
    crate::spell::form::clear(ctx, guid);
    crate::ghost::finish_resurrection(ctx, guid);
    crate::spell::clear_resurrect_request_and_option(ctx, guid);
    let auras = ctx.db.game_aura().by_target().filter(&guid).collect();
    crate::spell::shed_auras(ctx, guid, auras);
    for item in ctx.db.game_item_instance().by_owner_guid().filter(&guid) {
        ctx.db.game_item_instance().guid().delete(item.guid);
    }
    for item in ctx
        .db
        .game_character_buyback()
        .by_player_guid()
        .filter(&guid)
    {
        ctx.db.game_character_buyback().id().delete(item.id);
    }
    for spell in ctx.db.game_player_spell().by_character().filter(&guid) {
        ctx.db.game_player_spell().id().delete(spell.id);
    }
    for talent in ctx.db.game_character_talent().by_character().filter(&guid) {
        ctx.db.game_character_talent().id().delete(talent.id);
    }
    for skill in ctx.db.game_player_skill().by_character().filter(&guid) {
        ctx.db.game_player_skill().id().delete(skill.id);
    }
    for cooldown in ctx.db.game_spell_cd().by_caster().filter(&guid) {
        ctx.db.game_spell_cd().id().delete(cooldown.id);
    }
    for lockout in ctx.db.game_school_lockout().by_caster().filter(&guid) {
        ctx.db.game_school_lockout().id().delete(lockout.id);
    }
    ctx.db.game_spell_cooldown().caster_guid().delete(guid);
    crate::stats::set_character_level(ctx, guid, 1)?;
    crate::spell::spellbook::grant_createinfo_spells(
        ctx,
        guid,
        character.owner_identity,
        character.race,
        character.class,
    );
    crate::items::grant_starter_item(ctx, guid, character.owner_identity)?;
    crate::skill::ensure_player_skills(ctx, guid, character.owner_identity, 1, character.class);
    crate::spell::apply_racial_passives(ctx, guid, character.race, 1);
    crate::spell::recompute_sheet(ctx, guid);
    if let Some(mut body) = ctx.db.game_world_entity().guid().find(guid) {
        body.money = 0;
        body.xp = 0;
        body.stance = 0;
        body.unit_bytes_1 &= 0xFF00_0000;
        body.update(ctx);
    }
    let mut character = crate::helpers::character_by_guid(ctx, guid).ok_or("Missing Character")?;
    character.money = 0;
    character.xp = 0;
    character.health = 0;
    character.power = 0;
    character.rested_xp = 0;
    character.death_expire_micros = 0;
    character.pending_ghost = false;
    ctx.db.game_character().guid().update(character);
    refill(ctx, guid);
    Ok(())
}

pub(crate) fn advance_match_level(
    ctx: &ReducerContext,
    guid: u64,
    level: u32,
) -> Result<(), String> {
    if !(1..=60).contains(&level) || !crate::package_combat::active(ctx, guid) {
        return Err("Match leveling requires an admitted Character and level 1 through 60".into());
    }
    let character = crate::helpers::character_by_guid(ctx, guid).ok_or("Missing Character")?;
    crate::stats::set_character_level(ctx, guid, level)?;
    crate::skill::ensure_player_skills(ctx, guid, character.owner_identity, level, character.class);
    for mut skill in ctx.db.game_player_skill().by_character().filter(guid) {
        if crate::skill::is_combat_skill_line(skill.skill_line) {
            skill.current = (level * 5) as u16;
            skill.max_rank = skill.current;
            ctx.db.game_player_skill().id().update(skill);
        }
    }
    Ok(())
}

fn clear_hunter_pet(ctx: &ReducerContext, guid: u64) {
    use crate::creatures::{game_hunter_pet, game_hunter_pet_protocol};
    for pet in ctx.db.game_hunter_pet().by_owner().filter(guid) {
        ctx.db
            .game_hunter_pet_protocol()
            .pet_id()
            .delete(pet.pet_id);
        ctx.db.game_hunter_pet().pet_id().delete(pet.pet_id);
    }
}

pub(crate) fn provision_hunter_pet(
    ctx: &ReducerContext,
    guid: u64,
    entry: u32,
    restore: bool,
) -> Result<(), String> {
    use crate::creatures::{
        game_creature_family, game_creature_spawn, game_creature_template, game_hunter_pet,
    };
    if !crate::package_combat::active(ctx, guid) {
        return Err("Pet provisioning requires match admission".into());
    }
    let owner = crate::helpers::live_entity(ctx, guid)?;
    if owner.class() != 3 {
        return Err("Only a Hunter can receive a Hunter pet".into());
    }
    if owner.dead {
        return Ok(());
    }
    let template = ctx
        .db
        .game_creature_template()
        .entry()
        .find(entry)
        .ok_or("Pet template is not imported")?;
    if ctx
        .db
        .game_creature_family()
        .family_id()
        .find(u32::from(template.creature_family))
        .is_none_or(|family| family.pet_talent_type < 0)
    {
        return Err("Pet family is not tameable".into());
    }
    let existing = crate::creatures::pet_of(ctx, guid);
    if existing.as_ref().is_none_or(|pet| pet.dead) {
        if !restore {
            return Ok(());
        }
        if let Some(learned) = crate::creatures::hunter_pet_for_owner(ctx, guid) {
            let dead = existing.as_ref().map_or(learned.dead, |pet| pet.dead);
            crate::creatures::restore_hunter_pet(ctx, guid, dead.then_some(100))?;
        } else {
            crate::creatures::despawn_pets(ctx, guid);
            let wild = crate::encounter::spawn_wave(
                ctx,
                owner.instance_id,
                0,
                owner.map_id,
                &[entry],
                owner.x,
                owner.y,
                owner.z,
                owner.orientation,
            );
            let wild_guid = *wild.first().ok_or("Pet could not enter the Instance")?;
            let mut body = crate::helpers::live_entity(ctx, wild_guid)?;
            body.level = owner.level;
            body.update(ctx);
            crate::creatures::tame_creature(ctx, guid, wild_guid)?;
            ctx.db.game_creature_spawn().guid().delete(wild_guid);
        }
    }
    let mut pet = crate::creatures::pet_of(ctx, guid).ok_or("Provisioned pet is unavailable")?;
    let health = (u64::from(template.health.max(1)) * u64::from(owner.level)
        / u64::from(template.level.max(1)))
    .clamp(1, u64::from(u32::MAX)) as u32;
    pet.health = if restore {
        health
    } else {
        (u64::from(pet.health) * u64::from(health) / u64::from(pet.max_health.max(1)))
            .min(u64::from(health)) as u32
    };
    pet.max_health = health;
    pet.level = owner.level;
    pet.sheet_dmg_min = owner.level.saturating_mul(2).max(1);
    pet.sheet_dmg_max = owner.level.saturating_mul(3).max(2);
    if let Some(mut learned) = crate::creatures::hunter_pet_for_owner(ctx, guid) {
        learned.level = owner.level;
        learned.dead = false;
        learned.saved_health = pet.health;
        learned.saved_max_health = pet.max_health;
        learned.saved_damage_min = pet.sheet_dmg_min;
        learned.saved_damage_max = pet.sheet_dmg_max;
        learned.pet_xp = 0;
        if restore {
            learned.happiness = 1_000_000;
        }
        learned.care_updated_at = ctx.timestamp;
        crate::creatures::publish_hunter_pet_protocol(
            ctx,
            &learned,
            lyracore_shared::pet::pet_guid_for(guid),
        );
        ctx.db.game_hunter_pet().pet_id().update(learned);
    }
    pet.update(ctx);
    Ok(())
}

pub(crate) fn grant_spell(ctx: &ReducerContext, guid: u64, spell_id: u32) -> Result<(), String> {
    let character = crate::helpers::character_by_guid(ctx, guid).ok_or("Missing Character")?;
    let spell = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .ok_or("Spell is not imported")?;
    crate::spell::learn_spell_with_dependents(ctx, guid, character.owner_identity, spell_id);
    if spell.attributes & lyracore_shared::spell::taxonomy::SPELL_ATTR_PASSIVE != 0 {
        crate::spell::apply_spell_auras(ctx, spell_id, guid, character.level, 1);
        crate::spell::recompute_sheet(ctx, guid);
    }
    Ok(())
}

pub(crate) fn grant_talent(ctx: &ReducerContext, guid: u64, talent_id: u32) -> Result<(), String> {
    let character = crate::helpers::character_by_guid(ctx, guid).ok_or("Missing Character")?;
    let talent = ctx
        .db
        .game_talent()
        .talent_id()
        .find(talent_id)
        .ok_or("Talent is not imported")?;
    let tab = ctx
        .db
        .game_talent_tab()
        .tab_id()
        .find(talent.tab_id)
        .ok_or("Talent tree is not imported")?;
    if tab.class_mask & (1 << (character.class.saturating_sub(1))) == 0 || talent.max_rank == 0 {
        return Err("Talent does not belong to this class".into());
    }
    let rank_spell = crate::talent::talent_rank_spell(&talent, talent.max_rank);
    if rank_spell == 0 || ctx.db.game_spell().spell_id().find(rank_spell).is_none() {
        return Err("Talent rank spell is not imported".into());
    }
    let learned = ctx.db.game_character_talent();
    if let Some(mut row) = learned
        .by_character()
        .filter(&guid)
        .find(|row| row.talent_id == talent_id)
    {
        row.rank = talent.max_rank;
        learned.id().update(row);
    } else {
        learned.insert(CharacterTalent {
            id: 0,
            character_guid: guid,
            owner_identity: character.owner_identity,
            talent_id,
            rank: talent.max_rank,
        });
    }
    crate::talent::apply_learned_talents(
        ctx,
        guid,
        character.owner_identity,
        u32::from(character.level),
    );
    if talent.grant_spell_id != 0 {
        grant_spell(ctx, guid, talent.grant_spell_id)?;
    }
    Ok(())
}

pub(crate) fn refill(ctx: &ReducerContext, guid: u64) {
    if let Some(mut body) = ctx.db.game_world_entity().guid().find(guid) {
        if body.dead {
            return;
        }
        body.health = body.max_health;
        body.power = crate::stats::starting_power(body.power_type(), body.max_power);
        crate::spell::form::refill_mana(ctx, &body);
        body.update(ctx);
    }
}

pub(crate) fn respawn(
    ctx: &ReducerContext,
    guid: u64,
    at: (f32, f32, f32, f32),
) -> Result<(), String> {
    let body = crate::helpers::live_entity(ctx, guid)?;
    if ![at.0, at.1, at.2, at.3]
        .iter()
        .all(|value| value.is_finite())
    {
        return Err("Invalid respawn position".into());
    }
    crate::combat::disengage(ctx, guid);
    crate::ghost::finish_resurrection(ctx, guid);
    refill(ctx, guid);
    crate::world::teleport_player(
        ctx,
        guid,
        body.map_id,
        body.instance_id,
        at.0,
        at.1,
        at.2,
        at.3,
    );
    Ok(())
}

pub(crate) fn return_to_lobby(
    ctx: &ReducerContext,
    guid: u64,
    map_id: u32,
    instance_id: u64,
    at: (f32, f32, f32, f32),
) -> Result<(), String> {
    use crate::instance::{game_instance, game_package_instance};
    crate::package_party::require_authority(ctx)?;
    let mut character = crate::helpers::character_by_guid(ctx, guid).ok_or("Missing Character")?;
    if ![at.0, at.1, at.2, at.3]
        .iter()
        .all(|value| value.is_finite())
        || crate::taxi::is_in_flight(ctx, guid)
    {
        return Err("Character cannot enter the lobby".into());
    }
    let owner = ctx
        .db
        .game_package_instance()
        .instance_id()
        .find(instance_id)
        .ok_or("Destination is not a Package lobby")?;
    let instance = ctx
        .db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .ok_or("Lobby Instance is unavailable")?;
    if instance.map_id != map_id || instance.party_id != 0 || instance.reset_requested {
        return Err("Destination is not an open Package lobby".into());
    }
    character.home_map = map_id;
    character.home_x = at.0;
    character.home_y = at.1;
    character.home_z = at.2;
    if ctx.db.game_world_entity().guid().find(guid).is_none() {
        character.map_id = map_id;
        character.pending_instance_id = 0;
        character.x = at.0;
        character.y = at.1;
        character.z = at.2;
        character.orientation = at.3;
    }
    ctx.db.game_character().guid().update(character);
    crate::combat::disengage(ctx, guid);
    crate::creatures::despawn_pets(ctx, guid);
    if let Some(body) = ctx.db.game_world_entity().guid().find(guid) {
        if body.map_id == map_id {
            crate::instance::enter_package_instance(
                ctx,
                &owner.package_name,
                guid,
                instance_id,
                at,
            )?;
        } else {
            // Complete the cross-map World Session before admitting its body to the owned lobby.
            crate::world::teleport_player(ctx, guid, map_id, 0, at.0, at.1, at.2, at.3);
        }
    }
    Ok(())
}
