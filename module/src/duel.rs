//! Server-authoritative Duel lifecycle and its recipient-keyed gateway relay.

use spacetimedb::{reducer, table, Identity, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::game_world_entity;

pub mod duel_state {
    pub const REQUESTED: u8 = 0;
    pub const COUNTDOWN: u8 = 1;
    pub const ACTIVE: u8 = 2;
}

pub mod duel_event_kind {
    pub const REQUESTED: u8 = 0;
    pub const COUNTDOWN: u8 = 1;
    pub const ACTIVE: u8 = 2;
    pub const COMPLETE: u8 = 3;
}

pub mod duel_completion_kind {
    pub const INTERRUPTED: u8 = 0;
    pub const SURRENDERED: u8 = 1;
    pub const WON: u8 = 2;
    pub const FLED: u8 = 3;
}

pub(crate) const COUNTDOWN_MICROS: i64 = 3_000_000;
pub(crate) const DUEL_TICK_MICROS: i64 = 250_000;

/// One live Duel, indexed from either participant. Clients learn it only through [`DuelEvent`].
#[table(
    accessor = game_duel,
    index(accessor = by_initiator, btree(columns = [initiator_guid])),
    index(accessor = by_challenged, btree(columns = [challenged_guid])),
    index(accessor = by_flag, btree(columns = [flag_guid]))
)]
pub struct Duel {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub flag_guid: u64,
    pub flag_entry: u32,
    pub initiator_guid: u64,
    pub challenged_guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub flag_x: f32,
    pub flag_y: f32,
    pub flag_z: f32,
    pub flag_orientation: f32,
    pub state: u8,
    pub countdown_deadline_micros: i64,
    // Reserved by the boundary slice. End-appending later would be migration-safe, but landing
    // the fields with the lifecycle row lets both participants share one authoritative timer.
    pub initiator_out_of_bounds_deadline_micros: i64,
    pub challenged_out_of_bounds_deadline_micros: i64,
    pub created_at_micros: i64,
}

/// One lifecycle edge for one recipient. The full Duel snapshot makes every packet projection
/// independent of the private row, including the completion edge emitted after that row is gone.
#[table(
    accessor = game_duel_event,
    public,
    index(accessor = by_recipient, btree(columns = [recipient_identity]))
)]
pub struct DuelEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub recipient_guid: u64,
    pub kind: u8,
    pub completion_kind: u8,
    pub duel_id: u64,
    pub flag_guid: u64,
    pub flag_entry: u32,
    pub initiator_guid: u64,
    pub challenged_guid: u64,
    pub winner_guid: u64,
    pub loser_guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub flag_x: f32,
    pub flag_y: f32,
    pub flag_z: f32,
    pub flag_orientation: f32,
    pub created_at: Timestamp,
}

/// The module clock that advances accepted Duels after their three-second countdown.
#[table(accessor = game_duel_schedule, scheduled(tick_duels))]
pub struct DuelSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

crate::character_owned!(delete, fn sweep_delete_game_duel(ctx, character_guid) {
    interrupt_duel_for(ctx, character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_duel());

crate::character_owned!(delete, fn sweep_delete_game_duel_event(ctx, character_guid) {
    let events = ctx.db.game_duel_event();
    let ids: Vec<u64> = events
        .iter()
        .filter(|event| event.recipient_guid == character_guid)
        .map(|event| event.id)
        .collect();
    for id in ids {
        events.id().delete(id);
    }
});
crate::character_owned!(not_transported, fn sweep_transfer_game_duel_event());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestDenied {
    MissingPlayer,
    Dead,
    SelfTarget,
    DifferentPartition,
    Busy,
}

#[derive(Clone, Copy)]
pub(crate) struct RequestParticipant {
    guid: u64,
    is_player: bool,
    dead: bool,
    map_id: u32,
    instance_id: u64,
}

impl From<&crate::WorldEntity> for RequestParticipant {
    fn from(entity: &crate::WorldEntity) -> Self {
        Self {
            guid: entity.guid,
            is_player: entity.is_player(),
            dead: entity.dead || entity.health == 0,
            map_id: entity.map_id,
            instance_id: entity.instance_id,
        }
    }
}

/// Pure request gate. The reducer supplies membership facts from both Duel indexes.
pub(crate) fn request_verdict(
    initiator: Option<RequestParticipant>,
    challenged: Option<RequestParticipant>,
    initiator_busy: bool,
    challenged_busy: bool,
) -> Result<(), RequestDenied> {
    let (Some(initiator), Some(challenged)) = (initiator, challenged) else {
        return Err(RequestDenied::MissingPlayer);
    };
    if !initiator.is_player || !challenged.is_player {
        return Err(RequestDenied::MissingPlayer);
    }
    if initiator.dead || challenged.dead {
        return Err(RequestDenied::Dead);
    }
    if initiator.guid == challenged.guid {
        return Err(RequestDenied::SelfTarget);
    }
    if initiator.map_id != challenged.map_id || initiator.instance_id != challenged.instance_id {
        return Err(RequestDenied::DifferentPartition);
    }
    if initiator_busy || challenged_busy {
        return Err(RequestDenied::Busy);
    }
    Ok(())
}

/// The Duel containing `guid`, from either participant index.
pub(crate) fn duel_involving(ctx: &ReducerContext, guid: u64) -> Option<Duel> {
    let duels = ctx.db.game_duel();
    duels
        .by_initiator()
        .filter(&guid)
        .next()
        .or_else(|| duels.by_challenged().filter(&guid).next())
}

/// Whether these two characters are the two sides of the same active Duel. This is the sole
/// friendly-fire exception: requested and countdown Duels deliberately do not authorize damage.
pub(crate) fn active_opponents(ctx: &ReducerContext, attacker_guid: u64, target_guid: u64) -> bool {
    duel_involving(ctx, attacker_guid)
        .is_some_and(|duel| active_pair(&duel, attacker_guid, target_guid))
}

fn active_pair(duel: &Duel, attacker_guid: u64, target_guid: u64) -> bool {
    duel.state == duel_state::ACTIVE
        && attacker_guid != target_guid
        && ((duel.initiator_guid == attacker_guid && duel.challenged_guid == target_guid)
            || (duel.challenged_guid == attacker_guid && duel.initiator_guid == target_guid))
}

fn duel_flag_guid(duel_id: u64) -> u64 {
    const HIGHGUID_GAMEOBJECT: u64 = 0xF110u64 << 48;
    const DUEL_FLAG_BAND: u64 = 1u64 << 45;
    HIGHGUID_GAMEOBJECT | DUEL_FLAG_BAND | (duel_id & (DUEL_FLAG_BAND - 1))
}

fn push_event(
    ctx: &ReducerContext,
    duel: &Duel,
    recipient_guid: u64,
    kind: u8,
    completion_kind: u8,
    winner_guid: u64,
    loser_guid: u64,
) {
    let bound = crate::helpers::character_by_guid(ctx, recipient_guid)
        .map(|character| character.owner_identity);
    ctx.db.game_duel_event().insert(DuelEvent {
        id: 0,
        recipient_identity: crate::helpers::event_recipient_identity(bound),
        recipient_guid,
        kind,
        completion_kind,
        duel_id: duel.id,
        flag_guid: duel.flag_guid,
        flag_entry: duel.flag_entry,
        initiator_guid: duel.initiator_guid,
        challenged_guid: duel.challenged_guid,
        winner_guid,
        loser_guid,
        map_id: duel.map_id,
        instance_id: duel.instance_id,
        flag_x: duel.flag_x,
        flag_y: duel.flag_y,
        flag_z: duel.flag_z,
        flag_orientation: duel.flag_orientation,
        created_at: ctx.timestamp,
    });
}

fn push_both(ctx: &ReducerContext, duel: &Duel, kind: u8) {
    push_event(ctx, duel, duel.initiator_guid, kind, 0, 0, 0);
    push_event(ctx, duel, duel.challenged_guid, kind, 0, 0, 0);
}

/// Spell effect 83's state mutation. Invalid requests are benign no-ops: the cast itself still
/// resolves normally, but no Duel or duel flag is created.
pub(crate) fn request_duel(
    ctx: &ReducerContext,
    initiator_guid: u64,
    challenged_guid: u64,
    flag_entry: u32,
) {
    let entities = ctx.db.game_world_entity();
    let initiator = entities.guid().find(initiator_guid);
    let challenged = entities.guid().find(challenged_guid);
    if request_verdict(
        initiator.as_ref().map(RequestParticipant::from),
        challenged.as_ref().map(RequestParticipant::from),
        duel_involving(ctx, initiator_guid).is_some(),
        duel_involving(ctx, challenged_guid).is_some(),
    )
    .is_err()
    {
        return;
    }
    let initiator = initiator.expect("request verdict resolved initiator");
    let challenged = challenged.expect("request verdict resolved challenged character");
    let inserted = ctx.db.game_duel().insert(Duel {
        id: 0,
        flag_guid: 0,
        flag_entry,
        initiator_guid,
        challenged_guid,
        map_id: initiator.map_id,
        instance_id: initiator.instance_id,
        flag_x: (initiator.x + challenged.x) * 0.5,
        flag_y: (initiator.y + challenged.y) * 0.5,
        flag_z: (initiator.z + challenged.z) * 0.5,
        flag_orientation: initiator.orientation,
        state: duel_state::REQUESTED,
        countdown_deadline_micros: 0,
        initiator_out_of_bounds_deadline_micros: 0,
        challenged_out_of_bounds_deadline_micros: 0,
        created_at_micros: ctx.timestamp.to_micros_since_unix_epoch(),
    });
    let mut duel = inserted;
    duel.flag_guid = duel_flag_guid(duel.id);
    let duel = ctx.db.game_duel().id().update(duel);
    push_both(ctx, &duel, duel_event_kind::REQUESTED);
}

pub(crate) fn accept_allowed(duel: &Duel, actor_guid: u64, flag_guid: u64) -> bool {
    duel.state == duel_state::REQUESTED
        && duel.challenged_guid == actor_guid
        && duel.flag_guid == flag_guid
}

/// Accept only the proposal addressed to the challenged character and only for its exact arbiter.
pub(crate) fn accept_duel(ctx: &ReducerContext, actor_guid: u64, flag_guid: u64) {
    let Some(mut duel) = duel_involving(ctx, actor_guid) else {
        return;
    };
    if !accept_allowed(&duel, actor_guid, flag_guid) {
        return;
    }
    duel.state = duel_state::COUNTDOWN;
    duel.countdown_deadline_micros = ctx.timestamp.to_micros_since_unix_epoch() + COUNTDOWN_MICROS;
    let duel = ctx.db.game_duel().id().update(duel);
    push_both(ctx, &duel, duel_event_kind::COUNTDOWN);
}

pub(crate) fn countdown_due(state: u8, deadline_micros: i64, now_micros: i64) -> bool {
    state == duel_state::COUNTDOWN && now_micros >= deadline_micros
}

/// The one teardown path. Deleting before emitting makes repeated calls idempotent.
pub(crate) fn complete_duel(
    ctx: &ReducerContext,
    duel_id: u64,
    completion_kind: u8,
    winner_guid: u64,
    loser_guid: u64,
) -> bool {
    let duels = ctx.db.game_duel();
    let Some(duel) = duels.id().find(duel_id) else {
        return false;
    };
    duels.id().delete(duel_id);
    crate::combat::stop_duel_combat(ctx, duel.initiator_guid, duel.challenged_guid);
    push_event(
        ctx,
        &duel,
        duel.initiator_guid,
        duel_event_kind::COMPLETE,
        completion_kind,
        winner_guid,
        loser_guid,
    );
    push_event(
        ctx,
        &duel,
        duel.challenged_guid,
        duel_event_kind::COMPLETE,
        completion_kind,
        winner_guid,
        loser_guid,
    );
    true
}

pub(crate) fn interrupt_duel_for(ctx: &ReducerContext, character_guid: u64) {
    if let Some(duel) = duel_involving(ctx, character_guid) {
        complete_duel(ctx, duel.id, duel_completion_kind::INTERRUPTED, 0, 0);
    }
}

/// Before activation cancel means interruption. Once active it is a surrender, with the other
/// participant already recorded as winner for the combat slice to project.
pub(crate) fn cancel_duel(ctx: &ReducerContext, actor_guid: u64, flag_guid: u64) {
    let Some(duel) = duel_involving(ctx, actor_guid) else {
        return;
    };
    if duel.flag_guid != flag_guid {
        return;
    }
    let (kind, winner, loser) = if duel.state == duel_state::ACTIVE {
        let winner = if actor_guid == duel.initiator_guid {
            duel.challenged_guid
        } else {
            duel.initiator_guid
        };
        (duel_completion_kind::SURRENDERED, winner, actor_guid)
    } else {
        (duel_completion_kind::INTERRUPTED, 0, 0)
    };
    complete_duel(ctx, duel.id, kind, winner, loser);
}

#[reducer]
pub fn tick_duels(ctx: &ReducerContext, _schedule: DuelSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let due: Vec<Duel> = ctx
        .db
        .game_duel()
        .iter()
        .filter(|duel| countdown_due(duel.state, duel.countdown_deadline_micros, now))
        .collect();
    for mut duel in due {
        duel.state = duel_state::ACTIVE;
        duel.countdown_deadline_micros = 0;
        let duel = ctx.db.game_duel().id().update(duel);
        push_both(ctx, &duel, duel_event_kind::ACTIVE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(guid: u64, map_id: u32, instance_id: u64) -> RequestParticipant {
        RequestParticipant {
            guid,
            is_player: true,
            dead: false,
            map_id,
            instance_id,
        }
    }

    fn duel() -> Duel {
        Duel {
            id: 7,
            flag_guid: 99,
            flag_entry: 21_680,
            initiator_guid: 1,
            challenged_guid: 2,
            map_id: 0,
            instance_id: 0,
            flag_x: 1.0,
            flag_y: 2.0,
            flag_z: 3.0,
            flag_orientation: 0.5,
            state: duel_state::REQUESTED,
            countdown_deadline_micros: 0,
            initiator_out_of_bounds_deadline_micros: 0,
            challenged_out_of_bounds_deadline_micros: 0,
            created_at_micros: 0,
        }
    }

    #[test]
    fn request_gate_accepts_two_live_players_in_one_partition() {
        let a = entity(1, 0, 4);
        let b = entity(2, 0, 4);
        assert_eq!(request_verdict(Some(a), Some(b), false, false), Ok(()));
    }

    #[test]
    fn request_gate_rejects_invalid_dead_self_cross_partition_and_busy() {
        let a = entity(1, 0, 4);
        let mut b = entity(2, 0, 4);
        assert_eq!(
            request_verdict(Some(a), None, false, false),
            Err(RequestDenied::MissingPlayer)
        );
        b.dead = true;
        assert_eq!(
            request_verdict(Some(a), Some(b), false, false),
            Err(RequestDenied::Dead)
        );
        let same = entity(1, 0, 4);
        assert_eq!(
            request_verdict(Some(a), Some(same), false, false),
            Err(RequestDenied::SelfTarget)
        );
        let elsewhere = entity(2, 1, 4);
        assert_eq!(
            request_verdict(Some(a), Some(elsewhere), false, false),
            Err(RequestDenied::DifferentPartition)
        );
        let b = entity(2, 0, 4);
        assert_eq!(
            request_verdict(Some(a), Some(b), true, false),
            Err(RequestDenied::Busy)
        );
        assert_eq!(
            request_verdict(Some(a), Some(b), false, true),
            Err(RequestDenied::Busy)
        );
    }

    #[test]
    fn only_challenged_character_can_accept_the_exact_flag() {
        let duel = duel();
        assert!(accept_allowed(&duel, 2, 99));
        assert!(!accept_allowed(&duel, 1, 99));
        assert!(!accept_allowed(&duel, 2, 100));
    }

    #[test]
    fn countdown_activates_at_the_exact_boundary() {
        assert!(!countdown_due(duel_state::COUNTDOWN, 3_000_000, 2_999_999));
        assert!(countdown_due(duel_state::COUNTDOWN, 3_000_000, 3_000_000));
        assert!(!countdown_due(duel_state::ACTIVE, 3_000_000, 3_000_000));
    }

    #[test]
    fn only_active_participants_are_combat_opponents() {
        let mut duel = duel();
        assert!(!active_pair(&duel, 1, 2));
        duel.state = duel_state::COUNTDOWN;
        assert!(!active_pair(&duel, 1, 2));
        duel.state = duel_state::ACTIVE;
        assert!(active_pair(&duel, 1, 2));
        assert!(active_pair(&duel, 2, 1));
        assert!(!active_pair(&duel, 1, 3));
        assert!(!active_pair(&duel, 1, 1));
    }

    #[test]
    fn stable_flag_identity_is_unique_per_duel_and_repeatable() {
        assert_eq!(duel_flag_guid(7), duel_flag_guid(7));
        assert_ne!(duel_flag_guid(7), duel_flag_guid(8));
        assert_eq!(duel_flag_guid(7) >> 48, 0xF110);
    }

    #[test]
    fn completion_path_deletes_before_emitting_and_absence_is_a_noop() {
        let source = include_str!("duel.rs");
        let start = source
            .find("pub(crate) fn complete_duel(")
            .expect("completion chokepoint exists");
        let end = source[start..]
            .find("pub(crate) fn interrupt_duel_for(")
            .map(|offset| start + offset)
            .expect("completion chokepoint has a bounded body");
        let body = &source[start..end];
        assert!(body.contains("let Some(duel) = duels.id().find(duel_id) else"));
        assert!(body.contains("return false;"));
        assert!(body.contains("crate::combat::stop_duel_combat"));
        assert!(
            body.find("duels.id().delete(duel_id)").unwrap() < body.find("push_event(").unwrap(),
            "state must disappear before any completion edge is emitted"
        );
    }
}
