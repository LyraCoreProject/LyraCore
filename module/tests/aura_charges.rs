mod support;

use lyracore_package_delta::fixture_ids::{
    FIXTURE_BLADE, LINEN_BANDAGE_SPELL, TEST_POWER_WORD_SHIELD_SPELL, TEST_WOLF_ENTRY,
};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_protocol::codec::values_mask::parse_values_updates;
use lyracore_shared::spell::taxonomy::{
    A_ABSORB, A_CONTROL, A_FLAG, A_IMMUNITY, A_INERT, A_MOD_COMBAT, A_MOD_DAMAGE_TAKEN,
    A_MOD_POWER_COST_FLAT, A_MOD_POWER_COST_PCT, A_MOD_RESISTANCE, A_MOD_SPEED, A_PERIODIC_DAMAGE,
    A_PROC_TRIGGER, A_SPELLMOD_FLAT, A_SPELLMOD_PCT, CAST_FLAG_ALWAYS_HIT, CAST_FLAG_CANNOT_CRIT,
    CAST_FLAG_IGNORE_RESISTANCES, COMBAT_ATTACK_POWER, COMBAT_CRIT, COMBAT_SPELL_CRIT, E_DAMAGE,
    E_HEAL, E_INTERRUPT, E_TRIGGER, E_WEAPON_STRIKE, M_FEAR, M_POLY, M_STUN, P_COMBAT_FIELD,
    P_MECHANIC, P_NONE, P_RAW, P_SCHOOL_MASK, P_SPEED_KIND, P_SPELLMOD_OP, RESIST_ARMOR,
    SPEED_CAST, SPEED_MOVE, SPEED_SWING, SPELLMOD_OP_CASTING_TIME, SPELLMOD_OP_COST,
    SPELLMOD_OP_CRITICAL_CHANCE, T_SELF, T_TARGET_ALLY, T_TARGET_ANY, T_TARGET_ENEMY,
};
use lyracore_test_support::{NativePacket, ReducerConnection};
use serde_json::json;
use std::time::{Duration, Instant};
use support::{event_rows, fence_subscription, subscribe_with_fence, Standalone};

const PLAYER: u64 = 1;
const PLAYER_HEALTH: u32 = 10_000;
const WOLF_HEALTH: u32 = 100_000;
const OUT_OF_REACH_LEVEL: u32 = 60;
const DURATION_MS: u32 = 600_000;
const PHYSICAL: u8 = 0x01;
const HOLY: u8 = 0x02;
const FIRE: u8 = 0x04;
const NATURE: u8 = 0x08;
const ARCANE: u8 = 0x40;
// vmangos SpellDamageClass; class 0 rolls no hit.
const DAMAGE_CLASS_NONE: u8 = 0;
const DAMAGE_CLASS_MAGIC: u8 = 1;
const DAMAGE_CLASS_MELEE: u8 = 2;
// Spell.dbc Attributes bit SPELL_ATTR_NO_ACTIVE_DEFENSE: no dodge, parry or block.
const ATTR_NO_ACTIVE_DEFENSE: u32 = 0x0020_0000;

// vmangos SpellDefines.h ProcFlags and ProcFlagsEx.
const DEAL_MELEE_SWING: u32 = 0x4;
const TAKE_MELEE_SWING: u32 = 0x8;
const DEAL_HARMFUL_SPELL: u32 = 0x1_0000;
const TAKE_HARMFUL_SPELL: u32 = 0x2_0000;
const TAKE_HARMFUL_ABILITY: u32 = 0x2000;
const PROC_EX_NORMAL_HIT: u32 = 0x1;
const PROC_EX_BLOCK: u32 = 0x40;
const PROC_EX_ABSORB: u32 = 0x400;
const PROC_EX_TRIGGER_ALWAYS: u32 = 0x1_0000;
// Spell.dbc attribute bit 0, SPELL_ATTR_PROC_FAILURE_BURNS_CHARGE.
const PROC_FAILURE_BURNS_CHARGE: u32 = 0x1;
// vmangos SpellModOp SPELLMOD_CHARGES.
const SPELLMOD_CHARGES: i32 = 4;
// vmangos SharedDefines.h Mechanics.
const MECHANIC_FEAR: u8 = 5;
const MECHANIC_CHARM: u8 = 1;
const MECHANIC_SLEEP: u8 = 10;
const MECHANIC_STUN: u8 = 12;
// vmangos creature_template.mechanic_immune_mask sets bit `mechanic - 1`.
const STUN_IMMUNE_MASK: u32 = 1 << (MECHANIC_STUN - 1);
const MECHANIC_SILENCE: u8 = 9;
const MECHANIC_SNARE: u8 = 11;
const MECHANIC_BLEED: u8 = 15;
const MECHANIC_INTERRUPT: u8 = 26;
// vmangos Unit::HandleHasteAuraProc names Flurry by this SpellVisual.
const FLURRY_VISUAL: u32 = 2759;
const SPELL_FAMILY_WARRIOR: u8 = 4;
const SHIELD_BLOCK_FAMILY_FLAG: i32 = 0x1000;
// AttackOutcome::wire codes in game_combat_event_live.hit_info.
const HIT_INFO_CRIT: &str = "1";
const HIT_INFO_AVOIDED: [&str; 3] = ["2", "3", "4"];
const ATTEMPTS: usize = 20;
const BLOCK_ATTEMPTS: usize = 120;

// Spell ids the Module names, as vmangos spell_mage.cpp does.
const COMBUSTION: u32 = 11129;
const COMBUSTION_CRIT_BUFF: u32 = 28682;

const FLURRY: u32 = 5_091_100;
const INNER_FIRE: u32 = 5_091_101;
const STORMSTRIKE: u32 = 5_091_102;
const NATURE_BOLT: u32 = 5_091_103;
const FIRE_BOLT: u32 = 5_091_104;
const SCORCH: u32 = 5_091_105;
const CERTAIN_SPELL_CRIT: u32 = 5_091_106;
// The Module names Fear Ward by its vmangos spell id.
const FEAR_WARD: u32 = 6346;
const FEAR_BOLT: u32 = 5_091_108;
const FOCUS: u32 = 5_091_109;
const SLOW_BOLT: u32 = 5_091_110;
const COSTLY_BOLT: u32 = 5_091_111;
const CERTAIN_CRIT: u32 = 5_091_112;
const SHIELD_BLOCK: u32 = 5_091_113;
const IMPROVED_SHIELD_BLOCK: u32 = 5_091_115;
const WINDFURY_ATTACK: u32 = 5_091_116;
const POWER_WORD_SHIELD: u32 = 5_091_117;
const PSYCHIC_SCREAM: u32 = 5_091_118;
const WILL_OF_THE_FORSAKEN: u32 = 5_091_119;
const SLEEP_FIXTURE: u32 = 5_091_120;
const STUN_FIXTURE: u32 = 5_091_121;
const MIND_EXHAUSTION: u32 = 5_091_122;
const MIND_CONTROL: u32 = 5_091_123;
const FEAR_AND_BOLT: u32 = 5_091_124;
const FEAR_AND_SLOW: u32 = 5_091_125;
const DOMINION_OF_SOUL: u32 = 5_091_126;
const EFFECT_FEAR: u32 = 5_091_127;
const HELPFUL_STUN_MECHANIC: u32 = 5_091_128;
const KICK_FIXTURE: u32 = 5_091_129;
const FROSTBOLT_FIXTURE: u32 = 5_091_130;
const SNARE_FIXTURE: u32 = 5_091_131;
const RAKE_FIXTURE: u32 = 5_091_132;
const REND_FIXTURE: u32 = 5_091_133;
const WOLF_CAST: u32 = 5_091_134;
const FORBEARANCE: u32 = 5_091_135;
const DIVINE_SHIELD: u32 = 5_091_136;
const BLESSING_OF_PROTECTION: u32 = 5_091_137;
// vmangos SharedDefines.h MECHANIC_INVULNERABILITY.
const MECHANIC_INVULNERABILITY: u8 = 25;
// The A_IMMUNITY p1 bit for SPELL_ATTR_EX_IMMUNITY_TO_HOSTILE_AND_FRIENDLY_EFFECTS.
const IMMUNE_TO_HOSTILE_AND_FRIENDLY: i32 = 0x100;
const BUBBLE_COOLDOWN_MS: u32 = 300_000;
// The seeded follow-up spells: vmangos FirstAidScript and PriestPowerWordShieldScript.
const RECENTLY_BANDAGED: u32 = 11196;
const WEAKENED_SOUL: u32 = 6788;
const LOOP_FIRST: u32 = 5_091_138;
const LOOP_SECOND: u32 = 5_091_139;
const REAGENT_PROC_AURA: u32 = 5_091_140;
const REAGENT_BOLT: u32 = 5_091_141;
const PROC_REAGENT: u32 = 5_091_142;
const CLEARCASTING: u32 = 5_091_143;
const PRESENCE_OF_MIND: u32 = 5_091_144;
const MAGE_BOLT: u32 = 5_091_145;
const SLOW_MAGE_BOLT: u32 = 5_091_146;
const UNMASKED_MAGE_BOLT: u32 = 5_091_147;
const COLD_BLOOD_FIXTURE: u32 = 5_091_148;
const UNCRITTABLE_MAGE_BOLT: u32 = 5_091_149;
const MAGE_HEAL: u32 = 5_091_150;
const MAGE_STRIKE: u32 = 5_091_151;
const SCHOOL_COST_FIXTURE: u32 = 5_091_152;
// Insight: aura 73 at -500 on every school but physical.
const INSIGHT: u32 = 18820;
const INSIGHT_COST: i32 = -500;
const INSIGHT_SCHOOLS: u8 = 0x7E;
// UNIT_FIELD_POWER_COST_MODIFIER is fields 173 to 179 by school; arcane is school 6.
const ARCANE_POWER_COST_MODIFIER: u16 = 179;
const SMSG_UPDATE_OBJECT: u16 = 0xA9;
const PACKET_WAIT: Duration = Duration::from_secs(10);
// Cold Blood 14177 and Divine Favor 20216: a flat +100% crit chance modifier with one charge.
const CERTAIN_CRIT_PERCENT: i32 = 100;
const DIVINE_FAVOR: u32 = 20216;
const HOLY_LIGHT: u32 = 635;
const FLASH_OF_LIGHT: u32 = 19750;
const SPELL_FAMILY_PALADIN: u8 = 10;
// The imported values: Divine Favor's mask as vmangos SpellModMgr sets it, and Flash of Light's
// family flags with those of its vmangos heal 19993.
const DIVINE_FAVOR_MASK: u32 = 0x8020_2000;
const HOLY_LIGHT_FAMILY_FLAGS: u64 = 0x8000_0000;
const FLASH_OF_LIGHT_FAMILY_FLAGS: u64 = 0x4000_2000;
const SPELL_FAMILY_MAGE: u8 = 3;
const MAGE_BOLT_FAMILY_FLAG: i32 = 0x1;
const UNMASKED_FAMILY_FLAG: i32 = 0x2;
const MANA: u32 = 1000;
const BOLT_COST: u32 = 10;
const PROC_ICD_MS: u32 = 100;
const FROST: u8 = 0x10;
const SLOW_POINTS: i32 = -50;
const BLEED_POINTS: i32 = 3;
const BLEED_PERIOD_MS: u32 = 3000;
const SPELL_ROLL_SEED: u64 = 1;
const MISS_NONE: &str = "0";
const MISS_IMMUNE: &str = "7";
const MISS_IMMUNE2: &str = "8";
// Spell.dbc Attributes and AttributesEx3 bits that skip immunity.
const ATTR_NO_IMMUNITIES: u32 = 0x2000_0000;
const ATTR_EX3_IGNORE_RESTRICTIONS: u32 = 0x1000_0000;

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
fn a_proc_refused_for_a_missing_reagent_still_spends_a_charge_and_starts_its_cooldown() {
    let mut standalone = Standalone::start("aura-charges-proc-reagent");
    let wolf = stage(&mut standalone);
    bolt(&standalone, REAGENT_BOLT, FIRE, 0);
    standalone.insert_row(
        "game_item_template",
        &[
            ("entry", &PROC_REAGENT),
            ("name", &"Proc reagent fixture"),
            ("max_stack", &20u32),
        ],
    );
    standalone.insert_row(
        "game_spell_reagent",
        &[
            ("id", &(u64::from(REAGENT_BOLT) << 3)),
            ("spell_id", &REAGENT_BOLT),
            ("item_entry", &PROC_REAGENT),
            ("count", &1u32),
        ],
    );
    aura_spell(
        &standalone,
        Charged {
            spell_id: REAGENT_PROC_AURA,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: DEAL_MELEE_SWING,
            charges: 3,
        },
        &[(A_PROC_TRIGGER, 0, 0, P_NONE)],
    );
    standalone.assert_sql(&format!(
        "UPDATE game_spell_effect SET trigger_spell = {REAGENT_BOLT} \
         WHERE spell_id = {REAGENT_PROC_AURA}"
    ));
    standalone.insert_row(
        "game_spell_proc_event",
        &[
            ("spell_id", &REAGENT_PROC_AURA),
            ("proc_ex", &PROC_EX_TRIGGER_ALWAYS),
            ("icd_ms", &PROC_ICD_MS),
        ],
    );
    standalone.assert_call("debug_force_cast", &["1", &REAGENT_PROC_AURA.to_string()]);
    standalone.insert_row(
        "game_conformance_swing_random",
        &[("character_guid", &1), ("state", &1)],
    );
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);
    assert_eq!(proc_ready_micros(&standalone), 0);

    swing(&standalone);
    assert_eq!(
        charges(&standalone, PLAYER, REAGENT_PROC_AURA),
        Some(2),
        "vmangos TriggerProccedSpell ignores the refused cast, so the proc spends a charge"
    );
    assert_ne!(
        proc_ready_micros(&standalone),
        0,
        "and starts the internal cooldown"
    );
    assert_eq!(item_stack(&standalone, PROC_REAGENT), None);

    standalone.assert_call("debug_grant_item", &["1", &PROC_REAGENT.to_string(), "1"]);
    swing(&standalone);
    assert_eq!(charges(&standalone, PLAYER, REAGENT_PROC_AURA), Some(1));
    assert_eq!(
        item_stack(&standalone, PROC_REAGENT),
        None,
        "a Character that carries the reagent has the proc take it"
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
        "vmangos IsTriggeredAtSpellProcEvent keeps Fear Ward off a spell whose mechanic is not fear"
    );
    cast_by(&standalone, wolf, FEAR_BOLT, PLAYER);
    assert_eq!(charges(&standalone, PLAYER, FEAR_WARD), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn fear_ward_refuses_a_fear_that_deals_no_damage_and_is_used_up_by_it() {
    let mut standalone = Standalone::start("aura-charges-fear-ward-refuses");
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
    control_spell(&standalone, PSYCHIC_SCREAM, M_FEAR, MECHANIC_FEAR);
    set_spell(
        &standalone,
        PSYCHIC_SCREAM,
        &format!("damage_class = {DAMAGE_CLASS_MAGIC}"),
    );
    set_level(&standalone, PLAYER, OUT_OF_REACH_LEVEL);
    standalone.assert_call("debug_force_cast", &["1", &FEAR_WARD.to_string()]);
    advance(&standalone);
    let events = subscribe_with_fence(
        &standalone,
        &[&format!(
            "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {PSYCHIC_SCREAM} AND kind = 2"
        )],
    );

    cast_by(&standalone, wolf, PSYCHIC_SCREAM, PLAYER);
    fence_subscription(&standalone, &events);
    assert_eq!(
        event_rows(&events, "game_spell_cast_event_live")[0]["miss_info"],
        MISS_IMMUNE,
        "the cast reports its target as immune, not as a resist, though a hit roll could resist it"
    );
    assert_eq!(
        stacks(&standalone, PSYCHIC_SCREAM),
        None,
        "vmangos Unit::IsImmuneToSpell refuses a fear the Fear Warded unit takes"
    );
    assert_eq!(
        charges(&standalone, PLAYER, FEAR_WARD),
        None,
        "the refused fear uses Fear Ward up, though it deals no damage"
    );
    set_spell(
        &standalone,
        PSYCHIC_SCREAM,
        &format!("damage_class = {DAMAGE_CLASS_NONE}"),
    );
    cast_by(&standalone, wolf, PSYCHIC_SCREAM, PLAYER);
    assert!(
        stacks(&standalone, PSYCHIC_SCREAM).is_some(),
        "the next fear lands"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn will_of_the_forsaken_refuses_a_fear_and_a_sleep_and_lets_a_stun_land() {
    let mut standalone = Standalone::start("aura-charges-will-of-the-forsaken");
    let wolf = stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: WILL_OF_THE_FORSAKEN,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: 0,
            charges: 0,
        },
        &[
            (A_IMMUNITY, 0, i32::from(MECHANIC_CHARM), P_MECHANIC),
            (A_IMMUNITY, 0, i32::from(MECHANIC_FEAR), P_MECHANIC),
            (A_IMMUNITY, 0, i32::from(MECHANIC_SLEEP), P_MECHANIC),
        ],
    );
    control_spell(&standalone, PSYCHIC_SCREAM, M_FEAR, MECHANIC_FEAR);
    control_spell(&standalone, SLEEP_FIXTURE, M_POLY, MECHANIC_SLEEP);
    control_spell(&standalone, STUN_FIXTURE, M_STUN, MECHANIC_STUN);
    standalone.assert_call(
        "debug_force_cast",
        &["1", &WILL_OF_THE_FORSAKEN.to_string()],
    );
    advance(&standalone);

    cast_by(&standalone, wolf, PSYCHIC_SCREAM, PLAYER);
    cast_by(&standalone, wolf, SLEEP_FIXTURE, PLAYER);
    cast_by(&standalone, wolf, STUN_FIXTURE, PLAYER);
    assert_eq!(stacks(&standalone, PSYCHIC_SCREAM), None, "fear is refused");
    assert_eq!(stacks(&standalone, SLEEP_FIXTURE), None, "sleep is refused");
    assert!(
        stacks(&standalone, STUN_FIXTURE).is_some(),
        "its charm immunity is mechanic 1, not LyraCore's M_STUN, so a stun lands"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn will_of_the_forsaken_refuses_a_charm_whose_spell_data_calls_it_helpful() {
    let mut standalone = Standalone::start("aura-charges-charm-polarity");
    let wolf = stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: WILL_OF_THE_FORSAKEN,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: 0,
            charges: 0,
        },
        &[(A_IMMUNITY, 0, i32::from(MECHANIC_CHARM), P_MECHANIC)],
    );
    control_spell(&standalone, MIND_CONTROL, M_POLY, MECHANIC_CHARM);
    set_spell(&standalone, MIND_CONTROL, "is_negative = false");
    standalone.assert_call(
        "debug_force_cast",
        &["1", &WILL_OF_THE_FORSAKEN.to_string()],
    );
    advance(&standalone);

    cast_by(&standalone, wolf, MIND_CONTROL, PLAYER);
    assert_eq!(
        stacks(&standalone, MIND_CONTROL),
        None,
        "vmangos IsPositiveEffect makes a charm aimed at an enemy harmful, whatever the spell flags say"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_spell_that_skips_immunities_lands_through_fear_ward() {
    let mut standalone = Standalone::start("aura-charges-skips-immunities");
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
    control_spell(&standalone, PSYCHIC_SCREAM, M_FEAR, MECHANIC_FEAR);
    standalone.assert_call("debug_force_cast", &["1", &FEAR_WARD.to_string()]);
    advance(&standalone);

    for column in [
        format!("attributes = {ATTR_NO_IMMUNITIES}"),
        format!("attributes_ex_3 = {ATTR_EX3_IGNORE_RESTRICTIONS}"),
    ] {
        set_spell(&standalone, PSYCHIC_SCREAM, &column);
        cast_by(&standalone, wolf, PSYCHIC_SCREAM, PLAYER);
        assert!(
            stacks(&standalone, PSYCHIC_SCREAM).is_some(),
            "{column} skips the immunity"
        );
        standalone.assert_sql(&format!(
            "DELETE FROM game_aura WHERE spell_id = {PSYCHIC_SCREAM}"
        ));
        set_spell(
            &standalone,
            PSYCHIC_SCREAM,
            "attributes = 0, attributes_ex_3 = 0",
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_fear_with_a_speed_effect_is_refused_whole_and_uses_fear_ward_up() {
    let mut standalone = Standalone::start("aura-charges-fear-and-slow");
    let wolf = stage(&mut standalone);
    real_fear_ward(&standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: FEAR_AND_SLOW,
            school_mask: NATURE,
            target: T_TARGET_ENEMY,
            proc_flags: 0,
            charges: 0,
        },
        &[
            (A_CONTROL, 0, M_FEAR, P_MECHANIC),
            (A_MOD_SPEED, -50, i32::from(SPEED_MOVE), P_SPEED_KIND),
        ],
    );
    set_spell(
        &standalone,
        FEAR_AND_SLOW,
        &format!("mechanic = {MECHANIC_FEAR}"),
    );

    let miss = cast_miss_code(&standalone, wolf, FEAR_AND_SLOW, PLAYER);
    assert_eq!(
        miss, MISS_IMMUNE,
        "vmangos refuses the whole spell by its header mechanic"
    );
    assert_eq!(
        stacks(&standalone, FEAR_AND_SLOW),
        None,
        "no effect of the fear, its speed Aura included, lands"
    );
    assert_eq!(charges(&standalone, PLAYER, FEAR_WARD), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_effect_mechanic_refuses_an_effect_and_reports_immune_only_when_none_landed() {
    let mut standalone = Standalone::start("aura-charges-effect-mechanic");
    let wolf = stage(&mut standalone);
    real_fear_ward(&standalone);
    control_spell_by_effect(&standalone, FEAR_AND_BOLT, M_FEAR, MECHANIC_FEAR);
    add_damage_effect(&standalone, FEAR_AND_BOLT);
    set_spell(
        &standalone,
        FEAR_AND_BOLT,
        &format!("cast_flags = {CAST_FLAG_IGNORE_RESISTANCES}"),
    );
    control_spell_by_effect(&standalone, EFFECT_FEAR, M_FEAR, MECHANIC_FEAR);

    let mixed = cast_miss_code(&standalone, wolf, FEAR_AND_BOLT, PLAYER);
    assert_eq!(
        stacks(&standalone, FEAR_AND_BOLT),
        None,
        "the fear effect is refused"
    );
    assert_eq!(
        mixed, MISS_NONE,
        "its damage effect landed, so the target is not immune"
    );
    let whole = cast_miss_code(&standalone, wolf, EFFECT_FEAR, PLAYER);
    assert_eq!(stacks(&standalone, EFFECT_FEAR), None);
    assert_eq!(
        whole, MISS_IMMUNE2,
        "vmangos reports Immune2 when the effect masks, not the header, took every effect"
    );
    assert_eq!(
        charges(&standalone, PLAYER, FEAR_WARD),
        Some(1),
        "Fear Ward reads the header mechanic, which neither spell has"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn will_of_the_forsaken_refuses_a_spell_when_any_effect_aims_at_an_enemy() {
    let mut standalone = Standalone::start("aura-charges-dominion");
    let wolf = stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: WILL_OF_THE_FORSAKEN,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: 0,
            charges: 0,
        },
        &[(A_IMMUNITY, 0, i32::from(MECHANIC_CHARM), P_MECHANIC)],
    );
    aura_spell(
        &standalone,
        Charged {
            spell_id: DOMINION_OF_SOUL,
            school_mask: NATURE,
            target: T_TARGET_ENEMY,
            proc_flags: 0,
            charges: 0,
        },
        &[
            (A_CONTROL, 0, M_POLY, P_MECHANIC),
            (A_MOD_SPEED, 20, i32::from(SPEED_CAST), P_SPEED_KIND),
        ],
    );
    set_spell(
        &standalone,
        DOMINION_OF_SOUL,
        &format!("mechanic = {MECHANIC_CHARM}, is_negative = false"),
    );
    standalone.assert_sql(&format!(
        "UPDATE game_spell_effect SET target = {T_TARGET_ANY} \
         WHERE spell_id = {DOMINION_OF_SOUL} AND effect_index = 0"
    ));
    standalone.assert_call(
        "debug_force_cast",
        &["1", &WILL_OF_THE_FORSAKEN.to_string()],
    );
    advance(&standalone);

    cast_by(&standalone, wolf, DOMINION_OF_SOUL, PLAYER);
    assert_eq!(
        stacks(&standalone, DOMINION_OF_SOUL),
        None,
        "its haste effect aims at an enemy, so vmangos IsPositiveSpell is false for the whole spell"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_unit_is_immune_to_its_own_cast_by_header_mechanic() {
    let mut standalone = Standalone::start("aura-charges-self-cast");
    stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: WILL_OF_THE_FORSAKEN,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: 0,
            charges: 0,
        },
        &[(A_IMMUNITY, 0, i32::from(MECHANIC_SLEEP), P_MECHANIC)],
    );
    control_spell(&standalone, SLEEP_FIXTURE, M_POLY, MECHANIC_SLEEP);
    standalone.assert_call(
        "debug_force_cast",
        &["1", &WILL_OF_THE_FORSAKEN.to_string()],
    );
    advance(&standalone);

    cast_by(&standalone, PLAYER, SLEEP_FIXTURE, PLAYER);
    assert_eq!(
        stacks(&standalone, SLEEP_FIXTURE),
        None,
        "vmangos Unit::IsImmuneToSpell ignores castOnSelf, as Dreamless Sleep through Will of the Forsaken shows"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stun_lands_on_a_creature_before_its_template_mask_is_set_and_is_refused_after() {
    let mut standalone = Standalone::start("aura-charges-template-stun");
    let stunned_wolf = stage(&mut standalone);
    let immune_wolf = spawn_wolf(&standalone);
    control_spell(&standalone, STUN_FIXTURE, M_STUN, MECHANIC_STUN);

    cast(&standalone, STUN_FIXTURE, stunned_wolf);
    assert!(
        aura_column(&standalone, stunned_wolf, STUN_FIXTURE, "stacks").is_some(),
        "a creature is stunned before its template has a mask"
    );
    set_wolf_mechanic_immune_mask(&standalone, STUN_IMMUNE_MASK);
    assert_eq!(
        cast_miss_code(&standalone, PLAYER, STUN_FIXTURE, immune_wolf),
        MISS_IMMUNE,
        "vmangos Creature::IsImmuneToSpell refuses a stun its template's mechanic_immune_mask names"
    );
    assert_eq!(
        aura_column(&standalone, immune_wolf, STUN_FIXTURE, "stacks"),
        None
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn kick_deals_its_damage_to_an_interrupt_immune_creature_and_leaves_its_cast() {
    let mut standalone = Standalone::start("aura-charges-kick-interrupt-immune");
    let wolf = stage(&mut standalone);
    interrupt_spell(&standalone, KICK_FIXTURE);
    set_wolf_mechanic_immune_mask(&standalone, 1 << (MECHANIC_INTERRUPT - 1));
    wolf_begins_a_cast(&standalone, wolf);

    let go = go_event(&standalone, PLAYER, KICK_FIXTURE, wolf);
    assert_eq!(go["miss_info"], MISS_NONE);
    advance(&standalone);
    assert!(
        health(&standalone, wolf) < WOLF_HEALTH,
        "Kick's damage effect lands"
    );
    assert!(
        is_casting(&standalone, wolf),
        "vmangos Creature::IsImmuneToSpellEffect drops the interrupt effect of mechanic 26"
    );
    assert!(lockouts(&standalone, wolf).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn frostbolt_deals_its_damage_to_a_snare_immune_creature_without_its_slow() {
    let mut standalone = Standalone::start("aura-charges-frostbolt-snare-immune");
    let wolf = stage(&mut standalone);
    bolt(&standalone, FROSTBOLT_FIXTURE, FROST, 0);
    set_spell(
        &standalone,
        FROSTBOLT_FIXTURE,
        &format!("duration_ms = {DURATION_MS}"),
    );
    mechanic_effect(
        &standalone,
        FROSTBOLT_FIXTURE,
        (1, A_MOD_SPEED, SLOW_POINTS),
        (i32::from(SPEED_MOVE), P_SPEED_KIND),
        MECHANIC_SNARE,
    );
    set_wolf_mechanic_immune_mask(&standalone, 1 << (MECHANIC_SNARE - 1));

    let go = go_event(&standalone, PLAYER, FROSTBOLT_FIXTURE, wolf);
    assert_eq!(go["miss_info"], MISS_NONE);
    advance(&standalone);
    assert!(
        health(&standalone, wolf) < WOLF_HEALTH,
        "Frostbolt's damage effect lands"
    );
    assert_eq!(
        aura_column(&standalone, wolf, FROSTBOLT_FIXTURE, "stacks"),
        None,
        "vmangos Creature::IsImmuneToSpellEffect drops the slow of mechanic 11"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_snare_whose_only_effect_is_dropped_reports_immune2() {
    let mut standalone = Standalone::start("aura-charges-snare-only-immune2");
    let wolf = stage(&mut standalone);
    snare_only_spell(&standalone);
    set_wolf_mechanic_immune_mask(&standalone, 1 << (MECHANIC_SNARE - 1));

    assert_eq!(
        cast_miss_code(&standalone, PLAYER, SNARE_FIXTURE, wolf),
        MISS_IMMUNE2
    );
    assert_eq!(
        aura_column(&standalone, wolf, SNARE_FIXTURE, "stacks"),
        None
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_snare_whose_only_effect_is_dropped_reports_immune2_when_its_hit_roll_misses() {
    let mut standalone = Standalone::start("aura-charges-snare-immune2-on-miss");
    let wolf = stage(&mut standalone);
    snare_only_spell(&standalone);
    set_spell(
        &standalone,
        SNARE_FIXTURE,
        &format!("damage_class = {DAMAGE_CLASS_MAGIC}"),
    );
    set_level(&standalone, wolf, OUT_OF_REACH_LEVEL);
    standalone.insert_row(
        "game_conformance_spell_random",
        &[
            ("character_guid", &PLAYER),
            ("target_guid", &wolf),
            ("state", &SPELL_ROLL_SEED),
        ],
    );

    let missed = cast_miss_code(&standalone, PLAYER, SNARE_FIXTURE, wolf);
    assert!(
        missed != MISS_NONE && missed != MISS_IMMUNE2,
        "the seeded roll misses a target with no immunity: {missed}"
    );
    standalone.assert_sql(&format!(
        "UPDATE game_conformance_spell_random SET state = {SPELL_ROLL_SEED} \
         WHERE character_guid = {PLAYER}"
    ));
    set_wolf_mechanic_immune_mask(&standalone, 1 << (MECHANIC_SNARE - 1));
    assert_eq!(
        cast_miss_code(&standalone, PLAYER, SNARE_FIXTURE, wolf),
        MISS_IMMUNE2,
        "vmangos WriteSpellGoTargets reports Immune2 for an empty effect mask, whatever the roll"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn rake_deals_its_damage_to_a_bleed_immune_creature_without_its_bleed() {
    let mut standalone = Standalone::start("aura-charges-rake-bleed-immune");
    let wolf = stage(&mut standalone);
    bolt(&standalone, RAKE_FIXTURE, PHYSICAL, 0);
    set_spell(
        &standalone,
        RAKE_FIXTURE,
        &format!("duration_ms = {DURATION_MS}"),
    );
    mechanic_effect(
        &standalone,
        RAKE_FIXTURE,
        (1, A_PERIODIC_DAMAGE, BLEED_POINTS),
        (0, P_NONE),
        MECHANIC_BLEED,
    );
    bleed_period(&standalone, RAKE_FIXTURE);
    set_wolf_mechanic_immune_mask(&standalone, 1 << (MECHANIC_BLEED - 1));

    let go = go_event(&standalone, PLAYER, RAKE_FIXTURE, wolf);
    assert_eq!(go["miss_info"], MISS_NONE);
    advance(&standalone);
    assert!(
        health(&standalone, wolf) < WOLF_HEALTH,
        "Rake's damage effect lands"
    );
    assert_eq!(
        aura_column(&standalone, wolf, RAKE_FIXTURE, "stacks"),
        None,
        "vmangos drops Rake's bleed, an effect of mechanic 15"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn rend_is_refused_whole_by_a_bleed_immune_creature() {
    let mut standalone = Standalone::start("aura-charges-rend-bleed-immune");
    let wolf = stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: REND_FIXTURE,
            school_mask: PHYSICAL,
            target: T_TARGET_ENEMY,
            proc_flags: 0,
            charges: 0,
        },
        &[(A_PERIODIC_DAMAGE, BLEED_POINTS, 0, P_NONE)],
    );
    bleed_period(&standalone, REND_FIXTURE);
    set_spell(
        &standalone,
        REND_FIXTURE,
        &format!("mechanic = {MECHANIC_BLEED}"),
    );
    set_wolf_mechanic_immune_mask(&standalone, 1 << (MECHANIC_BLEED - 1));

    assert_eq!(
        cast_miss_code(&standalone, PLAYER, REND_FIXTURE, wolf),
        MISS_IMMUNE,
        "Rend's header mechanic is bleed, so the whole spell is refused"
    );
    assert_eq!(aura_column(&standalone, wolf, REND_FIXTURE, "stacks"), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_interrupt_stops_a_silence_immune_creature_without_locking_its_school() {
    let mut standalone = Standalone::start("aura-charges-silence-immune-lockout");
    let wolf = stage(&mut standalone);
    interrupt_spell(&standalone, KICK_FIXTURE);

    wolf_begins_a_cast(&standalone, wolf);
    go_event(&standalone, PLAYER, KICK_FIXTURE, wolf);
    assert!(!is_casting(&standalone, wolf));
    assert_eq!(
        lockouts(&standalone, wolf),
        [FIRE.to_string()],
        "an interrupt locks the school of a creature without the mask"
    );

    standalone.assert_sql(&format!(
        "DELETE FROM game_school_lockout WHERE caster_guid = {wolf}"
    ));
    set_wolf_mechanic_immune_mask(&standalone, 1 << (MECHANIC_SILENCE - 1));
    advance(&standalone);
    wolf_begins_a_cast(&standalone, wolf);
    go_event(&standalone, PLAYER, KICK_FIXTURE, wolf);
    assert!(
        !is_casting(&standalone, wolf),
        "silence immunity does not stop an interrupt"
    );
    assert!(
        lockouts(&standalone, wolf).is_empty(),
        "vmangos Creature::LockOutSpells skips a template immune to silence"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_bandage_leaves_recently_bandaged_and_the_next_bandage_is_refused() {
    let mut standalone = Standalone::start("aura-charges-bandage");
    stage(&mut standalone);

    cast(&standalone, LINEN_BANDAGE_SPELL, PLAYER);
    assert!(
        stacks(&standalone, RECENTLY_BANDAGED).is_some(),
        "vmangos FirstAidScript casts Recently Bandaged at the unit the bandage hit"
    );
    standalone.assert_sql(&format!(
        "DELETE FROM game_aura WHERE spell_id = {LINEN_BANDAGE_SPELL}"
    ));
    assert_refused_as_immune(&standalone, LINEN_BANDAGE_SPELL, PLAYER);
    assert_eq!(stacks(&standalone, LINEN_BANDAGE_SPELL), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn power_word_shield_leaves_weakened_soul_and_the_next_shield_is_refused_on_self_and_ally() {
    let mut standalone = Standalone::start("aura-charges-weakened-soul");
    stage(&mut standalone);
    let ally = spawn_ally(&standalone);

    for target in [PLAYER, ally] {
        cast(&standalone, TEST_POWER_WORD_SHIELD_SPELL, target);
        assert!(
            aura_column(&standalone, target, WEAKENED_SOUL, "stacks").is_some(),
            "vmangos PriestPowerWordShieldScript casts Weakened Soul at the shielded unit {target}"
        );
        standalone.assert_sql(&format!(
            "DELETE FROM game_aura WHERE spell_id = {TEST_POWER_WORD_SHIELD_SPELL}"
        ));
        standalone.assert_sql("DELETE FROM game_global_cooldown");
        assert_refused_as_immune(&standalone, TEST_POWER_WORD_SHIELD_SPELL, target);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_follow_up_that_triggers_its_own_spell_stops_after_one_round() {
    let mut standalone = Standalone::start("aura-charges-follow-up-loop");
    stage(&mut standalone);
    for spell_id in [LOOP_FIRST, LOOP_SECOND] {
        aura_spell(
            &standalone,
            Charged {
                spell_id,
                school_mask: PHYSICAL,
                target: T_SELF,
                proc_flags: 0,
                charges: 0,
            },
            &[(A_MOD_RESISTANCE, 1, i32::from(RESIST_ARMOR), P_SCHOOL_MASK)],
        );
    }
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(LOOP_SECOND, 1)),
            ("spell_id", &LOOP_SECOND),
            ("effect_index", &1u8),
            ("kind", &E_TRIGGER),
            ("target", &T_SELF),
            ("trigger_spell", &LOOP_FIRST),
        ],
    );
    standalone.insert_row(
        "game_spell_follow_up",
        &[
            ("spell_id", &LOOP_FIRST),
            ("follow_up_spell_id", &LOOP_SECOND),
        ],
    );

    cast(&standalone, LOOP_FIRST, PLAYER);
    assert!(stacks(&standalone, LOOP_FIRST).is_some());
    assert!(
        stacks(&standalone, LOOP_SECOND).is_some(),
        "the follow-up runs once, and the spell it triggers makes no second follow-up"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn divine_shield_applies_forbearance_to_its_caster() {
    let mut standalone = Standalone::start("aura-charges-forbearance");
    stage(&mut standalone);
    bubble(&standalone, DIVINE_SHIELD, T_SELF);

    cast(&standalone, DIVINE_SHIELD, PLAYER);
    assert!(stacks(&standalone, DIVINE_SHIELD).is_some());
    assert!(
        stacks(&standalone, FORBEARANCE).is_some(),
        "vmangos PaladinBubbleScript casts Forbearance at the unit Divine Shield hit"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_second_divine_shield_is_refused_while_forbearance_lasts() {
    let mut standalone = Standalone::start("aura-charges-forbearance-second-bubble");
    stage(&mut standalone);
    bubble(&standalone, DIVINE_SHIELD, T_SELF);
    cast(&standalone, DIVINE_SHIELD, PLAYER);
    standalone.assert_sql(&format!(
        "DELETE FROM game_aura WHERE spell_id = {DIVINE_SHIELD}"
    ));

    assert_refused_as_immune(&standalone, DIVINE_SHIELD, PLAYER);
    assert_eq!(stacks(&standalone, DIVINE_SHIELD), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn blessing_of_protection_on_an_ally_under_forbearance_is_refused_before_its_cooldown_starts() {
    let mut standalone = Standalone::start("aura-charges-forbearance-ally");
    stage(&mut standalone);
    let ally = spawn_ally(&standalone);
    bubble(&standalone, BLESSING_OF_PROTECTION, T_TARGET_ALLY);
    cast(&standalone, BLESSING_OF_PROTECTION, ally);
    assert!(
        aura_column(&standalone, ally, FORBEARANCE, "stacks").is_some(),
        "vmangos casts Forbearance at the ally that Blessing of Protection hit"
    );
    standalone.assert_sql(&format!(
        "DELETE FROM game_aura WHERE spell_id = {BLESSING_OF_PROTECTION}"
    ));
    set_spell(
        &standalone,
        BLESSING_OF_PROTECTION,
        &format!("cooldown_ms = {BUBBLE_COOLDOWN_MS}"),
    );

    assert_refused_as_immune(&standalone, BLESSING_OF_PROTECTION, ally);
    assert!(
        standalone
            .query_rows(&format!(
                "SELECT id FROM game_spell_cd WHERE caster_guid = {PLAYER} \
                 AND spell_id = {BLESSING_OF_PROTECTION}"
            ))
            .is_empty(),
        "the refusal comes before the cooldown starts"
    );
    assert_eq!(
        aura_column(&standalone, ally, BLESSING_OF_PROTECTION, "stacks"),
        None
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stun_immune_template_refuses_a_helpful_spell_of_its_mechanic_when_the_cast_starts() {
    let mut standalone = Standalone::start("aura-charges-template-helpful");
    let caster = stage(&mut standalone);
    let ally = spawn_wolf(&standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: HELPFUL_STUN_MECHANIC,
            school_mask: NATURE,
            target: T_TARGET_ANY,
            proc_flags: 0,
            charges: 0,
        },
        &[(A_MOD_RESISTANCE, 10, i32::from(RESIST_ARMOR), P_SCHOOL_MASK)],
    );
    set_spell(
        &standalone,
        HELPFUL_STUN_MECHANIC,
        &format!("is_negative = false, mechanic = {MECHANIC_STUN}"),
    );
    set_wolf_mechanic_immune_mask(&standalone, STUN_IMMUNE_MASK);

    let output = standalone.call(
        "debug_begin_cast",
        &[
            &caster.to_string(),
            &HELPFUL_STUN_MECHANIC.to_string(),
            &ally.to_string(),
        ],
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && text.contains("is immune to its mechanic"),
        "vmangos Spell::CheckCast refuses a helpful spell with TARGET_AURASTATE, \
         whatever the template mask's polarity: {text}"
    );
    assert_eq!(
        aura_column(&standalone, ally, HELPFUL_STUN_MECHANIC, "stacks"),
        None
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stun_immune_template_refuses_a_stun_effect_and_reports_immune2() {
    let mut standalone = Standalone::start("aura-charges-template-stun-effect");
    let wolf = stage(&mut standalone);
    control_spell_by_effect(&standalone, STUN_FIXTURE, M_STUN, MECHANIC_STUN);
    set_wolf_mechanic_immune_mask(&standalone, STUN_IMMUNE_MASK);

    assert_eq!(
        cast_miss_code(&standalone, PLAYER, STUN_FIXTURE, wolf),
        MISS_IMMUNE2,
        "vmangos Creature::IsImmuneToSpellEffect reads the template mask by effect mechanic"
    );
    assert_eq!(aura_column(&standalone, wolf, STUN_FIXTURE, "stacks"), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stun_immune_template_lets_the_creature_stun_itself() {
    let mut standalone = Standalone::start("aura-charges-template-self-stun");
    let wolf = stage(&mut standalone);
    control_spell(&standalone, STUN_FIXTURE, M_STUN, MECHANIC_STUN);
    set_wolf_mechanic_immune_mask(&standalone, STUN_IMMUNE_MASK);

    cast_by(&standalone, wolf, STUN_FIXTURE, wolf);
    assert!(
        aura_column(&standalone, wolf, STUN_FIXTURE, "stacks").is_some(),
        "vmangos Creature::IsImmuneToSpell skips the template mask when castOnSelf"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_debuff_that_names_a_mechanic_refuses_no_harmful_spell_of_it() {
    let mut standalone = Standalone::start("aura-charges-mind-exhaustion");
    let wolf = stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: MIND_EXHAUSTION,
            school_mask: PHYSICAL,
            target: T_TARGET_ENEMY,
            proc_flags: 0,
            charges: 0,
        },
        &[(A_IMMUNITY, 0, i32::from(MECHANIC_STUN), P_MECHANIC)],
    );
    control_spell(&standalone, STUN_FIXTURE, M_STUN, MECHANIC_STUN);

    cast_by(&standalone, wolf, MIND_EXHAUSTION, PLAYER);
    assert!(stacks(&standalone, MIND_EXHAUSTION).is_some());
    cast_by(&standalone, wolf, STUN_FIXTURE, PLAYER);
    assert!(
        stacks(&standalone, STUN_FIXTURE).is_some(),
        "vmangos immunity refuses a spell of the other polarity, and a debuff refuses only helpful ones"
    );
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
fn insight_makes_one_costly_magic_spell_free_and_that_spell_spends_it() {
    let mut standalone = Standalone::start("aura-charges-insight");
    let wolf = stage(&mut standalone);
    insight(&standalone);
    bolt(&standalone, FIRE_BOLT, FIRE, 0);
    bolt(&standalone, COSTLY_BOLT, FIRE, 0);
    set_spell(&standalone, COSTLY_BOLT, &format!("cost = {BOLT_COST}"));
    use_mana(&standalone);
    standalone.assert_call("debug_force_cast", &["1", &INSIGHT.to_string()]);
    advance(&standalone);

    cast(&standalone, FIRE_BOLT, wolf);
    assert_eq!(
        charges(&standalone, PLAYER, INSIGHT),
        Some(1),
        "vmangos HandleModPowerCostSchoolAuraProc skips a spell with no cost"
    );
    cast(&standalone, COSTLY_BOLT, wolf);
    assert_eq!(
        mana(&standalone),
        MANA,
        "Insight's -500 makes the spell free"
    );
    assert_eq!(charges(&standalone, PLAYER, INSIGHT), None);
    cast(&standalone, COSTLY_BOLT, wolf);
    assert_eq!(mana(&standalone), MANA - BOLT_COST);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn insight_sends_its_flat_cost_by_school_and_clears_it_when_spent() {
    let mut standalone = Standalone::start("aura-charges-insight-fields");
    let wolf = stage(&mut standalone);
    let mut wire = standalone.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    wire.call("debug_native_enter_world", json!([PLAYER]))
        .unwrap();
    insight(&standalone);
    bolt(&standalone, COSTLY_BOLT, FIRE, 0);
    set_spell(&standalone, COSTLY_BOLT, &format!("cost = {BOLT_COST}"));
    use_mana(&standalone);
    wire.drain_packets();

    standalone.assert_call("debug_force_cast", &["1", &INSIGHT.to_string()]);
    advance(&standalone);
    assert!(
        sends_power_cost_field(&mut wire, ARCANE_POWER_COST_MODIFIER, INSIGHT_COST as u32),
        "vmangos Aura::HandleModPowerCost adds -500 to UNIT_FIELD_POWER_COST_MODIFIER + arcane"
    );
    cast(&standalone, COSTLY_BOLT, wolf);
    assert_eq!(charges(&standalone, PLAYER, INSIGHT), None);
    assert!(
        sends_power_cost_field(&mut wire, ARCANE_POWER_COST_MODIFIER, 0),
        "the spent Insight takes its -500 back off the field"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn school_power_cost_auras_change_the_cost_of_a_spell_by_its_first_school() {
    let mut standalone = Standalone::start("aura-charges-school-power-cost");
    let wolf = stage(&mut standalone);
    // As Fire Focus 5837 (-2), Lucifron's Curse 19703 (+100%) and a -100% frost cost Aura.
    aura_spell(
        &standalone,
        Charged {
            spell_id: SCHOOL_COST_FIXTURE,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: 0,
            charges: 0,
        },
        &[
            (A_MOD_POWER_COST_FLAT, -2, i32::from(FIRE), P_SCHOOL_MASK),
            (A_MOD_POWER_COST_PCT, 100, i32::from(FIRE), P_SCHOOL_MASK),
            (A_MOD_POWER_COST_PCT, -100, i32::from(FROST), P_SCHOOL_MASK),
        ],
    );
    bolt(&standalone, COSTLY_BOLT, FIRE | FROST, 0);
    set_spell(&standalone, COSTLY_BOLT, &format!("cost = {BOLT_COST}"));
    use_mana(&standalone);
    standalone.assert_call("debug_force_cast", &["1", &SCHOOL_COST_FIXTURE.to_string()]);
    advance(&standalone);

    cast(&standalone, COSTLY_BOLT, wolf);
    assert_eq!(
        mana(&standalone),
        MANA - (BOLT_COST - 2) * 2,
        "vmangos CalculatePowerCost reads the first school, adds the flat cost, then multiplies"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn clearcasting_makes_one_spell_free_and_that_spell_spends_it() {
    let mut standalone = Standalone::start("aura-charges-clearcasting");
    let wolf = stage(&mut standalone);
    mage_modifier(&standalone, CLEARCASTING, -100, SPELLMOD_OP_COST);
    mage_bolt(
        &standalone,
        MAGE_BOLT,
        MAGE_BOLT_FAMILY_FLAG,
        &format!("cost = {BOLT_COST}"),
    );
    use_mana(&standalone);
    standalone.assert_call("debug_force_cast", &["1", &CLEARCASTING.to_string()]);
    advance(&standalone);

    cast(&standalone, MAGE_BOLT, wolf);
    assert_eq!(
        mana(&standalone),
        MANA,
        "Clearcasting's -100% cost makes the spell free"
    );
    assert_eq!(
        charges(&standalone, PLAYER, CLEARCASTING),
        None,
        "vmangos RemoveSpellMods removes the Aura when the cast takes its last charge"
    );
    cast(&standalone, MAGE_BOLT, wolf);
    assert_eq!(mana(&standalone), MANA - BOLT_COST);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn presence_of_mind_makes_one_cast_instant_and_ends_with_it() {
    let mut standalone = Standalone::start("aura-charges-presence-of-mind");
    let wolf = stage(&mut standalone);
    mage_modifier(
        &standalone,
        PRESENCE_OF_MIND,
        -100,
        SPELLMOD_OP_CASTING_TIME,
    );
    mage_bolt(
        &standalone,
        SLOW_MAGE_BOLT,
        MAGE_BOLT_FAMILY_FLAG,
        "cast_time_ms = 1500",
    );
    standalone.assert_call("debug_force_cast", &["1", &PRESENCE_OF_MIND.to_string()]);
    advance(&standalone);

    begin_cast(&standalone, SLOW_MAGE_BOLT, wolf);
    assert!(
        !is_casting(&standalone, PLAYER),
        "Presence of Mind makes the cast instant"
    );
    assert_eq!(charges(&standalone, PLAYER, PRESENCE_OF_MIND), None);
    advance(&standalone);
    begin_cast(&standalone, SLOW_MAGE_BOLT, wolf);
    assert!(
        is_casting(&standalone, PLAYER),
        "the next cast takes its whole cast time"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_interrupted_cast_keeps_clearcasting_for_the_cast_that_completes() {
    let mut standalone = Standalone::start("aura-charges-clearcasting-interrupt");
    let wolf = stage(&mut standalone);
    mage_modifier(&standalone, CLEARCASTING, -100, SPELLMOD_OP_COST);
    mage_bolt(
        &standalone,
        SLOW_MAGE_BOLT,
        MAGE_BOLT_FAMILY_FLAG,
        &format!("cast_time_ms = 1500, cost = {BOLT_COST}"),
    );
    use_mana(&standalone);
    standalone.assert_call("debug_force_cast", &["1", &CLEARCASTING.to_string()]);
    advance(&standalone);

    begin_cast(&standalone, SLOW_MAGE_BOLT, wolf);
    assert!(is_casting(&standalone, PLAYER));
    standalone.assert_call("debug_cancel_cast", &["1"]);
    assert!(!is_casting(&standalone, PLAYER));
    assert_eq!(
        charges(&standalone, PLAYER, CLEARCASTING),
        Some(1),
        "vmangos RestoreSpellMods gives an interrupted cast's charge back"
    );

    begin_cast(&standalone, SLOW_MAGE_BOLT, wolf);
    standalone.assert_call("debug_advance_roll_batch", &["40"]);
    assert!(!is_casting(&standalone, PLAYER), "the cast completes");
    assert_eq!(mana(&standalone), MANA);
    assert_eq!(charges(&standalone, PLAYER, CLEARCASTING), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_spell_outside_the_family_mask_spends_no_clearcasting() {
    let mut standalone = Standalone::start("aura-charges-clearcasting-unmasked");
    let wolf = stage(&mut standalone);
    mage_modifier(&standalone, CLEARCASTING, -100, SPELLMOD_OP_COST);
    mage_bolt(
        &standalone,
        UNMASKED_MAGE_BOLT,
        UNMASKED_FAMILY_FLAG,
        &format!("cost = {BOLT_COST}"),
    );
    use_mana(&standalone);
    standalone.assert_call("debug_force_cast", &["1", &CLEARCASTING.to_string()]);
    advance(&standalone);

    cast(&standalone, UNMASKED_MAGE_BOLT, wolf);
    assert_eq!(mana(&standalone), MANA - BOLT_COST);
    assert_eq!(charges(&standalone, PLAYER, CLEARCASTING), Some(1));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_crit_chance_modifier_is_kept_by_a_spell_that_cannot_crit_and_spent_by_the_crit_it_gives() {
    let mut standalone = Standalone::start("aura-charges-crit-modifier-spell");
    let wolf = stage(&mut standalone);
    cold_blood_fixture(&standalone);
    mage_bolt(
        &standalone,
        UNCRITTABLE_MAGE_BOLT,
        MAGE_BOLT_FAMILY_FLAG,
        &format!(
            "damage_class = {DAMAGE_CLASS_MAGIC}, \
             cast_flags = {}",
            CAST_FLAG_CANNOT_CRIT | CAST_FLAG_IGNORE_RESISTANCES
        ),
    );
    mage_bolt(
        &standalone,
        MAGE_BOLT,
        MAGE_BOLT_FAMILY_FLAG,
        &format!("damage_class = {DAMAGE_CLASS_MAGIC}"),
    );

    let go = go_event(&standalone, PLAYER, UNCRITTABLE_MAGE_BOLT, wolf);
    assert_eq!(go["is_crit"], "false");
    assert_eq!(
        charges(&standalone, PLAYER, COLD_BLOOD_FIXTURE),
        Some(1),
        "vmangos GetSpellCritChance returns before the modifier for a Spell that cannot crit"
    );
    advance(&standalone);
    let go = go_event(&standalone, PLAYER, MAGE_BOLT, wolf);
    assert_eq!(
        go["is_crit"], "true",
        "the +100% crit chance makes the crit certain"
    );
    assert_eq!(charges(&standalone, PLAYER, COLD_BLOOD_FIXTURE), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_crit_chance_modifier_makes_the_next_masked_heal_crit() {
    let mut standalone = Standalone::start("aura-charges-crit-modifier-heal");
    stage(&mut standalone);
    cold_blood_fixture(&standalone);
    mage_spell(
        &standalone,
        MAGE_HEAL,
        (E_HEAL, T_SELF, 0, P_NONE),
        &format!("damage_class = {DAMAGE_CLASS_MAGIC}"),
    );
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 1 WHERE guid = {PLAYER}"
    ));

    let go = go_event(&standalone, PLAYER, MAGE_HEAL, PLAYER);
    assert_eq!(go["is_crit"], "true");
    assert_eq!(charges(&standalone, PLAYER, COLD_BLOOD_FIXTURE), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_crit_chance_modifier_makes_the_next_masked_weapon_strike_crit() {
    let mut standalone = Standalone::start("aura-charges-crit-modifier-strike");
    let wolf = stage(&mut standalone);
    cold_blood_fixture(&standalone);
    mage_spell(
        &standalone,
        MAGE_STRIKE,
        (E_WEAPON_STRIKE, T_TARGET_ENEMY, 100, P_RAW),
        &format!(
            "damage_class = {DAMAGE_CLASS_MELEE}, school_mask = {PHYSICAL}, \
             attributes = {ATTR_NO_ACTIVE_DEFENSE}, cast_flags = {CAST_FLAG_ALWAYS_HIT}"
        ),
    );

    let go = go_event(&standalone, PLAYER, MAGE_STRIKE, wolf);
    assert_eq!(
        go["is_crit"], "true",
        "vmangos RollMeleeOutcomeAgainst takes the crit band from GetSpellCritChance"
    );
    assert_eq!(charges(&standalone, PLAYER, COLD_BLOOD_FIXTURE), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn divine_favor_makes_the_next_holy_light_or_flash_of_light_crit_and_ends_with_it() {
    let mut standalone = Standalone::start("aura-charges-divine-favor");
    stage(&mut standalone);
    aura_spell(
        &standalone,
        Charged {
            spell_id: DIVINE_FAVOR,
            school_mask: HOLY,
            target: T_SELF,
            proc_flags: 0,
            charges: 1,
        },
        &[(
            A_SPELLMOD_FLAT,
            CERTAIN_CRIT_PERCENT,
            SPELLMOD_OP_CRITICAL_CHANCE,
            P_SPELLMOD_OP,
        )],
    );
    set_spell(
        &standalone,
        DIVINE_FAVOR,
        &format!("family_name = {SPELL_FAMILY_PALADIN}"),
    );
    standalone.assert_sql(&format!(
        "UPDATE game_spell_effect SET p_1 = {} WHERE spell_id = {DIVINE_FAVOR}",
        DIVINE_FAVOR_MASK as i32
    ));
    for (heal, family_flags) in [
        (FLASH_OF_LIGHT, FLASH_OF_LIGHT_FAMILY_FLAGS),
        (HOLY_LIGHT, HOLY_LIGHT_FAMILY_FLAGS),
    ] {
        effect_spell(
            &standalone,
            heal,
            (E_HEAL, T_SELF, 0, P_NONE),
            &format!(
                "family_name = {SPELL_FAMILY_PALADIN}, family_flags = {family_flags}, \
                 school_mask = {HOLY}, damage_class = {DAMAGE_CLASS_MAGIC}"
            ),
        );
    }

    for heal in [FLASH_OF_LIGHT, HOLY_LIGHT] {
        standalone.assert_call("debug_force_cast", &["1", &DIVINE_FAVOR.to_string()]);
        advance(&standalone);
        standalone.assert_sql(&format!(
            "UPDATE game_world_entity SET health = 1 WHERE guid = {PLAYER}"
        ));
        let go = go_event(&standalone, PLAYER, heal, PLAYER);
        assert_eq!(go["is_crit"], "true", "spell {heal}");
        assert_eq!(
            charges(&standalone, PLAYER, DIVINE_FAVOR),
            None,
            "spell {heal}"
        );
        advance(&standalone);
    }
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_swing_that_an_absorb_takes_part_of_raises_the_absorb_bit() {
    const CHARGES: u8 = 20;
    const ABSORB_POINTS: i32 = 1;
    let mut standalone = Standalone::start("aura-charges-partly-absorbed-swing");
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
        &[(A_ABSORB, ABSORB_POINTS, i32::from(PHYSICAL), P_SCHOOL_MASK)],
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
        &[("spell_id", &FLURRY), ("proc_ex", &PROC_EX_ABSORB)],
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
        let last = &swings[swung - 1];
        if HIT_INFO_AVOIDED.contains(&last["hit_info"].as_str()) {
            assert_eq!(
                charges(&standalone, PLAYER, FLURRY),
                Some(CHARGES),
                "an avoided swing raises no PROC_EX_ABSORB"
            );
            continue;
        }
        assert_ne!(
            last["damage"], "0",
            "the absorb takes only part of the swing"
        );
        assert_eq!(
            charges(&standalone, PLAYER, FLURRY),
            Some(CHARGES - 1),
            "vmangos Unit::CalculateMeleeDamage raises PROC_EX_ABSORB for a partly absorbed swing"
        );
        return;
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

fn spawn_wolf(standalone: &Standalone) -> u64 {
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
    wolf
}

fn mechanic_effect(
    standalone: &Standalone,
    spell_id: u32,
    (index, kind, base_points): (u8, u8, i32),
    (p0, p0_kind): (i32, u8),
    mechanic: u8,
) {
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, index)),
            ("spell_id", &spell_id),
            ("effect_index", &index),
            ("kind", &kind),
            ("base_points", &base_points),
            ("target", &T_TARGET_ENEMY),
            ("p_0", &p0),
            ("p_0_kind", &p0_kind),
            ("effect_mechanic", &mechanic),
        ],
    );
}

fn snare_only_spell(standalone: &Standalone) {
    aura_spell(
        standalone,
        Charged {
            spell_id: SNARE_FIXTURE,
            school_mask: FROST,
            target: T_TARGET_ENEMY,
            proc_flags: 0,
            charges: 0,
        },
        &[(
            A_MOD_SPEED,
            SLOW_POINTS,
            i32::from(SPEED_MOVE),
            P_SPEED_KIND,
        )],
    );
    standalone.assert_sql(&format!(
        "UPDATE game_spell_effect SET effect_mechanic = {MECHANIC_SNARE} \
         WHERE spell_id = {SNARE_FIXTURE}"
    ));
}

fn bleed_period(standalone: &Standalone, spell_id: u32) {
    standalone.assert_sql(&format!(
        "UPDATE game_spell_effect SET period_ms = {BLEED_PERIOD_MS} WHERE spell_id = {spell_id}"
    ));
}

fn bubble(standalone: &Standalone, spell_id: u32, target: u8) {
    aura_spell(
        standalone,
        Charged {
            spell_id: FORBEARANCE,
            school_mask: PHYSICAL,
            target: T_TARGET_ALLY,
            proc_flags: 0,
            charges: 0,
        },
        &[(
            A_IMMUNITY,
            0,
            i32::from(MECHANIC_INVULNERABILITY),
            P_MECHANIC,
        )],
    );
    standalone.assert_sql(&format!(
        "UPDATE game_spell_effect SET p_1 = {IMMUNE_TO_HOSTILE_AND_FRIENDLY} \
         WHERE spell_id = {FORBEARANCE}"
    ));
    set_spell(
        standalone,
        FORBEARANCE,
        &format!("attributes = {ATTR_NO_IMMUNITIES}, mechanic = {MECHANIC_INVULNERABILITY}"),
    );
    aura_spell(
        standalone,
        Charged {
            spell_id,
            school_mask: PHYSICAL,
            target,
            proc_flags: 0,
            charges: 0,
        },
        &[(A_MOD_RESISTANCE, 10, i32::from(RESIST_ARMOR), P_SCHOOL_MASK)],
    );
    set_spell(
        standalone,
        spell_id,
        &format!("is_negative = false, mechanic = {MECHANIC_INVULNERABILITY}"),
    );
    standalone.insert_row(
        "game_spell_follow_up",
        &[
            ("spell_id", &spell_id),
            ("follow_up_spell_id", &FORBEARANCE),
        ],
    );
}

fn assert_refused_as_immune(standalone: &Standalone, spell_id: u32, target: u64) {
    let output = standalone.call(
        "debug_begin_cast",
        &[
            &PLAYER.to_string(),
            &spell_id.to_string(),
            &target.to_string(),
        ],
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && text.contains("is immune to its mechanic"),
        "vmangos Spell::CheckCast refuses a helpful spell of an immune mechanic with TARGET_AURASTATE: {text}"
    );
}

fn spawn_ally(standalone: &Standalone) -> u64 {
    standalone.assert_call(
        "debug_create_package_character",
        &[r#""forbearance-test""#, r#""Ally""#, "1", "2"],
    );
    let ally: u64 = standalone
        .query_rows("SELECT guid FROM game_character WHERE name = 'Ally'")
        .remove(0)["guid"]
        .parse()
        .unwrap();
    standalone.assert_call("debug_spawn_player_entity", &[&ally.to_string()]);
    let player = standalone
        .query_rows(&format!(
            "SELECT x, y, z, map_id, instance_id FROM game_world_entity WHERE guid = {PLAYER}"
        ))
        .remove(0);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {}, y = {}, z = {}, map_id = {}, instance_id = {} \
         WHERE guid = {ally}",
        player["x"], player["y"], player["z"], player["map_id"], player["instance_id"]
    ));
    ally
}

fn interrupt_spell(standalone: &Standalone, spell_id: u32) {
    bolt(standalone, spell_id, PHYSICAL, 0);
    mechanic_effect(
        standalone,
        spell_id,
        (1, E_INTERRUPT, 0),
        (0, P_NONE),
        MECHANIC_INTERRUPT,
    );
}

fn wolf_begins_a_cast(standalone: &Standalone, wolf: u64) {
    if standalone
        .query_rows(&format!(
            "SELECT spell_id FROM game_spell WHERE spell_id = {WOLF_CAST}"
        ))
        .is_empty()
    {
        bolt(standalone, WOLF_CAST, FIRE, 0);
        set_spell(standalone, WOLF_CAST, "cast_time_ms = 10000");
    }
    standalone.assert_call(
        "debug_begin_cast",
        &[
            &wolf.to_string(),
            &WOLF_CAST.to_string(),
            &PLAYER.to_string(),
        ],
    );
    assert!(is_casting(standalone, wolf));
}

fn is_casting(standalone: &Standalone, caster: u64) -> bool {
    !standalone
        .query_rows(&format!(
            "SELECT spell_id FROM game_pending_cast WHERE caster_guid = {caster}"
        ))
        .is_empty()
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .unwrap()
}

fn lockouts(standalone: &Standalone, caster: u64) -> Vec<String> {
    standalone
        .query_rows(&format!(
            "SELECT school FROM game_school_lockout WHERE caster_guid = {caster}"
        ))
        .into_iter()
        .map(|row| row["school"].clone())
        .collect()
}

fn go_event(standalone: &Standalone, caster: u64, spell_id: u32, target: u64) -> support::Row {
    let events = subscribe_with_fence(
        standalone,
        &[&format!(
            "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {spell_id} AND kind = 2"
        )],
    );
    standalone.assert_call(
        "debug_cast_at",
        &[
            &caster.to_string(),
            &spell_id.to_string(),
            &target.to_string(),
        ],
    );
    fence_subscription(standalone, &events);
    event_rows(&events, "game_spell_cast_event_live")
        .pop()
        .expect("the cast completes")
}

fn set_wolf_mechanic_immune_mask(standalone: &Standalone, mask: u32) {
    standalone.assert_sql(&format!(
        "UPDATE game_creature_template SET mechanic_immune_mask = {mask} \
         WHERE entry = {TEST_WOLF_ENTRY}"
    ));
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
    let wolf = spawn_wolf(standalone);
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

fn mage_modifier(standalone: &Standalone, spell_id: u32, amount: i32, op: i32) {
    mage_modifier_of_kind(standalone, spell_id, (A_SPELLMOD_PCT, amount, op));
}

fn cold_blood_fixture(standalone: &Standalone) {
    mage_modifier_of_kind(
        standalone,
        COLD_BLOOD_FIXTURE,
        (
            A_SPELLMOD_FLAT,
            CERTAIN_CRIT_PERCENT,
            SPELLMOD_OP_CRITICAL_CHANCE,
        ),
    );
    standalone.assert_call("debug_force_cast", &["1", &COLD_BLOOD_FIXTURE.to_string()]);
    advance(standalone);
}

fn mage_modifier_of_kind(
    standalone: &Standalone,
    spell_id: u32,
    (kind, amount, op): (u8, i32, i32),
) {
    aura_spell(
        standalone,
        Charged {
            spell_id,
            school_mask: FIRE,
            target: T_SELF,
            proc_flags: 0,
            charges: 1,
        },
        &[(kind, amount, op, P_SPELLMOD_OP)],
    );
    set_spell(
        standalone,
        spell_id,
        &format!("family_name = {SPELL_FAMILY_MAGE}"),
    );
    standalone.assert_sql(&format!(
        "UPDATE game_spell_effect SET p_1 = {MAGE_BOLT_FAMILY_FLAG} WHERE spell_id = {spell_id}"
    ));
}

fn mage_bolt(standalone: &Standalone, spell_id: u32, family_flag: i32, assignments: &str) {
    bolt(standalone, spell_id, FIRE, 0);
    set_spell(
        standalone,
        spell_id,
        &format!("family_name = {SPELL_FAMILY_MAGE}, family_flags = {family_flag}, {assignments}"),
    );
}

fn mage_spell(
    standalone: &Standalone,
    spell_id: u32,
    effect: (u8, u8, i32, u8),
    assignments: &str,
) {
    effect_spell(
        standalone,
        spell_id,
        effect,
        &format!(
            "family_name = {SPELL_FAMILY_MAGE}, family_flags = {MAGE_BOLT_FAMILY_FLAG}, \
             {assignments}"
        ),
    );
}

// Sets no family, as a SpacetimeDB UPDATE that assigns a column twice keeps the first value.
fn effect_spell(
    standalone: &Standalone,
    spell_id: u32,
    (kind, target, p0, p0_kind): (u8, u8, i32, u8),
    assignments: &str,
) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &format!("Mage fixture {spell_id}")),
            ("school_mask", &FIRE),
            ("range_yd", &30),
            ("is_negative", &(target != T_SELF)),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, 0)),
            ("spell_id", &spell_id),
            ("kind", &kind),
            ("base_points", &100),
            ("target", &target),
            ("p_0", &p0),
            ("p_0_kind", &p0_kind),
        ],
    );
    set_spell(standalone, spell_id, assignments);
}

fn use_mana(standalone: &Standalone) {
    let bytes: u32 = standalone
        .query_rows(&format!(
            "SELECT unit_bytes_0 FROM game_world_entity WHERE guid = {PLAYER}"
        ))
        .remove(0)["unit_bytes_0"]
        .parse()
        .expect("unit_bytes_0 is a number");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_bytes_0 = {}, power = {MANA}, max_power = {MANA} \
         WHERE guid = {PLAYER}",
        bytes & 0x00FF_FFFF
    ));
}

fn mana(standalone: &Standalone) -> u32 {
    standalone.query_rows(&format!(
        "SELECT power FROM game_world_entity WHERE guid = {PLAYER}"
    ))[0]["power"]
        .parse()
        .expect("power is a number")
}

fn begin_cast(standalone: &Standalone, spell_id: u32, target: u64) {
    standalone.assert_call(
        "debug_begin_cast",
        &[
            &PLAYER.to_string(),
            &spell_id.to_string(),
            &target.to_string(),
        ],
    );
}

fn real_fear_ward(standalone: &Standalone) {
    aura_spell(
        standalone,
        Charged {
            spell_id: FEAR_WARD,
            school_mask: PHYSICAL,
            target: T_SELF,
            proc_flags: TAKE_HARMFUL_ABILITY | TAKE_HARMFUL_SPELL,
            charges: 1,
        },
        &[(A_IMMUNITY, 0, i32::from(MECHANIC_FEAR), P_MECHANIC)],
    );
    standalone.assert_call("debug_force_cast", &["1", &FEAR_WARD.to_string()]);
    advance(standalone);
}

fn cast_miss_code(standalone: &Standalone, caster: u64, spell_id: u32, target: u64) -> String {
    let events = subscribe_with_fence(
        standalone,
        &[&format!(
            "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {spell_id} AND kind = 2"
        )],
    );
    cast_by(standalone, caster, spell_id, target);
    fence_subscription(standalone, &events);
    event_rows(&events, "game_spell_cast_event_live")[0]["miss_info"].clone()
}

fn control_spell(standalone: &Standalone, spell_id: u32, control: i32, mechanic: u8) {
    aura_spell(
        standalone,
        Charged {
            spell_id,
            school_mask: NATURE,
            target: T_TARGET_ENEMY,
            proc_flags: 0,
            charges: 0,
        },
        &[(A_CONTROL, 0, control, P_MECHANIC)],
    );
    set_spell(standalone, spell_id, &format!("mechanic = {mechanic}"));
}

fn control_spell_by_effect(standalone: &Standalone, spell_id: u32, control: i32, mechanic: u8) {
    control_spell(standalone, spell_id, control, 0);
    standalone.assert_sql(&format!(
        "UPDATE game_spell_effect SET effect_mechanic = {mechanic} WHERE spell_id = {spell_id}"
    ));
}

fn add_damage_effect(standalone: &Standalone, spell_id: u32) {
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, 1)),
            ("spell_id", &spell_id),
            ("effect_index", &1u8),
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

fn proc_ready_micros(standalone: &Standalone) -> i64 {
    standalone.query_rows(&format!(
        "SELECT proc_ready_micros FROM game_aura WHERE target_guid = {PLAYER} \
         AND spell_id = {REAGENT_PROC_AURA}"
    ))[0]["proc_ready_micros"]
        .parse()
        .unwrap()
}

fn item_stack(standalone: &Standalone, entry: u32) -> Option<u32> {
    standalone
        .query_rows(&format!(
            "SELECT stack_count FROM game_item_instance WHERE owner_guid = {PLAYER} \
             AND entry = {entry}"
        ))
        .first()
        .map(|row| row["stack_count"].parse().unwrap())
}

fn insight(standalone: &Standalone) {
    aura_spell(
        standalone,
        Charged {
            spell_id: INSIGHT,
            school_mask: ARCANE,
            target: T_SELF,
            proc_flags: DEAL_HARMFUL_SPELL,
            charges: 1,
        },
        &[(
            A_MOD_POWER_COST_FLAT,
            INSIGHT_COST,
            i32::from(INSIGHT_SCHOOLS),
            P_SCHOOL_MASK,
        )],
    );
}

fn sends_power_cost_field(wire: &mut ReducerConnection, field: u16, value: u32) -> bool {
    let carries = |packet: &NativePacket| {
        packet.opcode == Some(SMSG_UPDATE_OBJECT)
            && parse_values_updates(&packet.body)
                .iter()
                .any(|update| update.guid == PLAYER && update.fields.contains(&(field, value)))
    };
    let deadline = Instant::now() + PACKET_WAIT;
    while Instant::now() < deadline {
        if wire
            .wait_for_packets(Duration::from_secs(1))
            .unwrap()
            .iter()
            .any(carries)
        {
            return true;
        }
    }
    false
}

fn charges(standalone: &Standalone, carrier: u64, spell_id: u32) -> Option<u8> {
    aura_column(standalone, carrier, spell_id, "proc_charges")
}

fn stacks(standalone: &Standalone, spell_id: u32) -> Option<u8> {
    aura_column(standalone, PLAYER, spell_id, "stacks")
}
