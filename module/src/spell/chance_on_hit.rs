use lyracore_shared::constants::starter_item::{CLASS_WEAPON, MAINHAND_SLOT};
use lyracore_shared::spell::taxonomy::{STANCE_BEAR, STANCE_DIRE_BEAR};
use spacetimedb::{log, ReducerContext};

use crate::game_world_entity;
use crate::items::{game_item_template, ItemTemplate};

use super::game_spell;
use super::proc::{proc_chance_bp, ProcProfile, ProcSide, BASIS_POINTS};

// vmangos ITEM_SPELLTRIGGER_CHANCE_ON_HIT.
const SPELLTRIGGER_CHANCE_ON_HIT: u8 = 2;
// vmangos rolls a spell whose procChance exceeds 100 at one proc per minute.
const UNKNOWN_RATE_PPM: f32 = 1.0;

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
    let Some(weapon) = weapon_in_slot(ctx, attacker_guid, slot) else {
        return;
    };
    let spells = chance_on_hit_spells(&weapon);
    if spells.is_empty()
        || !holds_weapon(attacker.stance, slot, || {
            crate::spell::is_disarmed(ctx, attacker_guid)
        })
    {
        return;
    }
    for (spell_id, ppm) in spells {
        let Some(spell) = ctx.db.game_spell().spell_id().find(spell_id) else {
            log::info!(
                "chance on hit: item {} names unknown spell {spell_id}",
                weapon.entry
            );
            continue;
        };
        let roll_bp = ctx.random::<u32>() % BASIS_POINTS;
        if roll_bp >= chance_bp(spell.proc_chance, ppm, weapon.delay_ms) {
            continue;
        }
        if let Err(err) = super::cast::resolve::cast_chance_on_hit(
            ctx,
            attacker_guid,
            spell_id,
            attacker.level as u8,
            target_guid,
        ) {
            log::info!(
                "chance on hit: item {} could not cast {spell_id}: {err}",
                weapon.entry
            );
        }
    }
}

// vmangos CanUseEquippedWeapon: a feral form sets weapons aside and Disarm takes the main hand.
fn holds_weapon(stance: u8, slot: u8, disarmed: impl FnOnce() -> bool) -> bool {
    if (STANCE_BEAR..=STANCE_DIRE_BEAR).contains(&stance) {
        return false;
    }
    slot != MAINHAND_SLOT || !disarmed()
}

fn weapon_in_slot(ctx: &ReducerContext, player_guid: u64, slot: u8) -> Option<ItemTemplate> {
    let item = crate::items::item_in_slot(ctx, player_guid, slot)?;
    let weapon = ctx.db.game_item_template().entry().find(item.entry)?;
    (weapon.class == CLASS_WEAPON && !crate::items::item_is_broken(&weapon, &item))
        .then_some(weapon)
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
    fn without_a_rate_the_spell_chance_rolls_as_a_flat_percent() {
        assert_eq!(chance_bp(15, 0.0, 2400), 1500);
        assert_eq!(chance_bp(0, 0.0, 2400), 0);
    }
}
