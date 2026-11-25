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

/// The countdown that drops a Character's PvP Flag; [`expire_pvp_flag`] re-arms a held one.
#[table(accessor = game_pvp_flag_timer, scheduled(expire_pvp_flag))]
pub struct PvpFlagTimer {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    #[unique]
    pub character_guid: u64,
    pub scheduled_at: ScheduleAt,
    /// The last PvP combat pulse, cleared when a later combat begins; vmangos `inPvPCombat`.
    pub last_pvp_pulse_ms: Option<u64>,
    /// Whether the last zone check held the flag: a PvP-enforced zone outside a taxi flight.
    pub in_enforced_zone: bool,
}

crate::outside_table_catalog!(game_pvp_flag_timer);

crate::character_owned!(delete, fn sweep_delete_game_pvp_flag_timer(ctx, character_guid) {
    ctx.db.game_pvp_flag_timer().character_guid().delete(character_guid);
});
// Transfer rebuilds the Character's entity without its PvP Flag.
crate::character_owned!(not_transported, fn sweep_transfer_game_pvp_flag_timer());

/// What holds a PvP Flag past its countdown, besides `PVP_DESIRED`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Hold {
    last_pvp_pulse_ms: Option<u64>,
    in_enforced_zone: bool,
}

fn hold(ctx: &ReducerContext, character_guid: u64) -> Hold {
    ctx.db
        .game_pvp_flag_timer()
        .character_guid()
        .find(character_guid)
        .map_or_else(Hold::default, |timer| Hold {
            last_pvp_pulse_ms: timer.last_pvp_pulse_ms,
            in_enforced_zone: timer.in_enforced_zone,
        })
}

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
    let was_desired = character.player_flags & player_flags::PVP_DESIRED != 0;
    if desired.unwrap_or(!was_desired) {
        character.player_flags |= player_flags::PVP_DESIRED;
        let held = hold(ctx, character_guid);
        refresh(ctx, &mut character, held);
    } else {
        character.player_flags &= !player_flags::PVP_DESIRED;
        // Clearing the wish lets a held countdown run; an already running one keeps its time.
        if was_desired && is_flagged(&character) {
            arm(ctx, &character, hold(ctx, character_guid));
        }
    }
    character.update(ctx);
}

/// vmangos `Player::UpdateZone`: entering a PvP-enforced zone flags the Character unless it
/// flies a taxi, the zone holds the flag, and leaving starts the countdown. Writes only on a
/// change; returns whether the zone enforces PvP. The caller saves `character`.
pub(crate) fn update_zone(
    ctx: &ReducerContext,
    character: &mut WorldEntity,
    zone: Option<&GameArea>,
) -> bool {
    let enforced = zone.is_some_and(|zone| in_enforced_zone(ctx, character, zone));
    let held = hold(ctx, character.guid);
    if enforced && !character.is_flying() {
        if !is_flagged(character) || !held.in_enforced_zone {
            let held = Hold {
                in_enforced_zone: true,
                ..held
            };
            refresh(ctx, character, held);
        }
    } else if held.in_enforced_zone {
        let held = Hold {
            in_enforced_zone: false,
            ..held
        };
        if is_flagged(character) {
            arm(ctx, character, held);
        } else {
            ctx.db
                .game_pvp_flag_timer()
                .character_guid()
                .delete(character.guid);
        }
    }
    enforced
}

/// Runs the zone check for a Character entering the world, before viewers build its CREATE.
pub(crate) fn enter_world(ctx: &ReducerContext, character: &mut WorldEntity) {
    let zone = zone_at(ctx, character);
    update_zone(ctx, character, zone.as_ref());
}

/// Runs the zone check where a near teleport left the Character; movement's own check skips it.
pub(crate) fn recheck_zone(ctx: &ReducerContext, character_guid: u64) {
    let Some(mut character) = ctx.db.game_world_entity().guid().find(character_guid) else {
        return;
    };
    let unit_flags = character.unit_flags;
    let zone = zone_at(ctx, &character);
    update_zone(ctx, &mut character, zone.as_ref());
    if character.unit_flags != unit_flags {
        character.update(ctx);
    }
}

/// A map change keeps the PvP Flag and `PVP_DESIRED` of the replaced entity; its timer row stays.
pub(crate) fn keep_across_map_change(replaced: &WorldEntity, character: &mut WorldEntity) {
    character.unit_flags |= replaced.unit_flags & unit_flags::PVP;
    character.player_flags |= replaced.player_flags & player_flags::PVP_DESIRED;
}

fn zone_at(ctx: &ReducerContext, character: &WorldEntity) -> Option<GameArea> {
    crate::terrain::area_at(ctx, character.map_id, character.x, character.y)
        .and_then(|area| ctx.db.game_area().id().find(crate::terrain::zone_of(&area)))
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

fn in_enforced_area(ctx: &ReducerContext, character: &WorldEntity) -> bool {
    zone_at(ctx, character).is_some_and(|zone| in_enforced_zone(ctx, character, &zone))
}

fn is_pvp_realm(ctx: &ReducerContext) -> bool {
    ctx.db.game_realm().iter().any(|realm| {
        matches!(
            realm.realm_type,
            realm_type::PVP | realm_type::RP_PVP | realm_type::FFA_PVP
        )
    })
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

/// vmangos `Unit::SetInCombatWithAssisted` and `Spell::DoSpellHitOnUnit`: help to another
/// flagged Character flags the helper's Character, which joins the target's PvP combat.
fn helped(ctx: &ReducerContext, helper: &WorldEntity, target: &WorldEntity) {
    let (Some(by), Some(of)) = (
        affecting_character(ctx, helper),
        affecting_character(ctx, target),
    ) else {
        return;
    };
    if by.guid == of.guid || !(is_flagged(target) || of.flagged) {
        return;
    }
    let joins_pvp_combat = in_combat(target) && hold(ctx, of.guid).last_pvp_pulse_ms.is_some();
    pulse(ctx, by.guid, joins_pvp_combat);
}

/// vmangos `Spell::DoAllEffectOnTarget` and `DoSpellHitOnUnit`, hit or not: effect `e` harms a
/// unit its caster is not friendly to. It helps a friendly unit in combat, or one a helpful target
/// kind names. A Duel challenge does neither.
pub(crate) fn spell_targeted(
    ctx: &ReducerContext,
    caster_guid: u64,
    target_guid: u64,
    e: &crate::SpellEffect,
) {
    use lyracore_shared::spell::taxonomy::{
        E_DUEL, T_AREA_ALLY, T_AREA_NOT_HOSTILE, T_PARTY_AREA, T_RAID_AREA, T_TARGET_ALLY,
    };
    if e.kind == E_DUEL {
        return;
    }
    let helpful_target = matches!(
        e.target,
        T_TARGET_ALLY | T_AREA_ALLY | T_AREA_NOT_HOSTILE | T_PARTY_AREA | T_RAID_AREA
    );
    let entities = ctx.db.game_world_entity();
    let (Some(caster), Some(target)) = (
        entities.guid().find(caster_guid),
        entities.guid().find(target_guid),
    ) else {
        return;
    };
    match crate::combat::relation(ctx, &caster, &target) {
        crate::combat::Relation::SelfUnit => {}
        crate::combat::Relation::Friendly => {
            if helpful_target || in_combat(&target) {
                helped(ctx, &caster, &target);
            }
        }
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
    if timer
        .last_pvp_pulse_ms
        .is_none_or(|pulse_ms| pulse_ms >= crate::time::now_ms(ctx))
    {
        return;
    }
    timer.last_pvp_pulse_ms = None;
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
    let held = Hold {
        last_pvp_pulse_ms: timer.last_pvp_pulse_ms,
        in_enforced_zone: timer.in_enforced_zone,
    };
    // A teleport or a death can leave the zone with no zone check; that counts as leaving now.
    if held.in_enforced_zone && (character.is_flying() || !in_enforced_area(ctx, &character)) {
        let held = Hold {
            in_enforced_zone: false,
            ..held
        };
        arm(ctx, &character, held);
        return;
    }
    let frozen = (held.last_pvp_pulse_ms.is_some() && in_combat(&character))
        || held.in_enforced_zone
        || character.player_flags & player_flags::PVP_DESIRED != 0;
    if frozen {
        arm(ctx, &character, held);
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
    let mut held = hold(ctx, character_guid);
    if pvp_combat {
        held.last_pvp_pulse_ms = Some(crate::time::now_ms(ctx));
    }
    refresh(ctx, &mut character, held);
    if !was_flagged {
        character.update(ctx);
    }
}

/// vmangos `Player::UpdatePvP(true)`: flags `character` and restarts its countdown; the caller
/// saves `character`.
fn refresh(ctx: &ReducerContext, character: &mut WorldEntity, held: Hold) {
    character.unit_flags |= unit_flags::PVP;
    arm(ctx, character, held);
}

fn arm(ctx: &ReducerContext, character: &WorldEntity, held: Hold) {
    let timers = ctx.db.game_pvp_flag_timer();
    timers.character_guid().delete(character.guid);
    let start_ms = countdown_start_ms(
        crate::time::now_ms(ctx),
        held.last_pvp_pulse_ms,
        in_combat(character),
        character.combat_until_ms,
    );
    let due_micros = start_ms.saturating_add(PVP_FLAG_MS).saturating_mul(1000);
    let due = Timestamp::from_micros_since_unix_epoch(due_micros.min(i64::MAX as u64) as i64);
    timers.insert(PvpFlagTimer {
        scheduled_id: 0,
        character_guid: character.guid,
        scheduled_at: crate::time::schedule_time(ctx, due),
        last_pvp_pulse_ms: held.last_pvp_pulse_ms,
        in_enforced_zone: held.in_enforced_zone,
    });
}

/// vmangos freezes the countdown in PvP combat; a pulse now keeps combat for `COMBAT_DROP_MS`.
fn countdown_start_ms(
    now_ms: u64,
    last_pvp_pulse_ms: Option<u64>,
    in_combat: bool,
    combat_until_ms: u64,
) -> u64 {
    match last_pvp_pulse_ms {
        None => now_ms,
        Some(pulse_ms) if pulse_ms == now_ms => {
            combat_until_ms.max(now_ms + crate::combat::COMBAT_DROP_MS)
        }
        Some(_) if in_combat => combat_until_ms.max(now_ms),
        Some(_) => now_ms,
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
        assert_eq!(countdown_start_ms(1_000, None, true, 9_000), 1_000);
        assert_eq!(countdown_start_ms(1_000, Some(400), false, 9_000), 1_000);
    }

    #[test]
    fn pvp_combat_holds_the_countdown_until_combat_ends() {
        assert_eq!(countdown_start_ms(1_000, Some(400), true, 9_000), 9_000);
        assert_eq!(countdown_start_ms(1_000, Some(400), true, 0), 1_000);
        assert_eq!(
            countdown_start_ms(1_000, Some(1_000), false, 0),
            1_000 + COMBAT_DROP_MS
        );
        assert_eq!(countdown_start_ms(1_000, Some(1_000), true, 90_000), 90_000);
    }
}
