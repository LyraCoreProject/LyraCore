use spacetimedb::{table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::{game_faction_template, game_world_entity, WorldEntity};

// vmangos World.cpp CreatureFamilyAssistanceDelay default.
const ASSISTANCE_DELAY_MICROS: i64 = 1_500_000;

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

/// vmangos `Creature::CallAssistance`: the neighbours that could help when the caller gains its
/// victim are asked once, and each one that still can when the call lands attacks that victim.
pub(crate) fn call_assistance(ctx: &ReducerContext, caller_guid: u64, victim_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let (Some(caller), Some(victim)) = (
        entities.guid().find(caller_guid),
        entities.guid().find(victim_guid),
    ) else {
        return;
    };
    if caller.is_player() || caller.owner_guid != 0 {
        return;
    }
    let radius = super::ai::ASSIST_RADIUS;
    let mut assistant_guids: Vec<u64> = crate::helpers::entities_near(
        ctx,
        caller.map_id,
        caller.instance_id,
        caller.x,
        caller.y,
        radius,
    )
    .into_iter()
    .filter(|n| crate::helpers::dist_sq(&caller, n) <= radius * radius)
    .filter(|n| can_assist(ctx, n, &caller, &victim))
    .filter(|n| {
        crate::nav::has_los(
            ctx,
            caller.map_id,
            caller.instance_id,
            (caller.x, caller.y, caller.z),
            (n.x, n.y, n.z),
        )
    })
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
    for guid in call.assistant_guids {
        if entities
            .guid()
            .find(guid)
            .is_some_and(|assistant| can_assist(ctx, &assistant, &caller, &victim))
        {
            crate::combat::answer_creature_assistance(ctx, guid, victim.guid);
        }
    }
}

/// vmangos `Creature::CanAssistTo` with the same faction template required.
fn can_assist(
    ctx: &ReducerContext,
    assistant: &WorldEntity,
    caller: &WorldEntity,
    victim: &WorldEntity,
) -> bool {
    use lyracore_shared::constants::unit_flags;

    assistant.guid != caller.guid
        && !assistant.is_player()
        && !assistant.dead
        && assistant.owner_guid == 0
        && assistant.faction_template == caller.faction_template
        && assistant.unit_flags & (unit_flags::IN_COMBAT | unit_flags::NOT_SELECTABLE) == 0
        && !victim.dead
        && ctx
            .db
            .game_faction_template()
            .id()
            .find(assistant.faction_template)
            .is_some_and(|template| template.answers_call_for_help())
        && super::react_state(ctx, assistant.guid) != super::CreatureReactState::Passive
        && !crate::combat::is_engaged(ctx, assistant.guid)
        && !crate::spell::is_action_blocked(ctx, assistant.guid)
        && !super::eventai::movement::returning_home(ctx, assistant.guid)
        && crate::combat::may_harm(ctx, assistant, victim)
}
