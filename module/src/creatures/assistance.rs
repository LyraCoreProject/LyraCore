use spacetimedb::{table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::{game_creature_template, game_faction_template, game_world_entity, WorldEntity};

// vmangos World.cpp CreatureFamilyAssistanceDelay default.
const ASSISTANCE_DELAY_MICROS: i64 = 1_500_000;
// vmangos `MoveAwayFromTarget(pEnemy, 10.0f)`, and the distance inside which a creature flees.
const FLEE_DISTANCE_YD: f32 = 10.0;
const MOVE_AWAY_CLIMB_YD: f32 = 10.0;

#[table(
    accessor = game_creature_assistance_call,
    scheduled(answer_assistance_call)
)]
pub struct AssistanceCall {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub creature_guid: u64,
    #[index(btree)]
    pub victim_guid: u64,
    pub assistant_guids: Vec<u64>,
    pub scheduled_at: ScheduleAt,
}

crate::outside_table_catalog!(game_creature_assistance_call);

crate::character_owned!(delete, fn sweep_delete_game_creature_assistance_call(ctx, character_guid) {
    ctx.db.game_creature_assistance_call().victim_guid().delete(character_guid);
});
// A call belongs to the live fight and does not survive Transfer.
crate::character_owned!(not_transported, fn sweep_transfer_game_creature_assistance_call());

/// Which creatures a call reaches: vmangos `CanBeTargetedByCallForHelp` with or without `checkfaction`.
#[derive(Clone, Copy, PartialEq)]
enum Kin {
    SameFactionTemplate,
    Friendly,
}

fn call_for_help_range(ctx: &ReducerContext, creature: &WorldEntity) -> f32 {
    ctx.db
        .game_creature_template()
        .entry()
        .find(creature.entry)
        .map_or(0.0, |template| template.call_for_help_range)
}

/// Neighbours of `caller` within `radius` that can be called against `victim` and that it sees.
fn called_neighbours(
    ctx: &ReducerContext,
    caller: &WorldEntity,
    victim: &WorldEntity,
    radius: f32,
    kin: Kin,
) -> Vec<WorldEntity> {
    crate::helpers::entities_near(
        ctx,
        caller.map_id,
        caller.instance_id,
        caller.x,
        caller.y,
        radius,
    )
    .into_iter()
    .filter(|n| crate::helpers::dist_sq(caller, n) <= radius * radius)
    .filter(|n| can_be_called(ctx, n, caller, victim, kin))
    .filter(|n| {
        crate::nav::has_los(
            ctx,
            caller.map_id,
            caller.instance_id,
            (caller.x, caller.y, caller.z),
            (n.x, n.y, n.z),
        )
    })
    .collect()
}

/// vmangos `Creature::CallAssistance`: neighbours able to help when the call lands attack the victim.
pub(crate) fn call_assistance(ctx: &ReducerContext, caller_guid: u64, victim_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let (Some(caller), Some(victim)) = (
        entities.guid().find(caller_guid),
        entities.guid().find(victim_guid),
    ) else {
        return;
    };
    if caller.is_player() || caller.owner_guid != 0 || call_for_help_range(ctx, &caller) <= 0.0 {
        return;
    }
    let mut assistant_guids: Vec<u64> = called_neighbours(
        ctx,
        &caller,
        &victim,
        super::ai::ASSIST_RADIUS,
        Kin::SameFactionTemplate,
    )
    .iter()
    .filter(|n| answers_against(ctx, n))
    .map(|n| n.guid)
    .collect();
    if assistant_guids.is_empty() {
        return;
    }
    assistant_guids.sort_unstable();
    let Some(due) =
        crate::time::now(ctx).checked_add(TimeDuration::from_micros(ASSISTANCE_DELAY_MICROS))
    else {
        return;
    };
    ctx.db
        .game_creature_assistance_call()
        .insert(AssistanceCall {
            scheduled_id: 0,
            creature_guid: caller_guid,
            victim_guid,
            assistant_guids,
            scheduled_at: crate::time::schedule_time(ctx, due),
        });
}

/// vmangos `Creature::CallForHelp`; `moves_randomly` tells whether a creature is on random movement.
pub(crate) fn call_for_help(
    ctx: &ReducerContext,
    caller_guid: u64,
    victim_guid: u64,
    moves_randomly: impl Fn(u64) -> bool,
) {
    let entities = ctx.db.game_world_entity();
    let (Some(caller), Some(victim)) = (
        entities.guid().find(caller_guid),
        entities.guid().find(victim_guid),
    ) else {
        return;
    };
    let range = call_for_help_range(ctx, &caller);
    if caller.is_player() || caller.owner_guid != 0 || range <= 0.0 {
        return;
    }
    let mut neighbours = called_neighbours(ctx, &caller, &victim, range, Kin::Friendly);
    neighbours.sort_unstable_by_key(|n| n.guid);
    for neighbour in neighbours {
        if answers_against(ctx, &neighbour) {
            crate::combat::answer_creature_help(ctx, neighbour.guid, victim.guid);
        } else if flees_from(ctx, &neighbour, &victim, moves_randomly(neighbour.guid)) {
            move_away_from(ctx, neighbour, &victim);
        }
    }
}

#[spacetimedb::reducer]
pub fn answer_assistance_call(ctx: &ReducerContext, call: AssistanceCall) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    ctx.db
        .game_creature_assistance_call()
        .scheduled_id()
        .delete(call.scheduled_id);
    let entities = ctx.db.game_world_entity();
    let (Some(caller), Some(victim)) = (
        entities.guid().find(call.creature_guid),
        entities.guid().find(call.victim_guid),
    ) else {
        return;
    };
    if victim.map_id != caller.map_id || victim.instance_id != caller.instance_id {
        return;
    }
    for guid in call.assistant_guids {
        if entities.guid().find(guid).is_some_and(|assistant| {
            can_be_called(ctx, &assistant, &caller, &victim, Kin::SameFactionTemplate)
                && answers_against(ctx, &assistant)
        }) {
            crate::combat::answer_creature_assistance(ctx, guid, victim.guid);
        }
    }
}

/// vmangos `Creature::CanBeTargetedByCallForHelp`.
fn can_be_called(
    ctx: &ReducerContext,
    neighbour: &WorldEntity,
    caller: &WorldEntity,
    victim: &WorldEntity,
    kin: Kin,
) -> bool {
    use lyracore_shared::constants::unit_flags;

    neighbour.guid != caller.guid
        && !neighbour.is_player()
        && !neighbour.dead
        && neighbour.owner_guid == 0
        && (neighbour.faction_template == caller.faction_template
            || (kin == Kin::Friendly
                && crate::faction::is_friendly(
                    ctx,
                    neighbour.faction_template,
                    caller.faction_template,
                )))
        && neighbour.unit_flags & (unit_flags::IN_COMBAT | unit_flags::NOT_SELECTABLE) == 0
        && !victim.dead
        && ctx
            .db
            .game_faction_template()
            .id()
            .find(neighbour.faction_template)
            .is_some_and(|template| template.reacts_to_call_for_help())
        && !crate::combat::is_engaged(ctx, neighbour.guid)
        && !crate::spell::is_action_blocked(ctx, neighbour.guid)
        && crate::combat::may_harm(ctx, neighbour, victim)
}

/// vmangos `Creature::CanRespondToCallForHelpAgainst`.
fn answers_against(ctx: &ReducerContext, neighbour: &WorldEntity) -> bool {
    ctx.db
        .game_faction_template()
        .id()
        .find(neighbour.faction_template)
        .is_some_and(|template| template.answers_call_for_help())
        && super::react_state(ctx, neighbour.guid) != super::CreatureReactState::Passive
        && !super::eventai::movement::returning_home(ctx, neighbour.guid)
}

/// vmangos `Creature::CanFleeFromCallForHelpAgainst`.
fn flees_from(
    ctx: &ReducerContext,
    neighbour: &WorldEntity,
    victim: &WorldEntity,
    moves_randomly: bool,
) -> bool {
    moves_randomly
        && ctx
            .db
            .game_faction_template()
            .id()
            .find(neighbour.faction_template)
            .is_some_and(|template| template.flees_from_call_for_help())
        && !crate::spell::is_rooted(ctx, neighbour.guid)
        && crate::helpers::dist_sq(neighbour, victim) < FLEE_DISTANCE_YD * FLEE_DISTANCE_YD
}

/// vmangos `MoveDistance`: to the point 10 yd from `threat` on the mover's side.
fn move_away_from(ctx: &ReducerContext, mover: WorldEntity, threat: &WorldEntity) {
    let (dx, dy) = (mover.x - threat.x, mover.y - threat.y);
    let len = (dx * dx + dy * dy).sqrt();
    let (ux, uy) = if len > 0.0 {
        (dx / len, dy / len)
    } else {
        (1.0, 0.0)
    };
    let (x, y) = (
        threat.x + ux * FLEE_DISTANCE_YD,
        threat.y + uy * FLEE_DISTANCE_YD,
    );
    let z = crate::terrain::snap_z(ctx, mover.map_id, mover.instance_id, x, y, mover.z);
    if z > mover.z + MOVE_AWAY_CLIMB_YD
        || !crate::nav::has_los(
            ctx,
            mover.map_id,
            mover.instance_id,
            (threat.x, threat.y, threat.z),
            (x, y, z),
        )
    {
        return;
    }
    let run = crate::modifiers::effective_move_speed(
        ctx,
        mover.guid,
        lyracore_shared::constants::speeds::RUN,
    );
    let step = crate::nav::nav_step(
        ctx,
        mover.map_id,
        mover.instance_id,
        (mover.x, mover.y),
        (x, y),
        FLEE_DISTANCE_YD,
        0.0,
        mover.z,
    );
    let Some((x, y, duration_ms)) = super::ai::leg_toward((mover.x, mover.y), step, run) else {
        return;
    };
    let course = super::leg::Course::on_ground(ctx, &mover, (x, y), z, duration_ms, true)
        .held_until_landed(true);
    super::leg::start(ctx, mover, course);
}
