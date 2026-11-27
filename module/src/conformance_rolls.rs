use spacetimedb::ReducerContext;

/// The random stream a roll draws from; Roll Conformance seeds each stream per Character.
#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "conformance_test_fixture"), allow(dead_code))]
pub(crate) enum RollStream {
    Caster(u64),
    Target(u64),
    Swing(u64),
    Wear(u64),
}

/// Production draws `ctx.random`; the conformance fixture substitutes its seeded stream.
#[cfg_attr(not(feature = "conformance_test_fixture"), allow(unused_variables))]
pub(crate) fn draw(ctx: &ReducerContext, stream: RollStream) -> u32 {
    #[cfg(feature = "conformance_test_fixture")]
    if let Some(draw) = fixture::draw(ctx, stream) {
        return draw;
    }
    ctx.random::<u32>()
}

/// Lets the conformance fixture read the off-hand range a swing rolled.
#[cfg_attr(not(feature = "conformance_test_fixture"), allow(unused_variables))]
pub(crate) fn observe_offhand_range(ctx: &ReducerContext, range: (u32, u32)) {
    #[cfg(feature = "conformance_test_fixture")]
    fixture::record_offhand_range(ctx, range);
}

#[cfg(feature = "conformance_test_fixture")]
mod fixture {
    use spacetimedb::{reducer, table, ReducerContext, Table};

    use super::RollStream;
    use crate::{game_item_instance, game_world_entity};

    #[table(accessor = game_conformance_swing_random)]
    pub struct SwingRandom {
        #[primary_key]
        pub character_guid: u64,
        pub state: u64,
    }

    #[table(accessor = game_conformance_wear_random)]
    pub struct WearRandom {
        #[primary_key]
        pub character_guid: u64,
        pub state: u64,
    }

    #[table(accessor = game_conformance_spell_random)]
    pub struct SpellRandom {
        #[primary_key]
        pub character_guid: u64,
        #[unique]
        pub target_guid: u64,
        pub state: u64,
    }

    /// Exact swing draws, oldest id first, that a forced-outcome test takes before the seeded ones.
    #[table(accessor = game_conformance_scripted_swing_draw)]
    pub struct ScriptedSwingDraw {
        #[primary_key]
        #[auto_inc]
        pub id: u64,
        #[index(btree)]
        pub character_guid: u64,
        pub draw: u32,
    }

    crate::outside_table_catalog!(
        game_conformance_swing_random,
        game_conformance_spell_random,
        game_conformance_wear_random,
        game_conformance_scripted_swing_draw
    );

    fn next(state: &mut u64) -> u32 {
        *state = state.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = *state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        (value ^ (value >> 31)) as u32
    }

    fn next_spell(ctx: &ReducerContext, mut row: SpellRandom) -> u32 {
        let draw = next(&mut row.state);
        ctx.db
            .game_conformance_spell_random()
            .character_guid()
            .update(row);
        draw
    }

    pub(super) fn draw(ctx: &ReducerContext, stream: RollStream) -> Option<u32> {
        let spells = ctx.db.game_conformance_spell_random();
        match stream {
            RollStream::Caster(guid) => spells
                .character_guid()
                .find(guid)
                .map(|row| next_spell(ctx, row)),
            RollStream::Target(guid) => spells
                .target_guid()
                .find(guid)
                .map(|row| next_spell(ctx, row)),
            RollStream::Swing(guid) => {
                let scripted = ctx.db.game_conformance_scripted_swing_draw();
                if let Some(scripted_draw) = scripted
                    .character_guid()
                    .filter(guid)
                    .min_by_key(|row| row.id)
                {
                    scripted.id().delete(scripted_draw.id);
                    return Some(scripted_draw.draw);
                }
                let swings = ctx.db.game_conformance_swing_random();
                let mut row = swings.character_guid().find(guid)?;
                let draw = next(&mut row.state);
                swings.character_guid().update(row);
                Some(draw)
            }
            RollStream::Wear(guid) => {
                let wears = ctx.db.game_conformance_wear_random();
                let mut row = wears.character_guid().find(guid)?;
                let draw = next(&mut row.state);
                wears.character_guid().update(row);
                Some(draw)
            }
        }
    }

    pub(super) fn record_offhand_range(ctx: &ReducerContext, (base_min, base_max): (u32, u32)) {
        use crate::{game_debug_readout, DebugReadout};
        let readouts = ctx.db.game_debug_readout();
        let key = "offhand_roll".to_string();
        readouts.key().delete(&key);
        readouts.insert(DebugReadout {
            key,
            base_min,
            base_max,
            mitigation_pct: 0,
            final_min: 0,
            final_max: 0,
            note: String::new(),
            dodge_bp: 0,
            crit_bp: 0,
            hit_miss_bp: 0,
            parry_bp: 0,
            attack_time_ms: 0,
            spell_crit_bp: 0,
            spell_hit_normal: 0,
            spell_hit_crit: 0,
            damage_taken_pct: 0,
            block_bp: 0,
            block_value: 0,
            attacker_weapon_skill: 0,
            defender_defense_skill: 0,
        });
    }

    #[reducer]
    pub fn debug_script_swing_draws(
        ctx: &ReducerContext,
        character_guid: u64,
        draws: Vec<u32>,
    ) -> Result<(), String> {
        crate::helpers::require_operator(ctx)?;
        let scripted = ctx.db.game_conformance_scripted_swing_draw();
        for draw in draws {
            scripted.insert(ScriptedSwingDraw {
                id: 0,
                character_guid,
                draw,
            });
        }
        Ok(())
    }

    #[reducer]
    pub fn debug_prepare_spell_roll_character(
        ctx: &ReducerContext,
        character_guid: u64,
        target_guid: u64,
        target_level: u32,
        seed: u64,
        stream: u64,
    ) -> Result<(), String> {
        use crate::{game_spell, game_spell_effect};

        crate::helpers::require_operator(ctx)?;
        if !matches!(target_level, 60 | 63) || !crate::time::is_virtual(ctx) {
            return Err("spell rolls need a virtual clock and target level 60 or 63".into());
        }
        let entities = ctx.db.game_world_entity();
        let character = entities
            .guid()
            .find(character_guid)
            .ok_or("spell Character missing")?;
        let mut target = entities
            .guid()
            .find(target_guid)
            .ok_or("spell target missing")?;
        if !character.is_player()
            || target.is_player()
            || target.entry != 38
            || character.level != 60
        {
            return Err("invalid spell roll participants".into());
        }
        for item in ctx
            .db
            .game_item_instance()
            .by_owner_guid()
            .filter(&character_guid)
            .collect::<Vec<_>>()
        {
            crate::ItemInstance::delete(ctx, item.guid);
        }
        let mut character = entities
            .guid()
            .find(character_guid)
            .ok_or("spell Character missing")?;
        character.intellect = 125;
        character.health = 1_000_000_000;
        character.max_health = 1_000_000_000;
        character.power = 1_000_000_000;
        character.max_power = 1_000_000_000;
        character.update(ctx);
        target.level = target_level;
        target.health = 1_000_000_000;
        target.max_health = 1_000_000_000;
        target.orientation = std::f32::consts::PI;
        target.update(ctx);
        hold_passive(ctx, target_guid);

        let mut spell = crate::Spell::blank(2948, "Fixture spell 2948");
        spell.cost = 50;
        spell.cast_time_ms = 1500;
        spell.range_yd = 30;
        spell.school_mask = 4;
        spell.attributes = 65536;
        spell.spell_level = 22;
        spell.max_level = 26;
        spell.is_negative = true;
        spell.family_name = 3;
        spell.family_flags = 1073741840;
        spell.proc_chance = 101;
        spell.damage_class = 1;
        ctx.db.game_spell().spell_id().delete(2948);
        ctx.db.game_spell().insert(spell);
        let effects = ctx.db.game_spell_effect();
        for effect in effects.by_spell().filter(&2948u32).collect::<Vec<_>>() {
            effects.id().delete(effect.id);
        }
        let mut effect = crate::SpellEffect::blank(2948, 0);
        effect.kind = 1;
        effect.base_points = 53;
        effect.die_sides = 13;
        effect.per_level = 0.9;
        effect.target = 1;
        effect.bonus_coefficient = 0.429;
        effects.insert(effect);
        let learner = crate::helpers::live_entity(ctx, character_guid)?;
        crate::spell::learn_spell(ctx, character_guid, learner.owner_identity, 2948);
        let table = ctx.db.game_conformance_spell_random();
        table.character_guid().delete(character_guid);
        table.insert(SpellRandom {
            character_guid,
            target_guid,
            state: seed ^ stream.wrapping_mul(0xd1b54a32d192ed03),
        });
        Ok(())
    }

    #[reducer]
    pub fn debug_prepare_roll_character(
        ctx: &ReducerContext,
        character_guid: u64,
        target_guid: u64,
        target_level: u32,
        seed: u64,
        stream: u64,
    ) -> Result<(), String> {
        crate::helpers::require_operator(ctx)?;
        prepare_white_melee(ctx, character_guid, target_guid, target_level, seed, stream)
    }

    /// The white melee context plus Heroic Strike rank 1 and the axe it requires; ADR 0007.
    #[reducer]
    pub fn debug_prepare_queued_strike_character(
        ctx: &ReducerContext,
        character_guid: u64,
        target_guid: u64,
        target_level: u32,
        seed: u64,
        stream: u64,
    ) -> Result<(), String> {
        crate::helpers::require_operator(ctx)?;
        prepare_white_melee(ctx, character_guid, target_guid, target_level, seed, stream)?;
        equip_fixture_axe(ctx, character_guid)?;
        arm_heroic_strike(ctx, character_guid)
    }

    /// White melee plus the axe and Mortal Strike rank 1 on the caster stream; ADR 0007.
    #[reducer]
    pub fn debug_prepare_weapon_strike_character(
        ctx: &ReducerContext,
        character_guid: u64,
        target_guid: u64,
        target_level: u32,
        seed: u64,
        stream: u64,
    ) -> Result<(), String> {
        crate::helpers::require_operator(ctx)?;
        prepare_white_melee(ctx, character_guid, target_guid, target_level, seed, stream)?;
        equip_fixture_axe(ctx, character_guid)?;
        arm_mortal_strike(ctx, character_guid)?;
        let table = ctx.db.game_conformance_spell_random();
        table.character_guid().delete(character_guid);
        table.insert(SpellRandom {
            character_guid,
            target_guid,
            state: seed ^ stream.wrapping_mul(0xd1b54a32d192ed03),
        });
        Ok(())
    }

    /// White melee's warrior with Auto Shot, a gun and ammo, against a passive target; ADR 0007.
    #[reducer]
    pub fn debug_prepare_auto_shot_character(
        ctx: &ReducerContext,
        character_guid: u64,
        target_guid: u64,
        target_level: u32,
        seed: u64,
        stream: u64,
    ) -> Result<(), String> {
        crate::helpers::require_operator(ctx)?;
        prepare_white_melee(ctx, character_guid, target_guid, target_level, seed, stream)?;
        hold_passive(ctx, target_guid);
        // Items 2508 and 2516 translate pinned rows; ADR 0007 explains delay, durability and stack.
        let mut gun = crate::ItemTemplate::blank(2508, "Fixture gun 2508");
        gun.class = 2;
        gun.subclass = 3;
        gun.display_id = 6606;
        gun.quality = 1;
        gun.inventory_type = 26;
        gun.item_level = 2;
        gun.required_level = 1;
        gun.max_stack = 1;
        gun.damage_min = 2.0;
        gun.damage_max = 5.0;
        gun.delay_ms = 300;
        let mut ammo = crate::ItemTemplate::blank(2516, "Fixture ammo 2516");
        ammo.class = 6;
        ammo.subclass = 3;
        ammo.display_id = 5998;
        ammo.quality = 1;
        ammo.inventory_type = 24;
        ammo.item_level = 5;
        ammo.required_level = 1;
        ammo.max_stack = 10_000;
        ammo.damage_min = 1.0;
        ammo.damage_max = 2.0;
        ammo.bag_family = 2;
        carry(
            ctx,
            character_guid,
            gun,
            crate::items::equip_slot::RANGED,
            1,
        )?;
        carry(
            ctx,
            character_guid,
            ammo,
            lyracore_shared::constants::starter_item::BACKPACK_SLOT_0,
            10_000,
        )?;
        let mut spell = crate::Spell::blank(75, "Fixture spell 75");
        spell.range_yd = 35;
        spell.school_mask = 1;
        spell.attributes = 327698;
        spell.attributes_ex_2 = 32;
        spell.attributes_ex_3 = 32768;
        spell.spell_level = 1;
        spell.is_negative = true;
        spell.family_flags = 1;
        spell.proc_chance = 101;
        spell.equipped_item_class = 2;
        spell.equipped_item_subclass_mask = 262156;
        spell.damage_class = 3;
        arm_ranged_swing(ctx, character_guid, spell, 46)
    }

    /// A priest with a wand's Shoot and the white melee inputs, against a passive target; ADR 0007.
    #[reducer]
    pub fn debug_prepare_wand_shot_character(
        ctx: &ReducerContext,
        character_guid: u64,
        target_guid: u64,
        target_level: u32,
        seed: u64,
        stream: u64,
    ) -> Result<(), String> {
        use crate::stats::{class_level_key, game_crit_per_agility, CritPerAgility};

        crate::helpers::require_operator(ctx)?;
        prepare_white_melee(ctx, character_guid, target_guid, target_level, seed, stream)?;
        hold_passive(ctx, target_guid);
        // The pinned player_crit_per_agility row (5, 60, 20); curated seeds hold only warriors.
        let rates = ctx.db.game_crit_per_agility();
        rates.class_level().delete(class_level_key(5, 60));
        rates.insert(CritPerAgility {
            class_level: class_level_key(5, 60),
            class: 5,
            level: 60,
            rate: 20.0,
        });
        // Item 11287 translates its patch 7 row; ADR 0007 explains delay and durability.
        let mut wand = crate::ItemTemplate::blank(11287, "Fixture wand 11287");
        wand.class = 2;
        wand.subclass = 19;
        wand.display_id = 21096;
        wand.quality = 2;
        wand.inventory_type = 26;
        wand.item_level = 15;
        wand.required_level = 5;
        wand.max_stack = 1;
        wand.damage_min = 12.0;
        wand.damage_max = 22.0;
        wand.damage_school = 6;
        wand.delay_ms = 400;
        wand.bonding = 2;
        wand.item_flags = 32768;
        carry(
            ctx,
            character_guid,
            wand,
            crate::items::equip_slot::RANGED,
            1,
        )?;
        let mut spell = crate::Spell::blank(5019, "Fixture spell 5019");
        spell.range_yd = 30;
        spell.school_mask = 1;
        spell.attributes = 18;
        spell.attributes_ex_2 = 32;
        spell.attributes_ex_3 = 4227072;
        spell.category = 351;
        spell.spell_level = 1;
        spell.is_negative = true;
        spell.proc_chance = 101;
        spell.equipped_item_class = 2;
        spell.equipped_item_subclass_mask = 524288;
        spell.damage_class = 1;
        arm_ranged_swing(ctx, character_guid, spell, 228)
    }

    fn hold_passive(ctx: &ReducerContext, target_guid: u64) {
        use crate::game_creature_ai_state;

        let reactions = ctx.db.game_creature_ai_state();
        if let Some(mut reaction) = reactions.creature_guid().find(target_guid) {
            reaction.react_state = 0;
            reactions.creature_guid().update(reaction);
        } else {
            reactions.insert(crate::CreatureAiState {
                creature_guid: target_guid,
                phase: 0,
                lifecycle_id: crate::creatures::current_life_seq(ctx, target_guid),
                engagement_id: 1,
                ranged_distance: 0.0,
                ranged_angle: 0.0,
                ranged_posture_active: false,
                definition_revision: 0,
                active_object: false,
                react_state: 0,
                melee_enabled: true,
            });
        }
    }

    fn carry(
        ctx: &ReducerContext,
        character_guid: u64,
        template: crate::ItemTemplate,
        slot: u8,
        stack_count: u32,
    ) -> Result<(), String> {
        use crate::game_item_template;

        let entry = template.entry;
        ctx.db.game_item_template().entry().delete(entry);
        ctx.db.game_item_template().insert(template);
        let owner = crate::helpers::live_entity(ctx, character_guid)?;
        crate::ItemInstance::insert(
            ctx,
            crate::ItemInstance {
                guid: crate::items::next_item_guid(ctx)?,
                entry,
                owner_identity: owner.owner_identity,
                owner_guid: character_guid,
                slot,
                stack_count,
                durability: 0,
                created_at: crate::time::now(ctx),
                enchant_id: 0,
                soulbound: false,
                random_property_id: 0,
                item_text_id: 0,
            },
        );
        Ok(())
    }

    // The auto-repeat path reads only the spell header, so the fixture stores no effect rows.
    fn arm_ranged_swing(
        ctx: &ReducerContext,
        character_guid: u64,
        mut spell: crate::Spell,
        skill_line: u32,
    ) -> Result<(), String> {
        use crate::{game_spell, game_spell_effect};

        let spell_id = spell.spell_id;
        spell.cast_flags = lyracore_shared::spell::taxonomy::CAST_FLAG_RANGED_AUTO_REPEAT;
        ctx.db.game_spell().spell_id().delete(spell_id);
        ctx.db.game_spell().insert(spell);
        let effects = ctx.db.game_spell_effect();
        for effect in effects.by_spell().filter(&spell_id).collect::<Vec<_>>() {
            effects.id().delete(effect.id);
        }
        let learner = crate::helpers::live_entity(ctx, character_guid)?;
        crate::spell::learn_spell(ctx, character_guid, learner.owner_identity, spell_id);
        crate::skill::debug_set_skill(ctx, character_guid, skill_line, 300)
    }

    // Item 37 translates a pinned database row; ADR 0007 explains delay and durability.
    fn equip_fixture_axe(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
        use crate::game_item_template;

        let mut axe = crate::ItemTemplate::blank(37, "Fixture axe 37");
        axe.class = 2;
        axe.subclass = 0;
        axe.display_id = 14029;
        axe.quality = 1;
        axe.inventory_type = 21;
        axe.item_level = 2;
        axe.required_level = 1;
        axe.max_stack = 1;
        axe.damage_min = 1.0;
        axe.damage_max = 3.0;
        axe.delay_ms = 50;
        axe.sheath = 3;
        ctx.db.game_item_template().entry().delete(37);
        ctx.db.game_item_template().insert(axe);
        crate::debug::debug_equip_weapon(ctx, character_guid, 37)?;
        crate::skill::debug_set_skill(ctx, character_guid, 44, 300)
    }

    // Spell 78 translates a pinned database row.
    fn arm_heroic_strike(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
        use crate::{game_spell, game_spell_effect};

        let mut spell = crate::Spell::blank(78, "Fixture spell 78");
        spell.power_type = 1;
        spell.cost = 150;
        spell.gcd_ms = 0;
        spell.gcd_category = 0;
        spell.range_yd = 5;
        spell.school_mask = 1;
        spell.attributes = 327700;
        spell.attributes_ex_3 = 1024;
        spell.spell_level = 1;
        spell.is_negative = true;
        spell.cast_flags = crate::spell::CAST_FLAG_WEAPON_PROCS;
        spell.family_name = 4;
        spell.family_flags = 64;
        spell.proc_chance = 101;
        spell.custom_flags = lyracore_shared::spell_custom::FACE_TARGET;
        spell.visual_id = 39;
        spell.equipped_item_class = 2;
        spell.equipped_item_subclass_mask = 173555;
        spell.damage_class = 2;
        ctx.db.game_spell().spell_id().delete(78);
        ctx.db.game_spell().insert(spell);
        let effects = ctx.db.game_spell_effect();
        for effect in effects.by_spell().filter(&78u32).collect::<Vec<_>>() {
            effects.id().delete(effect.id);
        }
        let mut effect = crate::SpellEffect::blank(78, 0);
        effect.kind = crate::spell::E_NEXT_SWING;
        effect.base_points = 11;
        effect.die_sides = 1;
        effect.target = crate::spell::T_TARGET_ENEMY;
        effect.bonus_coefficient = 1.0;
        effects.insert(effect);
        let learner = crate::helpers::live_entity(ctx, character_guid)?;
        crate::spell::learn_spell(ctx, character_guid, learner.owner_identity, 78);
        crate::debug::debug_set_power(ctx, character_guid, 1_000_000_000)
    }

    // Spell 12294 translates a pinned database row; ADR 0007 explains the dropped cooldowns.
    fn arm_mortal_strike(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
        use crate::{game_spell, game_spell_effect};
        use lyracore_shared::spell::taxonomy::{A_INERT, CAST_FLAG_REFUSES_SELF_CAST, P_RAW};

        let mut spell = crate::Spell::blank(12294, "Fixture spell 12294");
        spell.power_type = 1;
        spell.cost = 300;
        spell.gcd_ms = 0;
        spell.gcd_category = 0;
        spell.category = 971;
        spell.range_yd = 5;
        spell.duration_ms = 10_000;
        spell.max_duration_ms = 10_000;
        spell.duration_index = 1;
        spell.school_mask = 1;
        spell.attributes = 327696;
        spell.attributes_ex_3 = 1024;
        spell.spell_level = 40;
        spell.is_negative = true;
        spell.cast_flags = crate::spell::CAST_FLAG_WEAPON_PROCS | CAST_FLAG_REFUSES_SELF_CAST;
        spell.family_name = 4;
        spell.family_flags = 33554432;
        spell.custom_flags = lyracore_shared::spell_custom::FACE_TARGET;
        spell.visual_id = 39;
        spell.equipped_item_class = 2;
        spell.equipped_item_subclass_mask = 173555;
        spell.damage_class = 2;
        ctx.db.game_spell().spell_id().delete(12294);
        ctx.db.game_spell().insert(spell);
        let effects = ctx.db.game_spell_effect();
        for effect in effects.by_spell().filter(&12294u32).collect::<Vec<_>>() {
            effects.id().delete(effect.id);
        }
        let mut healing = crate::SpellEffect::blank(12294, 0);
        healing.kind = A_INERT;
        healing.base_points = -50;
        healing.die_sides = 1;
        healing.target = crate::spell::T_TARGET_ENEMY;
        healing.bonus_coefficient = 0.0;
        effects.insert(healing);
        let mut strike = crate::SpellEffect::blank(12294, 1);
        strike.kind = crate::spell::E_WEAPON_STRIKE;
        strike.base_points = 85;
        strike.die_sides = 1;
        strike.target = crate::spell::T_TARGET_ENEMY;
        strike.p0 = 100;
        strike.p0_kind = P_RAW;
        strike.p1 = 1;
        strike.bonus_coefficient = 1.0;
        effects.insert(strike);
        let learner = crate::helpers::live_entity(ctx, character_guid)?;
        crate::spell::learn_spell(ctx, character_guid, learner.owner_identity, 12294);
        crate::debug::debug_set_power(ctx, character_guid, 1_000_000_000)
    }

    fn prepare_white_melee(
        ctx: &ReducerContext,
        character_guid: u64,
        target_guid: u64,
        target_level: u32,
        seed: u64,
        stream: u64,
    ) -> Result<(), String> {
        if !matches!(target_level, 60 | 63) || !crate::time::is_virtual(ctx) {
            return Err("white melee needs a virtual clock and target level 60 or 63".into());
        }
        let entities = ctx.db.game_world_entity();
        let character = entities
            .guid()
            .find(character_guid)
            .ok_or("roll Character missing")?;
        let mut target = entities
            .guid()
            .find(target_guid)
            .ok_or("roll target missing")?;
        if !character.is_player()
            || target.is_player()
            || target.entry != 38
            || character.level != 60
        {
            return Err("invalid white melee participants".into());
        }
        for item in ctx
            .db
            .game_item_instance()
            .by_owner_guid()
            .filter(&character_guid)
            .collect::<Vec<_>>()
        {
            crate::ItemInstance::delete(ctx, item.guid);
        }
        let mut character = entities
            .guid()
            .find(character_guid)
            .ok_or("roll Character missing")?;
        character.agility = 100;
        character.health = 1_000_000_000;
        character.max_health = 1_000_000_000;
        character.base_attack_time_ms = 50;
        crate::combat::queued_strike::forget(&mut character);
        character.update(ctx);
        target.level = target_level;
        target.health = 1_000_000_000;
        target.max_health = 1_000_000_000;
        target.orientation = std::f32::consts::PI;
        target.update(ctx);
        crate::skill::debug_set_skill(ctx, character_guid, 162, 300)?;
        crate::skill::debug_set_skill(ctx, character_guid, 95, 300)?;
        ctx.db
            .game_conformance_scripted_swing_draw()
            .character_guid()
            .delete(character_guid);
        let table = ctx.db.game_conformance_swing_random();
        table.character_guid().delete(character_guid);
        table.insert(SwingRandom {
            character_guid,
            state: seed ^ stream.wrapping_mul(0xd1b54a32d192ed03),
        });
        Ok(())
    }

    #[reducer]
    pub fn debug_advance_roll_batch(ctx: &ReducerContext, ticks: u32) -> Result<(), String> {
        crate::helpers::require_operator(ctx)?;
        if !(1..=4000).contains(&ticks) {
            return Err("roll batch needs 1 through 4000 scheduler ticks".into());
        }
        crate::time::advance_virtual_clock(ctx, i64::from(ticks) * 50_000)
    }
}
