use spacetimedb::ReducerContext;

/// The random stream a roll draws from; Roll Conformance seeds each stream per Character.
#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "conformance_test_fixture"), allow(dead_code))]
pub(crate) enum RollStream {
    Caster(u64),
    Target(u64),
    Swing(u64),
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

    #[table(accessor = game_conformance_spell_random)]
    pub struct SpellRandom {
        #[primary_key]
        pub character_guid: u64,
        #[unique]
        pub target_guid: u64,
        pub state: u64,
    }

    crate::outside_table_catalog!(game_conformance_swing_random, game_conformance_spell_random);

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
                let swings = ctx.db.game_conformance_swing_random();
                let mut row = swings.character_guid().find(guid)?;
                let draw = next(&mut row.state);
                swings.character_guid().update(row);
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
    pub fn debug_prepare_spell_roll_character(
        ctx: &ReducerContext,
        character_guid: u64,
        target_guid: u64,
        target_level: u32,
        seed: u64,
        stream: u64,
    ) -> Result<(), String> {
        use crate::{game_creature_ai_state, game_spell, game_spell_effect};

        crate::helpers::require_operator(ctx)?;
        if !matches!(target_level, 60 | 63) || !crate::time::is_virtual(ctx) {
            return Err("spell rolls need a virtual clock and target level 60 or 63".into());
        }
        let entities = ctx.db.game_world_entity();
        let mut character = entities
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
        if !matches!(target_level, 60 | 63) || !crate::time::is_virtual(ctx) {
            return Err("white melee needs a virtual clock and target level 60 or 63".into());
        }
        let entities = ctx.db.game_world_entity();
        let mut character = entities
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
