use lyracore_shared::constants::starter_item::{CLASS_WEAPON, MAINHAND_SLOT};
use lyracore_shared::item_property::COMBAT_SPELL;
use lyracore_shared::spell::taxonomy::{
    STANCE_BEAR, STANCE_DIRE_BEAR, T_AREA_ENEMY, T_CHAIN_ENEMY, T_TARGET_ENEMY,
};
use spacetimedb::{log, table, ReducerContext};

use crate::game_world_entity;
use crate::items::{
    game_item_enchantment, game_item_random_property, game_item_template, ItemInstance,
    ItemTemplate,
};

use super::proc::{proc_chance_bp, ProcProfile, ProcSide, BASIS_POINTS};
use super::{game_spell, game_spell_chain, game_spell_effect};

// vmangos ITEM_SPELLTRIGGER_CHANCE_ON_HIT.
const SPELLTRIGGER_CHANCE_ON_HIT: u8 = 2;
// vmangos rolls a spell whose procChance exceeds 100 at one proc per minute.
const UNKNOWN_RATE_PPM: f32 = 1.0;

// vmangos spell_proc_item_enchant: procs per minute of an enchantment's Chance on Hit spell.
#[table(accessor = game_enchant_proc_rate)]
pub struct EnchantProcRate {
    #[primary_key]
    pub spell_id: u32,
    pub ppm_rate: f32,
}

// Follows vmangos Player::CastItemCombatSpell. The caller picks the slot: the hand that swung,
// or the main hand for a spell that triggers weapon procs.
pub(crate) fn roll_chance_on_hit(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    slot: u8,
) {
    if attacker_guid == target_guid {
        return;
    }
    let entities = ctx.db.game_world_entity();
    let Some(attacker) = entities.guid().find(attacker_guid) else {
        return;
    };
    if !attacker.is_player() || entities.guid().find(target_guid).is_none_or(|t| t.dead) {
        return;
    }
    let Some((item, weapon)) = weapon_in_slot(ctx, attacker_guid, slot) else {
        return;
    };
    let spells = chance_on_hit_spells(&weapon);
    let enchants = combat_spell_enchants(ctx, &item);
    if (spells.is_empty() && enchants.is_empty())
        || !holds_weapon(attacker.stance, slot, || {
            crate::spell::is_disarmed(ctx, attacker_guid)
        })
    {
        return;
    }
    for (spell_id, ppm) in spells {
        let Some(spell) = known_spell(ctx, spell_id, || format!("item {}", weapon.entry)) else {
            continue;
        };
        let chance_bp = chance_bp(spell.proc_chance, ppm, weapon.delay_ms);
        cast_on_roll(ctx, &attacker, spell_id, chance_bp, target_guid);
    }
    for (enchant_id, spell_id, amount) in enchants {
        let Some(spell) = known_spell(ctx, spell_id, || format!("enchantment {enchant_id}")) else {
            continue;
        };
        let chance_bp = enchant_chance_bp(enchant_rate(ctx, spell_id), amount, weapon.delay_ms);
        let effect_targets = ctx
            .db
            .game_spell_effect()
            .by_spell()
            .filter(&spell_id)
            .map(|effect| effect.target);
        let cast_target = if helps_wielder(spell.is_negative, effect_targets) {
            attacker_guid
        } else {
            target_guid
        };
        cast_on_roll(ctx, &attacker, spell_id, chance_bp, cast_target);
    }
}

fn cast_on_roll(
    ctx: &ReducerContext,
    attacker: &crate::WorldEntity,
    spell_id: u32,
    chance_bp: u32,
    cast_target: u64,
) {
    if ctx.random::<u32>() % BASIS_POINTS >= chance_bp {
        return;
    }
    if let Err(err) = super::cast::resolve::cast_chance_on_hit(
        ctx,
        attacker.guid,
        spell_id,
        attacker.level as u8,
        cast_target,
    ) {
        log::info!("chance on hit: could not cast {spell_id}: {err}");
    }
}

fn known_spell(
    ctx: &ReducerContext,
    spell_id: u32,
    source: impl FnOnce() -> String,
) -> Option<super::Spell> {
    let spell = ctx.db.game_spell().spell_id().find(spell_id);
    if spell.is_none() {
        log::info!("chance on hit: {} names unknown spell {spell_id}", source());
    }
    spell
}

// vmangos keys the rate on the first rank and gives it to every higher rank.
fn enchant_rate(ctx: &ReducerContext, spell_id: u32) -> f32 {
    let first_rank = ctx
        .db
        .game_spell_chain()
        .spell_id()
        .find(spell_id)
        .map_or(spell_id, |chain| chain.first_spell);
    ctx.db
        .game_enchant_proc_rate()
        .spell_id()
        .find(first_rank)
        .map_or(0.0, |rate| rate.ppm_rate)
}

// vmangos IsPositiveSpell: an enemy-targeted effect makes the spell harmful even without a
// harmful aura, so Life Steal lands on the victim.
fn helps_wielder(is_negative: bool, effect_targets: impl IntoIterator<Item = u8>) -> bool {
    !is_negative
        && effect_targets
            .into_iter()
            .all(|target| !matches!(target, T_TARGET_ENEMY | T_AREA_ENEMY | T_CHAIN_ENEMY))
}

// vmangos CanUseEquippedWeapon: a feral form sets weapons aside and Disarm takes the main hand.
fn holds_weapon(stance: u8, slot: u8, disarmed: impl FnOnce() -> bool) -> bool {
    if (STANCE_BEAR..=STANCE_DIRE_BEAR).contains(&stance) {
        return false;
    }
    slot != MAINHAND_SLOT || !disarmed()
}

fn weapon_in_slot(
    ctx: &ReducerContext,
    player_guid: u64,
    slot: u8,
) -> Option<(ItemInstance, ItemTemplate)> {
    let item = crate::items::item_in_slot(ctx, player_guid, slot)?;
    let weapon = ctx.db.game_item_template().entry().find(item.entry)?;
    (weapon.class == CLASS_WEAPON && !crate::items::item_is_broken(&weapon, &item))
        .then_some((item, weapon))
}

// The permanent enchantment and the Random Property enchantments, as vmangos walks every slot.
fn combat_spell_enchants(ctx: &ReducerContext, item: &ItemInstance) -> Vec<(u32, u32, i32)> {
    let property = ctx
        .db
        .game_item_random_property()
        .property_id()
        .find(item.random_property_id);
    let enchant_ids = [item.enchant_id]
        .into_iter()
        .chain(property.into_iter().flat_map(|property| {
            [
                property.enchant_id_1,
                property.enchant_id_2,
                property.enchant_id_3,
            ]
        }))
        .filter(|&id| id != 0);
    enchant_ids
        .flat_map(|id| ctx.db.game_item_enchantment().by_enchant().filter(id))
        .filter(|effect| effect.kind == COMBAT_SPELL && effect.spell_id != 0)
        .map(|effect| (effect.enchant_id, effect.spell_id, effect.amount))
        .collect()
}

fn chance_on_hit_spells(weapon: &ItemTemplate) -> Vec<(u32, f32)> {
    [
        (
            weapon.spellid_1,
            weapon.spelltrigger_1,
            weapon.spellppmrate_1,
        ),
        (
            weapon.spellid_2,
            weapon.spelltrigger_2,
            weapon.spellppmrate_2,
        ),
        (
            weapon.spellid_3,
            weapon.spelltrigger_3,
            weapon.spellppmrate_3,
        ),
        (
            weapon.spellid_4,
            weapon.spelltrigger_4,
            weapon.spellppmrate_4,
        ),
        (
            weapon.spellid_5,
            weapon.spelltrigger_5,
            weapon.spellppmrate_5,
        ),
    ]
    .into_iter()
    .filter(|&(spell_id, trigger, _)| spell_id != 0 && trigger == SPELLTRIGGER_CHANCE_ON_HIT)
    .map(|(spell_id, _, ppm)| (spell_id, ppm))
    .collect()
}

fn chance_bp(proc_chance: u8, ppm: f32, weapon_delay_ms: u32) -> u32 {
    // vmangos takes any nonzero item rate, and a negative rate never procs.
    if ppm < 0.0 {
        return 0;
    }
    let ppm = if ppm > 0.0 {
        ppm
    } else if proc_chance > 100 {
        UNKNOWN_RATE_PPM
    } else {
        0.0
    };
    let profile = ProcProfile {
        chance: proc_chance,
        ppm,
        ..ProcProfile::default()
    };
    proc_chance_bp(&profile, ProcSide::Dealer, weapon_delay_ms)
}

// vmangos takes the rate table first, then the enchantment's own percent, then one proc per minute.
fn enchant_chance_bp(ppm: f32, amount: i32, weapon_delay_ms: u32) -> u32 {
    if ppm != 0.0 {
        chance_bp(0, ppm, weapon_delay_ms)
    } else if amount != 0 {
        chance_bp(amount.clamp(0, 100) as u8, 0.0, weapon_delay_ms)
    } else {
        chance_bp(0, UNKNOWN_RATE_PPM, weapon_delay_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::spell::taxonomy::{STANCE_BATTLE, STANCE_BERSERKER, STANCE_CAT};

    const OFFHAND: u8 = crate::items::equip_slot::OFFHAND;

    #[test]
    fn a_feral_form_sets_both_weapons_aside() {
        for stance in [STANCE_BEAR, STANCE_CAT, STANCE_DIRE_BEAR] {
            assert!(!holds_weapon(stance, MAINHAND_SLOT, || false));
            assert!(!holds_weapon(stance, OFFHAND, || false));
        }
        assert!(holds_weapon(STANCE_BERSERKER, MAINHAND_SLOT, || false));
    }

    #[test]
    fn disarm_takes_only_the_main_hand() {
        assert!(!holds_weapon(STANCE_BATTLE, MAINHAND_SLOT, || true));
        assert!(holds_weapon(STANCE_BATTLE, OFFHAND, || true));
    }

    #[test]
    fn only_chance_on_hit_slots_roll_and_each_keeps_its_rate() {
        let weapon = ItemTemplate {
            spellid_1: 21992,
            spelltrigger_1: SPELLTRIGGER_CHANCE_ON_HIT,
            spellppmrate_1: 8.0,
            spellid_2: 7597,
            spelltrigger_2: 1,
            spellid_3: 0,
            spelltrigger_3: SPELLTRIGGER_CHANCE_ON_HIT,
            spellid_5: 18104,
            spelltrigger_5: SPELLTRIGGER_CHANCE_ON_HIT,
            ..crate::seed::base_item(19019, "Thunderfury, Blessed Blade of the Windseeker")
        };
        assert_eq!(chance_on_hit_spells(&weapon), [(21992, 8.0), (18104, 0.0)]);
    }

    #[test]
    fn a_rate_scales_with_the_weapon_delay() {
        // Thunderfury at patch 10: 8 procs per minute on a 1.9 second blade is 25.33 percent.
        assert_eq!(chance_bp(101, 8.0, 1900), 2533);
    }

    #[test]
    fn a_negative_rate_never_procs() {
        assert_eq!(chance_bp(101, -1.0, 2400), 0);
    }

    #[test]
    fn a_spell_chance_above_certainty_rolls_one_proc_per_minute() {
        // One proc per minute on a 2.4 second weapon is 4 percent.
        assert_eq!(chance_bp(101, 0.0, 2400), 400);
    }

    #[test]
    fn an_enchantment_takes_its_rate_then_its_percent_then_one_proc_per_minute() {
        // Fiery Weapon: 6 procs per minute on a 2.4 second weapon is 24 percent.
        assert_eq!(enchant_chance_bp(6.0, 15, 2400), 2400);
        assert_eq!(enchant_chance_bp(0.0, 15, 2400), 1500);
        assert_eq!(enchant_chance_bp(0.0, 0, 2400), 400);
        assert_eq!(enchant_chance_bp(-1.0, 15, 2400), 0);
        assert_eq!(enchant_chance_bp(0.0, -5, 2400), 0);
    }

    #[test]
    fn a_helpful_spell_lands_on_the_wielder_unless_an_effect_aims_at_an_enemy() {
        use lyracore_shared::spell::taxonomy::T_SELF;
        // Holy Strength buffs the wielder; Life Steal drains the victim though no aura marks it.
        assert!(helps_wielder(false, [T_SELF]));
        assert!(!helps_wielder(false, [T_TARGET_ENEMY]));
        assert!(!helps_wielder(false, [T_SELF, T_AREA_ENEMY]));
        assert!(!helps_wielder(true, [T_SELF]));
    }

    #[test]
    fn without_a_rate_the_spell_chance_rolls_as_a_flat_percent() {
        assert_eq!(chance_bp(15, 0.0, 2400), 1500);
        assert_eq!(chance_bp(0, 0.0, 2400), 0);
    }
}
