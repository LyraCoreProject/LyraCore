use lyracore_shared::constants::{player_flags, realm_type, unit_flags};
use lyracore_shared::faction;
use spacetimedb::{table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{game_area, game_realm, game_world_entity, GameArea, WorldEntity};

// vmangos Player::UpdatePvP restarts the countdown at 300 000 ms.
const PVP_FLAG_MS: u64 = 300_000;
// DBCEnums.h AREA_FLAG_CAPITAL and the AreaTable.dbc team values.
pub(crate) const AREA_FLAG_CAPITAL: u32 = 0x0000_0100;
const AREA_TEAM_NONE: u32 = 0;
const AREA_TEAM_ALLIANCE: u32 = 2;
const AREA_TEAM_HORDE: u32 = 4;

/// The countdown that drops a Character's PvP flag; [`expire_pvp_flag`] re-arms a frozen one.
#[table(accessor = game_pvp_flag_timer, scheduled(expire_pvp_flag))]
pub struct PvpFlagTimer {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    #[unique]
    pub character_guid: u64,
    pub scheduled_at: ScheduleAt,
    /// The last PvP combat pulse in ms, zeroed when a later combat begins; vmangos `inPvPCombat`.
    pub pvp_combat_ms: u64,
}

crate::outside_table_catalog!(game_pvp_flag_timer);

crate::character_owned!(delete, fn sweep_delete_game_pvp_flag_timer(ctx, character_guid) {
    ctx.db.game_pvp_flag_timer().character_guid().delete(character_guid);
});
// Transfer rebuilds the Character's entity without its PvP flag.
crate::character_owned!(not_transported, fn sweep_transfer_game_pvp_flag_timer());

pub(crate) fn is_flagged(unit: &WorldEntity) -> bool {
    unit.unit_flags & unit_flags::PVP != 0
}

/// The Character a unit acts for, as vmangos `GetAffectingPlayer`: itself or its owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Combatant {
    pub(crate) guid: u64,
    pub(crate) flagged: bool,
}

pub(crate) fn affecting_character(ctx: &ReducerContext, unit: &WorldEntity) -> Option<Combatant> {
    let of = |character: &WorldEntity| Combatant {
        guid: character.guid,
        flagged: is_flagged(character),
    };
    if unit.is_player() {
        return Some(of(unit));
    }
    if unit.owner_guid == 0 {
        return None;
    }
    ctx.db
        .game_world_entity()
        .guid()
        .find(unit.owner_guid)
        .filter(WorldEntity::is_player)
        .map(|owner| of(&owner))
}

/// vmangos `WorldSession::HandleTogglePvP`: sets `PVP_DESIRED` to `desired`, or toggles it.
pub(crate) fn toggle(ctx: &ReducerContext, character_guid: u64, desired: Option<bool>) {
    let Some(mut character) = ctx.db.game_world_entity().guid().find(character_guid) else {
        return;
    };
    let desired = desired.unwrap_or(character.player_flags & player_flags::PVP_DESIRED == 0);
    if desired {
        character.player_flags |= player_flags::PVP_DESIRED;
        refresh(ctx, &mut character, false);
    } else {
        character.player_flags &= !player_flags::PVP_DESIRED;
        if is_flagged(&character) {
            let pvp_combat_ms = pvp_combat_ms(ctx, character_guid);
            arm(ctx, &character, pvp_combat_ms);
        }
    }
    character.update(ctx);
}

/// vmangos `Player::UpdateZone`: flags a Character in a PvP-enforced zone unless it flies a taxi.
/// Returns whether the zone enforces PvP; the caller saves `character`.
pub(crate) fn update_zone(
    ctx: &ReducerContext,
    character: &mut WorldEntity,
    zone: Option<&GameArea>,
) -> bool {
    let enforced = zone.is_some_and(|zone| in_enforced_zone(ctx, character, zone));
    if enforced && !character.is_flying() {
        refresh(ctx, character, false);
    }
    enforced
}

/// Flags a Character entering the world in a PvP-enforced zone before viewers build its CREATE.
pub(crate) fn enter_world(ctx: &ReducerContext, character: &mut WorldEntity) {
    if !character.is_flying() && in_enforced_area(ctx, character) {
        refresh(ctx, character, false);
    }
}

/// vmangos `Player::UpdateZone`: an enemy capital enforces PvP, and on a PvP Realm so does every
/// zone the Character's team does not control.
fn in_enforced_zone(ctx: &ReducerContext, character: &WorldEntity, zone: &GameArea) -> bool {
    let team = faction::team_for_race(character.race());
    let enemy_ground =
        |owner: u32| team != owner && (zone.flags & AREA_FLAG_CAPITAL != 0 || is_pvp_realm(ctx));
    match zone.faction_group {
        AREA_TEAM_ALLIANCE => enemy_ground(faction::TEAM_ALLIANCE),
        AREA_TEAM_HORDE => enemy_ground(faction::TEAM_HORDE),
        AREA_TEAM_NONE => is_pvp_realm(ctx),
        _ => false,
    }
}

fn is_pvp_realm(ctx: &ReducerContext) -> bool {
    ctx.db.game_realm().iter().any(|realm| {
        matches!(
            realm.realm_type,
            realm_type::PVP | realm_type::RP_PVP | realm_type::FFA_PVP
        )
    })
}

fn in_enforced_area(ctx: &ReducerContext, character: &WorldEntity) -> bool {
    crate::terrain::area_at(ctx, character.map_id, character.x, character.y)
        .and_then(|area| ctx.db.game_area().id().find(crate::terrain::zone_of(&area)))
        .is_some_and(|zone| in_enforced_zone(ctx, character, &zone))
}

/// vmangos `Unit::TogglePlayerPvPFlagOnAttackVictim` and `Unit::SetInCombatWithAggressor`: harm
/// to a flagged unit flags the attacker's Character, and harm from a flagged one keeps the victim's
/// Character flagged. Neither counts between one Character's units or Duel opponents.
pub(crate) fn harmed(ctx: &ReducerContext, attacker: &WorldEntity, victim: &WorldEntity) {
    let by = affecting_character(ctx, attacker);
    let of = affecting_character(ctx, victim);
    let rivals = |by: Combatant, of: Combatant| {
        by.guid != of.guid && !crate::duel::active_opponents(ctx, by.guid, of.guid)
    };
    if let Some(by) = by {
        let victim_flagged = is_flagged(victim) || of.is_some_and(|of| of.flagged);
        if victim_flagged && of.is_none_or(|of| rivals(by, of)) {
            pulse(ctx, by.guid, true);
        }
    }
    if let (Some(by), Some(of)) = (by, of) {
        if (is_flagged(attacker) || by.flagged) && rivals(by, of) {
            pulse(ctx, of.guid, true);
        }
    }
}

/// vmangos `Unit::SetInCombatWithAssisted` and `Spell::DoAllEffectOnTarget`: help to another
/// flagged Character flags the helper's Character, which joins the target's PvP combat.
pub(crate) fn helped(ctx: &ReducerContext, helper: &WorldEntity, target: &WorldEntity) {
    let (Some(by), Some(of)) = (
        affecting_character(ctx, helper),
        affecting_character(ctx, target),
    ) else {
        return;
    };
    if by.guid == of.guid || !(is_flagged(target) || of.flagged) {
        return;
    }
    let joins_pvp_combat = in_combat(target) && pvp_combat_ms(ctx, of.guid) != 0;
    pulse(ctx, by.guid, joins_pvp_combat);
}

/// vmangos `Spell::DoAllEffectOnTarget`: an effect harms a unit its caster is not friendly to, and
/// helps a friendly one only through a helpful target kind.
pub(crate) fn spell_hit(
    ctx: &ReducerContext,
    caster_guid: u64,
    target_guid: u64,
    helpful_target: bool,
) {
    let entities = ctx.db.game_world_entity();
    let (Some(caster), Some(target)) = (
        entities.guid().find(caster_guid),
        entities.guid().find(target_guid),
    ) else {
        return;
    };
    match crate::combat::relation(ctx, &caster, &target) {
        crate::combat::Relation::SelfUnit => {}
        crate::combat::Relation::Friendly if helpful_target => helped(ctx, &caster, &target),
        crate::combat::Relation::Friendly => {}
        crate::combat::Relation::Neutral | crate::combat::Relation::Hostile => {
            harmed(ctx, &caster, &target)
        }
    }
}

/// A new combat ends the PvP combat of an earlier pulse; a pulse in this transaction stays.
pub(crate) fn combat_began(ctx: &ReducerContext, character_guid: u64) {
    let timers = ctx.db.game_pvp_flag_timer();
    let Some(mut timer) = timers.character_guid().find(character_guid) else {
        return;
    };
    if timer.pvp_combat_ms == 0 || timer.pvp_combat_ms >= crate::time::now_ms(ctx) {
        return;
    }
    timer.pvp_combat_ms = 0;
    timers.scheduled_id().update(timer);
}

/// vmangos `Player::UpdatePvPFlagTimer`: the flag drops when the countdown ends, unless the
/// Character wants PvP, fights in PvP combat or stands in a PvP-enforced zone.
#[spacetimedb::reducer]
pub fn expire_pvp_flag(ctx: &ReducerContext, timer: PvpFlagTimer) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    ctx.db
        .game_pvp_flag_timer()
        .scheduled_id()
        .delete(timer.scheduled_id);
    let Some(mut character) = ctx.db.game_world_entity().guid().find(timer.character_guid) else {
        return;
    };
    if !is_flagged(&character) {
        return;
    }
    let frozen = (timer.pvp_combat_ms != 0 && in_combat(&character))
        || character.player_flags & player_flags::PVP_DESIRED != 0
        || in_enforced_area(ctx, &character);
    if frozen {
        arm(ctx, &character, timer.pvp_combat_ms);
        return;
    }
    character.unit_flags &= !unit_flags::PVP;
    character.update(ctx);
}

fn pulse(ctx: &ReducerContext, character_guid: u64, pvp_combat: bool) {
    let Some(mut character) = ctx.db.game_world_entity().guid().find(character_guid) else {
        return;
    };
    let was_flagged = is_flagged(&character);
    refresh(ctx, &mut character, pvp_combat);
    if !was_flagged {
        character.update(ctx);
    }
}

/// vmangos `Player::UpdatePvP(true)`: flags `character` and restarts its countdown; the caller
/// saves `character`.
fn refresh(ctx: &ReducerContext, character: &mut WorldEntity, pvp_combat: bool) {
    character.unit_flags |= unit_flags::PVP;
    let pvp_combat_ms = if pvp_combat {
        crate::time::now_ms(ctx)
    } else {
        pvp_combat_ms(ctx, character.guid)
    };
    arm(ctx, character, pvp_combat_ms);
}

fn pvp_combat_ms(ctx: &ReducerContext, character_guid: u64) -> u64 {
    ctx.db
        .game_pvp_flag_timer()
        .character_guid()
        .find(character_guid)
        .map_or(0, |timer| timer.pvp_combat_ms)
}

fn arm(ctx: &ReducerContext, character: &WorldEntity, pvp_combat_ms: u64) {
    let timers = ctx.db.game_pvp_flag_timer();
    timers.character_guid().delete(character.guid);
    let start_ms = countdown_start_ms(
        crate::time::now_ms(ctx),
        pvp_combat_ms,
        in_combat(character),
        character.combat_until_ms,
    );
    let due_micros = start_ms.saturating_add(PVP_FLAG_MS).saturating_mul(1000);
    let due = Timestamp::from_micros_since_unix_epoch(due_micros.min(i64::MAX as u64) as i64);
    timers.insert(PvpFlagTimer {
        scheduled_id: 0,
        character_guid: character.guid,
        scheduled_at: crate::time::schedule_time(ctx, due),
        pvp_combat_ms,
    });
}

/// vmangos freezes the countdown in PvP combat; a pulse now keeps combat for `COMBAT_DROP_MS`.
fn countdown_start_ms(
    now_ms: u64,
    pvp_combat_ms: u64,
    in_combat: bool,
    combat_until_ms: u64,
) -> u64 {
    if pvp_combat_ms == 0 {
        now_ms
    } else if pvp_combat_ms == now_ms {
        combat_until_ms.max(now_ms + crate::combat::COMBAT_DROP_MS)
    } else if in_combat {
        combat_until_ms.max(now_ms)
    } else {
        now_ms
    }
}

fn in_combat(unit: &WorldEntity) -> bool {
    unit.unit_flags & unit_flags::IN_COMBAT != 0
}

#[cfg(test)]
mod tests {
    use super::countdown_start_ms;
    use crate::combat::COMBAT_DROP_MS;

    #[test]
    fn the_countdown_starts_now_outside_pvp_combat() {
        assert_eq!(countdown_start_ms(1_000, 0, true, 9_000), 1_000);
        assert_eq!(countdown_start_ms(1_000, 400, false, 9_000), 1_000);
    }

    #[test]
    fn pvp_combat_holds_the_countdown_until_combat_ends() {
        assert_eq!(countdown_start_ms(1_000, 400, true, 9_000), 9_000);
        assert_eq!(countdown_start_ms(1_000, 400, true, 0), 1_000);
        assert_eq!(
            countdown_start_ms(1_000, 1_000, false, 0),
            1_000 + COMBAT_DROP_MS
        );
        assert_eq!(countdown_start_ms(1_000, 1_000, true, 90_000), 90_000);
    }
}
