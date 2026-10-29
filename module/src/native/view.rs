use crate::guild::game_guild_member;
use crate::items::{game_item_instance, game_item_random_property, game_item_template};
use crate::{Character, WorldEntity};
use lyracore_protocol::codec;
use lyracore_protocol::messages::vanilla::{CharacterGear, InventoryType};
use spacetimedb::ReducerContext;

pub(super) fn character(ctx: &ReducerContext, character: Character) -> codec::CharacterView {
    let mut equipment = [CharacterGear::default(); 19];
    for item in ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(character.guid)
    {
        let Some(gear) = equipment.get_mut(usize::from(item.slot)) else {
            continue;
        };
        let Some(template) = ctx.db.game_item_template().entry().find(item.entry) else {
            continue;
        };
        *gear = CharacterGear {
            equipment_display_id: template.display_id,
            inventory_type: InventoryType::try_from(template.inventory_type).unwrap_or_default(),
        };
    }
    let guild_id = ctx
        .db
        .game_guild_member()
        .character_guid()
        .find(character.guid)
        .map_or(0, |member| member.guild_id);
    codec::CharacterView {
        guid: character.guid,
        name: character.name,
        race: character.race,
        class: character.class,
        gender: character.gender,
        skin: character.skin,
        face: character.face,
        hair_style: character.hair_style,
        hair_color: character.hair_color,
        facial_hair: character.facial_hair,
        level: character.level,
        map_id: character.map_id,
        zone_id: character.zone_id,
        x: character.x,
        y: character.y,
        z: character.z,
        first_login: character.first_login,
        equipment,
        played_total_secs: character.played_total_secs,
        session_start_micros: character.session_start_micros,
        guild_id,
    }
}
pub(super) fn entity(ctx: &ReducerContext, e: &WorldEntity) -> codec::EntityView {
    let character = crate::helpers::character_by_guid(ctx, e.guid);
    let guild = ctx.db.game_guild_member().character_guid().find(e.guid);
    codec::EntityView {
        guid: e.guid,
        map_id: e.map_id,
        instance_id: e.instance_id,
        watched_faction_index: character.as_ref().map(|c| c.watched_faction_index),
        zone_id: if e.zone_id != 0 {
            e.zone_id
        } else {
            character.as_ref().map_or(0, |c| c.zone_id)
        },
        x: e.x,
        y: e.y,
        z: e.z,
        orientation: e.orientation,
        last_move_ms: e.last_move_ms,
        movement_flags: e.movement_flags,
        run_speed_mult_bp: e.run_speed_mult_bp,
        type_mask: e.type_mask,
        entry: e.entry,
        scale_x: e.scale_x,
        health: e.health,
        max_health: e.max_health,
        power: e.power,
        max_power: e.max_power,
        level: e.level,
        faction_template: e.faction_template,
        target_guid: e.target_guid,
        unit_bytes_0: e.unit_bytes_0,
        display_id: e.display_id,
        native_display_id: e.native_display_id,
        unit_flags: e.unit_flags,
        mount_display_id: e.mount_display_id,
        base_attack_time_ms: e.base_attack_time_ms,
        offhand_attack_time_ms: if e.is_player() {
            let delay = crate::combat::equipped_offhand_weapon_damage(ctx, e.guid)
                .map(|(_, _, delay)| delay)
                .filter(|delay| *delay != 0)
                .unwrap_or(crate::combat::DEFAULT_ATTACK_TIME_MS);
            let haste = crate::spell::speed_bonus(ctx, e.guid, crate::spell::SPEED_SWING);
            Some(crate::combat::swing_time_with_haste(delay, haste))
        } else {
            None
        },
        dynamic_flags: e.dynamic_flags,
        player_bytes: e.player_bytes,
        player_bytes_2: e.player_bytes_2,
        player_bytes_3: e.player_bytes_3,
        player_flags: e.player_flags,
        xp: e.xp,
        next_level_xp: e.next_level_xp,
        money: e.money,
        unit_bytes_1: e.unit_bytes_1,
        unit_bytes_2: e.unit_bytes_2,
        strength: e.strength,
        agility: e.agility,
        stamina: e.stamina,
        intellect: e.intellect,
        spirit: e.spirit,
        sheet_ap_base: e.sheet_ap_base,
        sheet_ap_mods: e.sheet_ap_mods,
        sheet_dmg_min: e.sheet_dmg_min,
        sheet_dmg_max: e.sheet_dmg_max,
        sheet_ranged_ap: e.sheet_ranged_ap,
        sheet_ranged_dmg_min: e.sheet_ranged_dmg_min,
        sheet_ranged_dmg_max: e.sheet_ranged_dmg_max,
        npc_flags: e.npc_flags,
        effective_armor: crate::combat::effective_armor(ctx, e),
        magic_resistances: std::array::from_fn(|index| {
            lyracore_shared::resistance::effective_resistance(
                0,
                crate::spell::resistance_bonus(ctx, e.guid, 1 << (index + 1)),
            )
        }),
        owner_guid: e.owner_guid,
        home_map: character.as_ref().map_or(0, |c| c.home_map),
        home_zone: character.as_ref().map_or(0, |c| c.home_zone),
        home_x: character.as_ref().map_or(0.0, |c| c.home_x),
        home_y: character.as_ref().map_or(0.0, |c| c.home_y),
        home_z: character.as_ref().map_or(0.0, |c| c.home_z),
        guild_id: guild.as_ref().map_or(0, |g| g.guild_id),
        guild_rank: guild.as_ref().map_or(0, |g| g.rank_id),
        spline: None,
    }
}

pub(super) fn item(ctx: &ReducerContext, item: &crate::ItemInstance) -> codec::ItemInstanceView {
    let template = ctx.db.game_item_template().entry().find(item.entry);
    let enchantments = ctx
        .db
        .game_item_random_property()
        .property_id()
        .find(item.random_property_id)
        .map_or([0; 3], |p| [p.enchant_id_1, p.enchant_id_2, p.enchant_id_3]);
    codec::ItemInstanceView {
        guid: item.guid,
        entry: item.entry,
        owner_guid: item.owner_guid,
        slot: item.slot,
        stack_count: item.stack_count,
        durability: item.durability,
        random_property_id: item.random_property_id,
        item_text_id: item.item_text_id,
        enchantment: lyracore_shared::item_property::client_enchantment_id(item.enchant_id),
        max_durability: template
            .as_ref()
            .map_or(item.durability, |t| t.max_durability),
        container_slots: template.map_or(0, |t| t.container_slots),
        random_property_enchant_ids: enchantments,
    }
}

pub(super) fn items(ctx: &ReducerContext, guid: u64) -> Vec<codec::ItemInstanceView> {
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(guid)
        .map(|row| item(ctx, &row))
        .collect()
}

pub(super) fn quests(ctx: &ReducerContext, guid: u64) -> Vec<codec::update_mask::QuestLogSlot> {
    use crate::quest::{game_character_quest, game_quest_objective, objective_kind};
    let mut quests: Vec<_> = ctx
        .db
        .game_character_quest()
        .by_character()
        .filter(guid)
        .filter(|quest| !quest.rewarded)
        .collect();
    quests.sort_by_key(|quest| quest.quest_entry);
    quests
        .into_iter()
        .take(codec::update_mask::idx::QUEST_LOG_SLOTS as usize)
        .enumerate()
        .map(|(slot, quest)| {
            let state = u8::from(crate::quest::quest_is_complete(ctx, &quest));
            let mut counts = quest.counts;
            // The client redraws its quest tracker only when a counter changes, so carried
            // items fill their objective's counter as vmangos does.
            for objective in ctx
                .db
                .game_quest_objective()
                .by_quest()
                .filter(quest.quest_entry)
                .filter(|objective| objective.kind == objective_kind::COLLECT_ITEM)
            {
                if let Some(count) = counts.get_mut(usize::from(objective.obj_index)) {
                    *count = crate::items::item_count(ctx, guid, objective.target_entry)
                        .min(objective.required_count);
                }
            }
            codec::update_mask::QuestLogSlot {
                slot: slot as u8,
                quest_id: quest.quest_entry,
                state,
                counts,
                timer: 0,
            }
        })
        .collect()
}

pub(super) fn item_template(t: crate::items::ItemTemplate) -> codec::ItemTemplateView {
    codec::ItemTemplateView {
        entry: t.entry,
        class: t.class,
        subclass: t.subclass,
        name: t.name,
        display_id: t.display_id,
        quality: t.quality,
        inventory_type: t.inventory_type,
        item_level: t.item_level,
        required_level: t.required_level,
        max_durability: t.max_durability,
        buy_price: t.buy_price,
        sell_price: t.sell_price,
        max_stack: t.max_stack,
        damage_min: t.damage_min,
        damage_max: t.damage_max,
        delay_ms: t.delay_ms,
        stat_strength: t.stat_strength,
        stat_agility: t.stat_agility,
        stat_stamina: t.stat_stamina,
        stat_intellect: t.stat_intellect,
        stat_spirit: t.stat_spirit,
        stat_armor: t.stat_armor,
        block_value: t.block_value,
        spellid_1: t.spellid_1,
        spelltrigger_1: t.spelltrigger_1,
        spellid_2: t.spellid_2,
        spelltrigger_2: t.spelltrigger_2,
        bonding: t.bonding,
        sheath: t.sheath,
        holy_res: t.holy_res,
        fire_res: t.fire_res,
        nature_res: t.nature_res,
        frost_res: t.frost_res,
        shadow_res: t.shadow_res,
        arcane_res: t.arcane_res,
        spellid_3: t.spellid_3,
        spelltrigger_3: t.spelltrigger_3,
        spellid_4: t.spellid_4,
        spelltrigger_4: t.spelltrigger_4,
        spellid_5: t.spellid_5,
        spelltrigger_5: t.spelltrigger_5,
        required_skill: t.required_skill,
        required_skill_rank: t.required_skill_rank,
        required_reputation_faction: t.required_reputation_faction,
        required_reputation_rank: t.required_reputation_rank,
        max_count: t.max_count,
        item_flags: t.item_flags,
        page_text: t.page_text,
        start_quest: t.start_quest,
        bag_family: t.bag_family,
        allowed_class: t.allowed_class,
        allowed_race: t.allowed_race,
        random_property: t.random_property,
    }
}

pub(super) fn sheet_stats(e: &WorldEntity) -> codec::SheetStatsValues {
    // Strength, agility, stamina and intellect are base values; spirit is already effective.
    codec::SheetStatsValues {
        strength: (i64::from(e.strength) + i64::from(e.sheet_str_bonus))
            .clamp(0, i64::from(i32::MAX)) as u32,
        agility: (i64::from(e.agility) + i64::from(e.sheet_agi_bonus)).clamp(0, i64::from(i32::MAX))
            as u32,
        stamina: (i64::from(e.stamina) + i64::from(e.sheet_sta_bonus)).clamp(0, i64::from(i32::MAX))
            as u32,
        intellect: (i64::from(e.intellect) + i64::from(e.sheet_int_bonus))
            .clamp(0, i64::from(i32::MAX)) as u32,
        spirit: e.spirit,
        str_bonus: e.sheet_str_bonus,
        agi_bonus: e.sheet_agi_bonus,
        sta_bonus: e.sheet_sta_bonus,
        int_bonus: e.sheet_int_bonus,
        spi_bonus: e.sheet_spi_bonus,
        attack_power: e.sheet_ap_base,
        ap_mods: e.sheet_ap_mods,
        dmg_min: e.sheet_dmg_min,
        dmg_max: e.sheet_dmg_max,
        ranged_attack_power: e.sheet_ranged_ap,
        ranged_dmg_min: e.sheet_ranged_dmg_min,
        ranged_dmg_max: e.sheet_ranged_dmg_max,
        // PLAYER_CRIT_PERCENTAGE uses percent; sheet_crit_bp uses basis points.
        crit_pct: e.sheet_crit_bp as f32 / 100.0,
    }
}
