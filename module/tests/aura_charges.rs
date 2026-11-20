mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_ABSORB, A_FLAG, A_IMMUNITY, A_INERT, A_MOD_COMBAT, A_MOD_DAMAGE_TAKEN, A_MOD_RESISTANCE,
    A_MOD_SPEED, A_SPELLMOD_FLAT, CAST_FLAG_CANNOT_CRIT, CAST_FLAG_IGNORE_RESISTANCES,
    COMBAT_ATTACK_POWER, COMBAT_CRIT, COMBAT_SPELL_CRIT, E_DAMAGE, P_COMBAT_FIELD, P_MECHANIC,
    P_NONE, P_SCHOOL_MASK, P_SPEED_KIND, P_SPELLMOD_OP, RESIST_ARMOR, SPEED_CAST, SPEED_SWING,
    T_SELF, T_TARGET_ENEMY,
};
use support::{event_rows, fence_subscription, subscribe_with_fence, Standalone};

const PLAYER: u64 = 1;
const PLAYER_HEALTH: u32 = 10_000;
const WOLF_HEALTH: u32 = 100_000;
const OUT_OF_REACH_LEVEL: u32 = 60;
const DURATION_MS: u32 = 600_000;
const PHYSICAL: u8 = 0x01;
const FIRE: u8 = 0x04;
const NATURE: u8 = 0x08;

// vmangos SpellDefines.h ProcFlags and ProcFlagsEx.
const DEAL_MELEE_SWING: u32 = 0x4;
const TAKE_MELEE_SWING: u32 = 0x8;
const DEAL_HARMFUL_SPELL: u32 = 0x1_0000;
const TAKE_HARMFUL_SPELL: u32 = 0x2_0000;
const TAKE_HARMFUL_ABILITY: u32 = 0x2000;
const PROC_EX_NORMAL_HIT: u32 = 0x1;
const PROC_EX_BLOCK: u32 = 0x40;
const PROC_EX_TRIGGER_ALWAYS: u32 = 0x1_0000;
// Spell.dbc attribute bit 0, SPELL_ATTR_PROC_FAILURE_BURNS_CHARGE.
const PROC_FAILURE_BURNS_CHARGE: u32 = 0x1;
// vmangos SpellModOp SPELLMOD_CHARGES and SharedDefines.h MECHANIC_FEAR.
const SPELLMOD_CHARGES: i32 = 4;
const MECHANIC_FEAR: u8 = 5;
// vmangos Unit::HandleHasteAuraProc names Flurry by this SpellVisual.
const FLURRY_VISUAL: u32 = 2759;
const SPELL_FAMILY_WARRIOR: u8 = 4;
const SHIELD_BLOCK_FAMILY_FLAG: i32 = 0x1000;
// AttackOutcome::wire codes in game_combat_event_live.hit_info.
const HIT_INFO_CRIT: &str = "1";
const HIT_INFO_AVOIDED: [&str; 3] = ["2", "3", "4"];
const ATTEMPTS: usize = 20;
const BLOCK_ATTEMPTS: usize = 120;

// Spell ids the Module names, as vmangos spell_mage.cpp and HandleModPowerCostSchoolAuraProc do.
const COMBUSTION: u32 = 11129;
const COMBUSTION_CRIT_BUFF: u32 = 28682;
const INSIGHT: u32 = 18820;

const FLURRY: u32 = 5_091_100;
const INNER_FIRE: u32 = 5_091_101;
const STORMSTRIKE: u32 = 5_091_102;
const NATURE_BOLT: u32 = 5_091_103;
const FIRE_BOLT: u32 = 5_091_104;
const SCORCH: u32 = 5_091_105;
const CERTAIN_SPELL_CRIT: u32 = 5_091_106;
const FEAR_WARD: u32 = 5_091_107;
const FEAR_BOLT: u32 = 5_091_108;
const FOCUS: u32 = 5_091_109;
const SLOW_BOLT: u32 = 5_091_110;
const COSTLY_BOLT: u32 = 5_091_111;
const CERTAIN_CRIT: u32 = 5_091_112;
const SHIELD_BLOCK: u32 = 5_091_113;
const IMPROVED_SHIELD_BLOCK: u32 = 5_091_115;
const WINDFURY_ATTACK: u32 = 5_091_116;
const POWER_WORD_SHIELD: u32 = 5_091_117;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn flurry_spends_a_charge_on_each_of_three_swings_hit_or_miss() {
    let mut standalone = Standalone::start("aura-charges-flurry");
    let wolf = stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: FLURRY,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: DEAL_MELEE_SWING,
            charges: 3,
        },
        &[(A_MOD_SPEED, 30, i32::from(SPEED_SWING), P_SPEED_KIND)],
    );
    standalone.insert_row(
        "game_spell_proc_event",
        &[("spell_id", &FLURRY), ("proc_ex", &PROC_EX_TRIGGER_ALWAYS)],
    );
    standalone.assert_call("debug_force_cast", &["1", &FLURRY.to_string()]);
    standalone.insert_row(
        "game_conformance_swing_random",
        &[("character_guid", &1), ("state", &1)],
    );
    let events = subscribe_with_fence(
        &standalone,
        &["SELECT * FROM game_combat_event_live WHERE attacker_guid = 1"],
    );
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);

    set_level(&standalone, wolf, OUT_OF_REACH_LEVEL);
    swing(&standalone);
    fence_subscription(&standalone, &events);
    let swings = event_rows(&events, "game_combat_event_live");
    assert_eq!(swings.len(), 1);
    assert_eq!(swings[0]["hit_info"], "2", "the first swing misses");
    assert_eq!(
        charges(&standalone, PLAYER, FLURRY),
        Some(2),
        "vmangos procEx TRIGGER_ALWAYS spends Flurry on a missed swing"
    );

    set_level(&standalone, wolf, 1);
    swing(&standalone);
    assert_eq!(charges(&standalone, PLAYER, FLURRY), Some(1));
    swing(&standalone);
    fence_subscription(&standalone, &events);
    assert_eq!(event_rows(&events, "game_combat_event_live").len(), 3);
    assert_eq!(
        charges(&standalone, PLAYER, FLURRY),
        None,
        "the third swing spends the last charge and removes Flurry"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn inner_fire_spends_a_charge_on_each_hit_taken_and_goes_after_twenty() {
    let mut standalone = Standalone::start("aura-charges-inner-fire");
    let wolf = stage(&mut standalone);
    // The wolf the Character strikes would otherwise start swinging on the real-time melee tick.
    standalone.clear_schedule("game_melee_schedule");
    aura_spell(
        &standalone,
        Charged {
            spell_id: INNER_FIRE,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: TAKE_MELEE_SWING,
            charges: 20,
        },
        &[(
            A_MOD_RESISTANCE,
            315,
            i32::from(RESIST_ARMOR),
            P_SCHOOL_MASK,
        )],
    );
    standalone.assert_call("debug_force_cast", &["1", &INNER_FIRE.to_string()]);
    advance(&standalone);

    standalone.assert_call(
        "debug_apply_damage",
        &[&wolf.to_string(), "1", &PLAYER.to_string()],
    );
    assert_eq!(
        charges(&standalone, PLAYER, INNER_FIRE),
        Some(20),
        "a hit Inner Fire's bearer deals spends nothing"
    );
    for remaining in (1..20).rev() {
        hit_player(&standalone, wolf);
        assert_eq!(charges(&standalone, PLAYER, INNER_FIRE), Some(remaining));
    }
    hit_player(&standalone, wolf);
    assert_eq!(
        charges(&standalone, PLAYER, INNER_FIRE),
        None,
        "the twentieth hit taken removes Inner Fire"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn stormstrike_spends_a_charge_on_each_nature_hit_and_not_on_a_swing() {
    let mut standalone = Standalone::start("aura-charges-stormstrike");
    let wolf = stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: STORMSTRIKE,
            school_mask: NATURE,
            target: T_TARGET_ENEMY,
            proc_flags: TAKE_MELEE_SWING | TAKE_HARMFUL_SPELL,
            charges: 2,
        },
        &[(A_MOD_DAMAGE_TAKEN, 20, i32::from(NATURE), P_SCHOOL_MASK)],
    );
    standalone.insert_row(
        "game_spell_proc_event",
        &[("spell_id", &STORMSTRIKE), ("school_mask", &NATURE)],
    );
    bolt(&standalone, NATURE_BOLT, NATURE, 0);
    standalone.assert_call(
        "debug_force_cast_at",
        &["1", &STORMSTRIKE.to_string(), &wolf.to_string()],
    );
    advance(&standalone);

    standalone.assert_call(
        "debug_apply_damage",
        &[&wolf.to_string(), "1", &PLAYER.to_string()],
    );
    assert_eq!(
        charges(&standalone, wolf, STORMSTRIKE),
        Some(2),
        "a physical hit spends no Stormstrike charge"
    );
    cast(&standalone, NATURE_BOLT, wolf);
    assert_eq!(charges(&standalone, wolf, STORMSTRIKE), Some(1));
    cast(&standalone, NATURE_BOLT, wolf);
    assert_eq!(
        charges(&standalone, wolf, STORMSTRIKE),
        None,
        "the second nature hit removes Stormstrike"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn combustion_stacks_on_fire_hits_and_spends_its_charges_on_fire_crits() {
    let mut standalone = Standalone::start("aura-charges-combustion");
    let wolf = stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: COMBUSTION,
            school_mask: FIRE,
            target: T_SELF,
            proc_flags: DEAL_HARMFUL_SPELL,
            charges: 3,
        },
        &[(A_FLAG, 0, 0, P_NONE)],
    );
    standalone.insert_row(
        "game_spell_proc_event",
        &[("spell_id", &COMBUSTION), ("school_mask", &FIRE)],
    );
    aura_spell(
        &standalone,
        Charged {
            spell_id: COMBUSTION_CRIT_BUFF,
            school_mask: FIRE,
            target: T_SELF,
            proc_flags: 0,
            charges: 0,
        },
        &[(A_SPELLMOD_FLAT, 10, 7, P_SPELLMOD_OP)],
    );
    standalone.assert_sql(&format!(
        "UPDATE game_spell SET max_stacks = 10 WHERE spell_id = {COMBUSTION_CRIT_BUFF}"
    ));
    aura_spell(
        &standalone,
        Charged {
            spell_id: CERTAIN_SPELL_CRIT,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: 0,
            charges: 0,
        },
        &[(
            A_MOD_COMBAT,
            10_000,
            i32::from(COMBAT_SPELL_CRIT),
            P_COMBAT_FIELD,
        )],
    );
    bolt(&standalone, FIRE_BOLT, FIRE, 0);
    bolt(&standalone, SCORCH, FIRE, CAST_FLAG_CANNOT_CRIT);
    for spell in [CERTAIN_SPELL_CRIT, COMBUSTION, COMBUSTION_CRIT_BUFF] {
        standalone.assert_call("debug_force_cast", &["1", &spell.to_string()]);
    }
    advance(&standalone);

    cast(&standalone, SCORCH, wolf);
    assert_eq!(
        (
            charges(&standalone, PLAYER, COMBUSTION),
            stacks(&standalone, COMBUSTION_CRIT_BUFF)
        ),
        (Some(3), Some(2)),
        "a fire hit that does not crit stacks the crit buff and spends nothing"
    );
    cast(&standalone, FIRE_BOLT, wolf);
    assert_eq!(
        (
            charges(&standalone, PLAYER, COMBUSTION),
            stacks(&standalone, COMBUSTION_CRIT_BUFF)
        ),
        (Some(2), Some(3))
    );
    cast(&standalone, FIRE_BOLT, wolf);
    assert_eq!(charges(&standalone, PLAYER, COMBUSTION), Some(1));
    cast(&standalone, FIRE_BOLT, wolf);
    assert_eq!(
        (
            charges(&standalone, PLAYER, COMBUSTION),
            stacks(&standalone, COMBUSTION_CRIT_BUFF)
        ),
        (None, None),
        "the third crit removes Combustion and its crit buff"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn fear_ward_is_kept_by_a_frostbolt_and_spent_by_a_fear_spell() {
    let mut standalone = Standalone::start("aura-charges-fear-ward");
    let wolf = stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: FEAR_WARD,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: TAKE_HARMFUL_ABILITY | TAKE_HARMFUL_SPELL,
            charges: 1,
        },
        &[(A_IMMUNITY, 0, i32::from(MECHANIC_FEAR), P_MECHANIC)],
    );
    standalone.insert_row(
        "game_spell_proc_event",
        &[("spell_id", &FEAR_WARD), ("proc_ex", &PROC_EX_NORMAL_HIT)],
    );
    bolt(&standalone, NATURE_BOLT, NATURE, 0);
    bolt(&standalone, FEAR_BOLT, NATURE, 0);
    set_spell(
        &standalone,
        FEAR_BOLT,
        &format!("mechanic = {MECHANIC_FEAR}"),
    );
    standalone.assert_call("debug_force_cast", &["1", &FEAR_WARD.to_string()]);
    advance(&standalone);

    cast_by(&standalone, wolf, NATURE_BOLT, PLAYER);
    assert_eq!(
        charges(&standalone, PLAYER, FEAR_WARD),
        Some(1),
        "vmangos HandleMechanicImmuneResistanceAuraProc keeps Fear Ward off a spell of another mechanic"
    );
    cast_by(&standalone, wolf, FEAR_BOLT, PLAYER);
    assert_eq!(charges(&standalone, PLAYER, FEAR_WARD), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn focus_is_kept_by_an_instant_spell_and_spent_by_one_with_a_cast_time() {
    let mut standalone = Standalone::start("aura-charges-focus");
    let wolf = stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: FOCUS,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: DEAL_HARMFUL_SPELL,
            charges: 1,
        },
        &[(A_MOD_SPEED, 100, i32::from(SPEED_CAST), P_SPEED_KIND)],
    );
    bolt(&standalone, FIRE_BOLT, FIRE, 0);
    bolt(&standalone, SLOW_BOLT, FIRE, 0);
    set_spell(&standalone, SLOW_BOLT, "cast_time_ms = 1500");
    standalone.assert_call("debug_force_cast", &["1", &FOCUS.to_string()]);
    advance(&standalone);

    cast(&standalone, FIRE_BOLT, wolf);
    assert_eq!(
        charges(&standalone, PLAYER, FOCUS),
        Some(1),
        "vmangos HandleModCastingSpeedNotStackAuraProc skips an instant spell"
    );
    cast(&standalone, SLOW_BOLT, wolf);
    assert_eq!(charges(&standalone, PLAYER, FOCUS), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn insight_is_kept_by_a_free_spell_and_spent_by_one_that_costs_power() {
    let mut standalone = Standalone::start("aura-charges-insight");
    let wolf = stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: INSIGHT,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: DEAL_HARMFUL_SPELL,
            charges: 1,
        },
        &[(A_INERT, 0, 0, P_NONE)],
    );
    bolt(&standalone, FIRE_BOLT, FIRE, 0);
    bolt(&standalone, COSTLY_BOLT, FIRE, 0);
    set_spell(&standalone, COSTLY_BOLT, "cost = 10");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET power = 1000, max_power = 1000 WHERE guid = {PLAYER}"
    ));
    standalone.assert_call("debug_force_cast", &["1", &INSIGHT.to_string()]);
    advance(&standalone);

    cast(&standalone, FIRE_BOLT, wolf);
    assert_eq!(
        charges(&standalone, PLAYER, INSIGHT),
        Some(1),
        "vmangos HandleModPowerCostSchoolAuraProc skips a spell with no cost"
    );
    cast(&standalone, COSTLY_BOLT, wolf);
    assert_eq!(charges(&standalone, PLAYER, INSIGHT), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn flurry_keeps_its_last_charge_on_a_crit() {
    let mut standalone = Standalone::start("aura-charges-flurry-crit");
    let wolf = stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: FLURRY,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: DEAL_MELEE_SWING,
            charges: 3,
        },
        &[(A_MOD_SPEED, 30, i32::from(SPEED_SWING), P_SPEED_KIND)],
    );
    set_spell(&standalone, FLURRY, &format!("visual_id = {FLURRY_VISUAL}"));
    aura_spell(
        &standalone,
        Charged {
            spell_id: CERTAIN_CRIT,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: 0,
            charges: 0,
        },
        &[(A_MOD_COMBAT, 10_000, i32::from(COMBAT_CRIT), P_COMBAT_FIELD)],
    );
    standalone.assert_call("debug_force_cast", &["1", &CERTAIN_CRIT.to_string()]);
    standalone.insert_row(
        "game_conformance_swing_random",
        &[("character_guid", &1), ("state", &1)],
    );
    let events = subscribe_with_fence(
        &standalone,
        &["SELECT * FROM game_combat_event_live WHERE attacker_guid = 1"],
    );
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);

    for attempt in 0..ATTEMPTS {
        standalone.assert_call("debug_force_cast", &["1", &FLURRY.to_string()]);
        standalone.assert_sql(&format!(
            "UPDATE game_aura SET proc_charges = 1 WHERE spell_id = {FLURRY}"
        ));
        swing(&standalone);
        fence_subscription(&standalone, &events);
        let swings = event_rows(&events, "game_combat_event_live");
        assert_eq!(swings.len(), attempt + 1);
        if swings[attempt]["hit_info"] == HIT_INFO_CRIT {
            assert_eq!(
                charges(&standalone, PLAYER, FLURRY),
                Some(1),
                "vmangos HandleHasteAuraProc keeps Flurry's last charge on a crit"
            );
            return;
        }
    }
    panic!("no swing crit");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_full_block_spends_a_charge_that_blocks_spend() {
    let mut standalone = Standalone::start("aura-charges-full-block");
    let wolf = stage(&mut standalone);
    // A wolf facing the Character can block, and its block value covers any swing.
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 0, y = 0, orientation = 0 WHERE guid = {PLAYER}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 1, y = 0, orientation = 3.1415927, \
         strength = 100000 WHERE guid = {wolf}"
    ));
    // At level 60 with full skills, the creature block chance is its 5% cap.
    set_level(&standalone, PLAYER, OUT_OF_REACH_LEVEL);
    set_level(&standalone, wolf, OUT_OF_REACH_LEVEL);
    for skill_line in ["43", "95", "162"] {
        standalone.assert_call("debug_set_skill", &["1", skill_line, "300"]);
    }
    standalone.insert_row(
        "game_spell_proc_event",
        &[("spell_id", &SHIELD_BLOCK), ("proc_ex", &PROC_EX_BLOCK)],
    );
    aura_spell(
        &standalone,
        Charged {
            spell_id: SHIELD_BLOCK,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: TAKE_MELEE_SWING,
            charges: 3,
        },
        &[(A_INERT, 0, 0, P_NONE)],
    );
    standalone.assert_call(
        "debug_force_cast_at",
        &[
            &wolf.to_string(),
            &SHIELD_BLOCK.to_string(),
            &wolf.to_string(),
        ],
    );
    standalone.insert_row(
        "game_conformance_swing_random",
        &[("character_guid", &1), ("state", &1)],
    );
    let events = subscribe_with_fence(
        &standalone,
        &["SELECT * FROM game_combat_event_live WHERE attacker_guid = 1"],
    );
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);

    for attempt in 0..BLOCK_ATTEMPTS {
        swing(&standalone);
        fence_subscription(&standalone, &events);
        let swings = event_rows(&events, "game_combat_event_live");
        assert_eq!(swings.len(), attempt + 1);
        let swing = &swings[attempt];
        // A blocked crit keeps its crit outcome, so read the blocked amount.
        if swing["blocked_amount"] != "0" {
            assert_eq!(swing["damage"], "0", "the wolf blocks the whole swing");
            assert_eq!(
                charges(&standalone, wolf, SHIELD_BLOCK),
                Some(2),
                "vmangos raises PROC_EX_BLOCK for a full block"
            );
            return;
        }
        assert_eq!(charges(&standalone, wolf, SHIELD_BLOCK), Some(3));
    }
    panic!("the wolf never blocked");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn improved_shield_block_adds_a_charge_to_shield_block() {
    let mut standalone = Standalone::start("aura-charges-charge-modifier");
    stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: SHIELD_BLOCK,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: TAKE_MELEE_SWING,
            charges: 1,
        },
        &[(A_INERT, 0, 0, P_NONE)],
    );
    aura_spell(
        &standalone,
        Charged {
            spell_id: IMPROVED_SHIELD_BLOCK,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: 0,
            charges: 0,
        },
        &[(A_SPELLMOD_FLAT, 1, SPELLMOD_CHARGES, P_SPELLMOD_OP)],
    );
    for spell in [SHIELD_BLOCK, IMPROVED_SHIELD_BLOCK] {
        set_spell(
            &standalone,
            spell,
            &format!(
                "family_name = {SPELL_FAMILY_WARRIOR}, family_flags = {SHIELD_BLOCK_FAMILY_FLAG}"
            ),
        );
    }
    standalone.assert_sql(&format!(
        "UPDATE game_spell_effect SET p_1 = {SHIELD_BLOCK_FAMILY_FLAG} \
         WHERE spell_id = {IMPROVED_SHIELD_BLOCK}"
    ));
    standalone.assert_call(
        "debug_force_cast",
        &["1", &IMPROVED_SHIELD_BLOCK.to_string()],
    );
    standalone.assert_call("debug_force_cast", &["1", &SHIELD_BLOCK.to_string()]);
    assert_eq!(
        charges(&standalone, PLAYER, SHIELD_BLOCK),
        Some(2),
        "vmangos SpellAuraHolder applies the caster's SPELLMOD_CHARGES to the base charges"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_spell_that_burns_charges_on_failure_spends_none_on_a_counted_swing() {
    let mut standalone = Standalone::start("aura-charges-burns-on-failure");
    let wolf = stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: WINDFURY_ATTACK,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: DEAL_MELEE_SWING,
            charges: 3,
        },
        &[(
            A_MOD_COMBAT,
            315,
            i32::from(COMBAT_ATTACK_POWER),
            P_COMBAT_FIELD,
        )],
    );
    set_spell(
        &standalone,
        WINDFURY_ATTACK,
        &format!("attributes = {PROC_FAILURE_BURNS_CHARGE}"),
    );
    standalone.assert_call("debug_force_cast", &["1", &WINDFURY_ATTACK.to_string()]);
    advance(&standalone);

    standalone.assert_call(
        "debug_apply_damage",
        &[&wolf.to_string(), "1", &PLAYER.to_string()],
    );
    assert_eq!(
        charges(&standalone, PLAYER, WINDFURY_ATTACK),
        Some(3),
        "vmangos HandleTriggers spends no charge on SPELL_ATTR_PROC_FAILURE_BURNS_CHARGE and OK"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_swing_that_an_absorb_takes_whole_still_spends_flurry() {
    const CHARGES: u8 = 20;
    let mut standalone = Standalone::start("aura-charges-absorbed-swing");
    let wolf = stage(&mut standalone);
    // Behind a wolf facing away, a swing is neither parried nor blocked.
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 0, y = 0, orientation = 0 WHERE guid = {PLAYER}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 1, y = 0, orientation = 0 WHERE guid = {wolf}"
    ));
    aura_spell(
        &standalone,
        Charged {
            spell_id: POWER_WORD_SHIELD,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: 0,
            charges: 0,
        },
        &[(A_ABSORB, 100_000, i32::from(PHYSICAL), P_SCHOOL_MASK)],
    );
    standalone.assert_call(
        "debug_force_cast_at",
        &[
            &wolf.to_string(),
            &POWER_WORD_SHIELD.to_string(),
            &wolf.to_string(),
        ],
    );
    standalone.insert_row(
        "game_spell_proc_event",
        &[("spell_id", &FLURRY), ("proc_ex", &PROC_EX_TRIGGER_ALWAYS)],
    );
    aura_spell(
        &standalone,
        Charged {
            spell_id: FLURRY,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: DEAL_MELEE_SWING,
            charges: CHARGES,
        },
        &[(A_MOD_SPEED, 30, i32::from(SPEED_SWING), P_SPEED_KIND)],
    );
    standalone.assert_call("debug_force_cast", &["1", &FLURRY.to_string()]);
    standalone.insert_row(
        "game_conformance_swing_random",
        &[("character_guid", &1), ("state", &1)],
    );
    let events = subscribe_with_fence(
        &standalone,
        &["SELECT * FROM game_combat_event_live WHERE attacker_guid = 1"],
    );
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);

    for swung in 1..=ATTEMPTS {
        swing(&standalone);
        fence_subscription(&standalone, &events);
        let swings = event_rows(&events, "game_combat_event_live");
        assert_eq!(swings.len(), swung);
        assert_eq!(
            charges(&standalone, PLAYER, FLURRY),
            Some(CHARGES - swung as u8),
            "vmangos runs the proc pass for a swing an absorb takes, with PROC_EX_ABSORB"
        );
        let last = &swings[swung - 1];
        if !HIT_INFO_AVOIDED.contains(&last["hit_info"].as_str()) {
            assert_eq!(last["damage"], "0", "the absorb takes the whole swing");
            return;
        }
    }
    panic!("no swing connected");
}

struct Charged {
    spell_id: u32,
    school_mask: u8,
    target: u8,
    proc_flags: u32,
    charges: u8,
}

fn stage(standalone: &mut Standalone) -> u64 {
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {PLAYER_HEALTH}, max_health = {PLAYER_HEALTH} \
         WHERE guid = {PLAYER}"
    ));
    standalone.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "1.0"],
    );
    let wolf = standalone
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, level = 1, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    wolf
}

fn aura_spell(standalone: &Standalone, spell: Charged, effects: &[(u8, i32, i32, u8)]) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell.spell_id),
            ("name", &format!("Charged fixture {}", spell.spell_id)),
            ("school_mask", &spell.school_mask),
            ("range_yd", &30),
            ("is_negative", &(spell.target != T_SELF)),
            ("duration_ms", &DURATION_MS),
            ("spell_level", &1),
            ("proc_flags", &spell.proc_flags),
            ("proc_chance", &100),
            ("proc_charges", &spell.charges),
        ],
    );
    for (index, &(kind, base_points, p0, p0_kind)) in effects.iter().enumerate() {
        standalone.insert_row(
            "game_spell_effect",
            &[
                ("id", &packed_spell_effect_id(spell.spell_id, index as u8)),
                ("spell_id", &spell.spell_id),
                ("effect_index", &(index as u8)),
                ("kind", &kind),
                ("base_points", &base_points),
                ("target", &spell.target),
                ("p_0", &p0),
                ("p_0_kind", &p0_kind),
            ],
        );
    }
}

fn bolt(standalone: &Standalone, spell_id: u32, school_mask: u8, cast_flags: u32) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &format!("Bolt fixture {spell_id}")),
            ("school_mask", &school_mask),
            ("range_yd", &30),
            ("is_negative", &true),
            ("cast_flags", &(cast_flags | CAST_FLAG_IGNORE_RESISTANCES)),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, 0)),
            ("spell_id", &spell_id),
            ("kind", &E_DAMAGE),
            ("base_points", &5),
            ("target", &T_TARGET_ENEMY),
        ],
    );
}

// A proc never fires off an Aura applied in the same virtual instant.
fn advance(standalone: &Standalone) {
    standalone.assert_call("debug_advance_virtual_clock", &["50000"]);
}

fn cast(standalone: &Standalone, spell_id: u32, target: u64) {
    standalone.assert_call(
        "debug_cast_at",
        &["1", &spell_id.to_string(), &target.to_string()],
    );
    advance(standalone);
}

fn cast_by(standalone: &Standalone, caster: u64, spell_id: u32, target: u64) {
    standalone.assert_call(
        "debug_cast_at",
        &[
            &caster.to_string(),
            &spell_id.to_string(),
            &target.to_string(),
        ],
    );
    advance(standalone);
}

fn set_spell(standalone: &Standalone, spell_id: u32, assignments: &str) {
    standalone.assert_sql(&format!(
        "UPDATE game_spell SET {assignments} WHERE spell_id = {spell_id}"
    ));
}

fn hit_player(standalone: &Standalone, wolf: u64) {
    standalone.assert_call(
        "debug_apply_damage",
        &[&PLAYER.to_string(), "1", &wolf.to_string()],
    );
    advance(standalone);
}

fn swing(standalone: &Standalone) {
    standalone.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
    standalone.assert_call("debug_advance_roll_batch", &["4"]);
}

fn set_level(standalone: &Standalone, guid: u64, level: u32) {
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET level = {level} WHERE guid = {guid}"
    ));
}

fn aura_column(standalone: &Standalone, carrier: u64, spell_id: u32, column: &str) -> Option<u8> {
    standalone
        .query_rows(&format!(
            "SELECT {column} FROM game_aura WHERE target_guid = {carrier} AND spell_id = {spell_id}"
        ))
        .first()
        .map(|row| row[column].parse().unwrap())
}

fn charges(standalone: &Standalone, carrier: u64, spell_id: u32) -> Option<u8> {
    aura_column(standalone, carrier, spell_id, "proc_charges")
}

fn stacks(standalone: &Standalone, spell_id: u32) -> Option<u8> {
    aura_column(standalone, PLAYER, spell_id, "stacks")
}
