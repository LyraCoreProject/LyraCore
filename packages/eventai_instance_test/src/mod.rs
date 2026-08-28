#[cfg(feature = "debug_reducers")]
use spacetimedb::{reducer, ReducerContext, ScheduleAt, Table, TimeDuration};

#[cfg(feature = "debug_reducers")]
use crate::encounter::{
    self, EncounterBinding, EncounterSignal, DOOR_OPEN_STATE, ENCOUNTER_DONE, ENCOUNTER_FAILED,
    ENCOUNTER_IN_PROGRESS, ENCOUNTER_NOT_STARTED,
};
#[cfg(feature = "debug_reducers")]
use crate::pkg_dungeons::{
    blackrock_tomb_round, shadowfang_dark_offering_schedule, shadowfang_fenrus_choreography,
    shadowfang_voidwalker_group, sunken_temple_suppression, wailing_escort_progress,
    wailing_escort_schedule, WailingEscortSchedule, VOIDWALKER_ROUTE,
};
#[cfg(feature = "debug_reducers")]
use crate::{
    game_chat_event, game_creature_gossip_menu_override, game_creature_spline,
    game_creature_template, game_gameobject, game_gossip_menu_profile_option, game_instance,
    game_spell, game_spell_effect, game_world_entity, GameInstance, SpellEffect,
};

#[cfg(feature = "debug_reducers")]
const FIXTURE_LOW_BAND: u64 = 0x10_0000;
#[cfg(feature = "debug_reducers")]
const TOMB_SCHEDULER_INSTANCE: u64 = 920;
#[cfg(feature = "debug_reducers")]
const WAILING_WAIT_MUTANUS_PHASE: u8 = 19;
#[cfg(feature = "debug_reducers")]
const WAILING_DISCIPLE_AWAKE_PHASE: u8 = 21;
#[cfg(feature = "debug_reducers")]
const WAILING_ROUTE_ARRIVE_PHASE: u8 = 2;
#[cfg(feature = "debug_reducers")]
const WAILING_FIRST_CORNER_POINT: u8 = 12;
#[cfg(feature = "debug_reducers")]
const WAILING_EXIT_PHASE: u8 = 25;
#[cfg(feature = "debug_reducers")]
const WAILING_DESPAWN_PHASE: u8 = 26;

/// Starts the Tomb of Seven through EventAI's production notification boundary. The standalone
/// verifier calls this reducer, waits for the package-owned schedule, then checks the next round.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_begin_tomb_round_scheduler(ctx: &ReducerContext) -> Result<(), String> {
    let first_dwarf = spawn_source(ctx, 9034, 230, TOMB_SCHEDULER_INSTANCE, false, 30)?;
    for (sequence, entry) in (31..).zip([9035, 9036, 9037, 9038, 9039, 9040]) {
        spawn_source(ctx, entry, 230, TOMB_SCHEDULER_INSTANCE, false, sequence)?;
    }
    let player_guid = spawn_fixture_player(ctx, 230, TOMB_SCHEDULER_INSTANCE, 38)?;
    set_fixture_position(ctx, player_guid, 0.0, 0.0, 0.0)?;
    notify(
        ctx,
        first_dwarf,
        EncounterBinding::BlackrockDepthsTombOfSeven,
        EncounterSignal::Begin,
    )?;
    let first_dwarf = ctx
        .db
        .game_world_entity()
        .guid()
        .find(first_dwarf)
        .ok_or_else(|| "first Tomb dwarf disappeared".to_string())?;
    require(
        first_dwarf.faction_template == 754 && first_dwarf.target_guid == player_guid,
        "Tomb Begin did not activate the first dwarf against the living player",
    )?;
    let timer = ctx
        .db
        .blackrock_tomb_round()
        .by_instance()
        .filter(&TOMB_SCHEDULER_INSTANCE)
        .next()
        .ok_or_else(|| "Tomb Begin did not schedule the second round".to_string())?;
    require(
        timer.next_round == 1,
        "Tomb Begin scheduled the wrong next round",
    )
}

/// Verifies the durable outcome of the Tomb round callback after the standalone wait.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_tomb_round_scheduler(ctx: &ReducerContext) -> Result<(), String> {
    let second_guid = fixture_guid(9035, 31);
    let second_dwarf = ctx
        .db
        .game_world_entity()
        .guid()
        .find(second_guid)
        .ok_or_else(|| "second Tomb dwarf disappeared".to_string())?;
    require(
        second_dwarf.faction_template == 754,
        "Tomb scheduler did not make the second dwarf hostile",
    )?;
    let timer = ctx
        .db
        .blackrock_tomb_round()
        .by_instance()
        .filter(&TOMB_SCHEDULER_INSTANCE)
        .next()
        .ok_or_else(|| "Tomb scheduler did not schedule the third round".to_string())?;
    require(
        timer.next_round == 2,
        "Tomb scheduler advanced to the wrong round",
    )
}

/// Verifies Fenrus's delayed package choreography after the standalone wait.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_shadowfang_choreography(ctx: &ReducerContext) -> Result<(), String> {
    require(
        ctx.db
            .game_world_entity()
            .by_map()
            .filter(&33u32)
            .all(|entity| entity.instance_id != 907 || entity.entry != 4275),
        "Archmage Arugal stayed visible after the invisibility step",
    )?;
    let voidwalkers: Vec<_> = ctx
        .db
        .game_world_entity()
        .by_map()
        .filter(&33u32)
        .filter(|entity| entity.instance_id == 907 && entity.entry == 4627 && !entity.dead)
        .collect();
    require(
        voidwalkers.len() == 4,
        "Fenrus choreography did not summon four Arugal Voidwalkers",
    )?;
    let group = ctx
        .db
        .shadowfang_voidwalker_group()
        .instance_id()
        .find(907)
        .ok_or_else(|| "Fenrus choreography installed no durable voidwalker group".to_string())?;
    require(
        group.walker_guids.len() == 4 && group.walker_guids.contains(&group.leader_guid),
        "Arugal Voidwalkers did not elect one durable leader",
    )?;
    require(
        group.route_point > 0,
        "Arugal Voidwalker group did not advance its pinned patrol route",
    )?;
    let route_point =
        (usize::from(group.route_point) + VOIDWALKER_ROUTE.len() - 1) % VOIDWALKER_ROUTE.len();
    let destination = VOIDWALKER_ROUTE[route_point];
    let previous = if route_point == 0 {
        VOIDWALKER_ROUTE[VOIDWALKER_ROUTE.len() - 2]
    } else {
        VOIDWALKER_ROUTE[route_point - 1]
    };
    let heading = (destination.1 - previous.1).atan2(destination.0 - previous.0);
    let leader_target = ctx
        .db
        .game_creature_spline()
        .guid()
        .find(group.leader_guid)
        .map(|spline| (spline.dx, spline.dy, spline.dz))
        .or_else(|| {
            ctx.db
                .game_world_entity()
                .guid()
                .find(group.leader_guid)
                .map(|leader| (leader.x, leader.y, leader.z))
        })
        .ok_or_else(|| "Arugal Voidwalker leader disappeared".to_string())?;
    require(
        (leader_target.0 - destination.0).abs() < 0.01
            && (leader_target.1 - destination.1).abs() < 0.01
            && (leader_target.2 - destination.2).abs() < 0.01,
        "Arugal Voidwalker leader did not target its pinned patrol point",
    )?;
    for (position, follower_guid) in group.walker_guids.iter().copied().enumerate().skip(1) {
        let angle = heading + std::f32::consts::FRAC_PI_2 * position as f32;
        let expected = (
            destination.0 + angle.cos(),
            destination.1 + angle.sin(),
            destination.2,
        );
        let actual = ctx
            .db
            .game_creature_spline()
            .guid()
            .find(follower_guid)
            .map(|spline| (spline.dx, spline.dy, spline.dz))
            .or_else(|| {
                ctx.db
                    .game_world_entity()
                    .guid()
                    .find(follower_guid)
                    .map(|follower| (follower.x, follower.y, follower.z))
            })
            .ok_or_else(|| "an Arugal Voidwalker follower disappeared".to_string())?;
        require(
            (actual.0 - expected.0).abs() < 0.01
                && (actual.1 - expected.1).abs() < 0.01
                && (actual.2 - expected.2).abs() < 0.01,
            "an Arugal Voidwalker did not target its 1 yard formation offset",
        )?;
    }
    require(
        gameobject_state(ctx, fixture_gameobject_guid(26))? == DOOR_OPEN_STATE,
        "Arugal's focus did not activate for the lightning step",
    )?;
    require(
        ctx.db
            .shadowfang_fenrus_choreography()
            .by_instance()
            .filter(&907u64)
            .next()
            .is_none(),
        "Fenrus choreography left a scheduled step behind",
    )?;

    let wounded_guid = group
        .walker_guids
        .iter()
        .copied()
        .find(|guid| *guid != group.leader_guid)
        .ok_or_else(|| "voidwalker group had no Dark Offering target".to_string())?;
    let mut wounded = ctx
        .db
        .game_world_entity()
        .guid()
        .find(wounded_guid)
        .ok_or_else(|| "Dark Offering target disappeared".to_string())?;
    wounded.max_health = 1_000;
    wounded.health = 500;
    ctx.db.game_world_entity().guid().update(wounded);
    require(
        crate::combat::arm_creature_engagement(ctx, group.leader_guid, fixture_guid(0, 46), false),
        "Arugal Voidwalker did not cross the creature aggro boundary",
    )?;
    require(
        ctx.db
            .shadowfang_dark_offering_schedule()
            .by_caster()
            .filter(&group.leader_guid)
            .count()
            == 1,
        "Arugal Voidwalker aggro did not arm one Dark Offering callback",
    )
}

/// Verifies Dark Offering's durable heal, kills the source-spawned group through the production
/// death path, then closes the door to model a reloaded gameobject before restart recovery.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_shadowfang_dark_offering_and_prepare_restart(
    ctx: &ReducerContext,
) -> Result<(), String> {
    let group = ctx
        .db
        .shadowfang_voidwalker_group()
        .instance_id()
        .find(907)
        .ok_or_else(|| "Arugal Voidwalker group disappeared before Dark Offering".to_string())?;
    let wounded_guid = group
        .walker_guids
        .iter()
        .copied()
        .find(|guid| *guid != group.leader_guid)
        .ok_or_else(|| "voidwalker group had no Dark Offering target".to_string())?;
    let wounded = ctx
        .db
        .game_world_entity()
        .guid()
        .find(wounded_guid)
        .ok_or_else(|| "Dark Offering target disappeared".to_string())?;
    require(
        wounded.health > 500,
        "Dark Offering did not durably heal the lowest-health friendly",
    )?;

    let player_guid = fixture_guid(0, 46);
    for walker_guid in group.walker_guids {
        require(
            crate::combat::kill_creature(ctx, walker_guid, Some(player_guid)),
            "production creature death refused an Arugal Voidwalker",
        )?;
    }
    let sorcerer_door = fixture_gameobject_guid(27);
    require(
        gameobject_state(ctx, sorcerer_door)? == DOOR_OPEN_STATE,
        "the last Arugal Voidwalker did not open the Sorcerer Door",
    )?;
    let gameobjects = ctx.db.game_gameobject();
    let mut door = gameobjects
        .guid()
        .find(sorcerer_door)
        .ok_or_else(|| "Sorcerer Door disappeared".to_string())?;
    door.state = 0;
    gameobjects.guid().update(door);
    Ok(())
}

/// Verifies that the package tick restores the Sorcerer Door from durable encounter and group
/// state after the object was reloaded closed.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_shadowfang_restart_recovery(ctx: &ReducerContext) -> Result<(), String> {
    require(
        gameobject_state(ctx, fixture_gameobject_guid(27))? == DOOR_OPEN_STATE,
        "Shadowfang restart recovery left the Sorcerer Door closed",
    )
}

/// Verifies the first durable escort leg, then begins a second instance's awakening through the
/// same production encounter-notification boundary used by imported EventAI.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_wailing_escort_and_begin_awakening(ctx: &ReducerContext) -> Result<(), String> {
    let disciple_guid = fixture_guid(3678, 25);
    let disciple = ctx
        .db
        .game_world_entity()
        .guid()
        .find(disciple_guid)
        .ok_or_else(|| "Wailing escort Disciple disappeared".to_string())?;
    let spline = ctx
        .db
        .game_creature_spline()
        .guid()
        .find(disciple_guid)
        .ok_or_else(|| "Wailing escort emitted no durable movement spline".to_string())?;
    let targets_second_route_point = (spline.dx - -124.4064).abs() < 0.01
        && (spline.dy - 131.07953).abs() < 0.01
        && (spline.dz - -78.71027).abs() < 0.01;
    if !targets_second_route_point {
        let phase = ctx
            .db
            .wailing_escort_progress()
            .instance_id()
            .find(908)
            .map(|progress| progress.phase);
        let schedules = ctx
            .db
            .wailing_escort_schedule()
            .by_instance()
            .filter(&908u64)
            .count();
        return Err(format!(
            "Wailing escort emitted the wrong durable move leg: entity=({}, {}, {}), destination=({}, {}, {}), phase={phase:?}, schedules={schedules}",
            disciple.x, disciple.y, disciple.z, spline.dx, spline.dy, spline.dz
        ));
    }
    require(
        encounter::get_encounter_state(ctx, 908, 4) == ENCOUNTER_IN_PROGRESS,
        "Wailing escort left InProgress before the ritual completed",
    )?;

    clear_wailing_schedule(ctx, 908);
    ctx.db.game_creature_spline().guid().delete(disciple_guid);
    set_fixture_position(ctx, disciple_guid, -104.28827, 234.40804, -91.64163)?;
    set_fixture_position(ctx, fixture_guid(0, 29), -104.28827, 234.40804, -91.64163)?;
    let progress_table = ctx.db.wailing_escort_progress();
    let mut first_corner = progress_table
        .instance_id()
        .find(908)
        .ok_or_else(|| "Wailing escort progress disappeared".to_string())?;
    first_corner.phase = WAILING_ROUTE_ARRIVE_PHASE;
    first_corner.route_point = WAILING_FIRST_CORNER_POINT;
    progress_table.instance_id().update(first_corner);
    schedule_wailing_fixture(
        ctx,
        908,
        WAILING_ROUTE_ARRIVE_PHASE,
        WAILING_FIRST_CORNER_POINT,
        100_000,
    );

    install_creature_template(ctx, 3636)?;
    install_creature_template(ctx, 5762)?;
    install_creature_template(ctx, 5763)?;
    install_creature_template(ctx, 3654)?;
    install_spell(ctx, 6271, "Awakening")?;
    install_spell(ctx, 8153, "Naralex shapeshift")?;
    let source = spawn_source(ctx, 3671, 43, 921, true, 40)?;
    let disciple = spawn_source(ctx, 3678, 43, 921, false, 41)?;
    let _naralex = spawn_source(ctx, 3679, 43, 921, false, 42)?;
    let player = spawn_fixture_player(ctx, 43, 921, 43)?;
    complete_wailing_leaders(ctx, source)?;
    crate::world::apply_gossip_select(ctx, player, disciple, 0, 50_296)?;
    require(
        encounter::get_encounter_state(ctx, 921, 4) == ENCOUNTER_IN_PROGRESS,
        "second Wailing escort did not start through gossip",
    )?;
    clear_wailing_schedule(ctx, 921);
    let mut awaiting_mutanus = progress_table
        .instance_id()
        .find(921)
        .ok_or_else(|| "second Wailing escort progress disappeared".to_string())?;
    awaiting_mutanus.phase = WAILING_WAIT_MUTANUS_PHASE;
    awaiting_mutanus.route_point = 70;
    progress_table.instance_id().update(awaiting_mutanus);
    let mutanus = spawn_source(ctx, 3654, 43, 921, true, 44)?;
    notify(
        ctx,
        mutanus,
        EncounterBinding::WailingCavernsMutanus,
        EncounterSignal::Complete,
    )?;
    require(
        encounter::get_encounter_state(ctx, 921, 5) == ENCOUNTER_DONE,
        "Mutanus completion was not durable",
    )?;
    require(
        ctx.db
            .wailing_escort_schedule()
            .by_instance()
            .filter(&921u64)
            .count()
            == 1,
        "Mutanus completion did not arm one awakening callback",
    )
}

/// Verifies the point-12 source line while its one-second relay row is still live.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_wailing_first_corner_dialogue(ctx: &ReducerContext) -> Result<(), String> {
    require(
        ctx.db.game_chat_event().iter().any(|event| {
            event.message
                == "These caverns were once a temple of promise for regrowth in the Barrens. Now, they are the halls of nightmares."
        }),
        "Wailing first corner did not emit its source dialogue",
    )
}

/// Verifies the scheduled awakening outcome after Mutanus completed through production notify.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_wailing_awakening(ctx: &ReducerContext) -> Result<(), String> {
    require(
        encounter::get_encounter_state(ctx, 921, 4) == ENCOUNTER_DONE,
        "Naralex awakening did not complete the Disciple encounter",
    )?;
    let naralex = ctx
        .db
        .game_world_entity()
        .guid()
        .find(fixture_guid(3679, 42))
        .ok_or_else(|| "awakened Naralex disappeared".to_string())?;
    require(
        naralex.unit_bytes_1 & 0xFF == 0,
        "Naralex did not stand after awakening",
    )?;
    let progress = ctx
        .db
        .wailing_escort_progress()
        .instance_id()
        .find(921)
        .ok_or_else(|| "Wailing awakening progress disappeared".to_string())?;
    require(
        progress.phase == WAILING_DISCIPLE_AWAKE_PHASE,
        "Wailing awakening did not preserve the source-timed Disciple response",
    )?;

    let raptors: Vec<_> = ctx
        .db
        .game_world_entity()
        .by_map()
        .filter(&43u32)
        .filter(|entity| entity.instance_id == 908 && entity.entry == 3636 && !entity.dead)
        .collect();
    require(
        raptors.len() == 2,
        "Wailing first corner did not summon both Deviate Raptors",
    )?;
    require(
        raptors
            .iter()
            .all(|raptor| raptor.target_guid == fixture_guid(3678, 25)),
        "Wailing first-corner raptors did not engage the exact Disciple instance",
    )?;
    let player_guid = fixture_guid(0, 29);
    for raptor in raptors {
        require(
            crate::combat::kill_creature(ctx, raptor.guid, Some(player_guid)),
            "production creature death refused a Wailing first-corner raptor",
        )?;
    }
    Ok(())
}

/// Fast-forwards the completed awakening to the source exit phase. The scheduled package reducer
/// still owns the movement and cleanup outcomes verified by the next two reducers.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_prepare_wailing_exit(ctx: &ReducerContext) -> Result<(), String> {
    clear_wailing_schedule(ctx, 921);
    let progress_table = ctx.db.wailing_escort_progress();
    let mut progress = progress_table
        .instance_id()
        .find(921)
        .ok_or_else(|| "Wailing awakening progress disappeared before exit".to_string())?;
    progress.phase = WAILING_EXIT_PHASE;
    progress.route_point = 70;
    progress_table.instance_id().update(progress);
    schedule_wailing_fixture(ctx, 921, WAILING_EXIT_PHASE, 70, 100_000);
    Ok(())
}

/// Verifies the first scheduled exit leg for both druids, then shortens only the fixture's
/// package-owned cleanup callback.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_wailing_exit_and_prepare_cleanup(ctx: &ReducerContext) -> Result<(), String> {
    let disciple_target = ctx
        .db
        .game_creature_spline()
        .guid()
        .find(fixture_guid(3678, 41))
        .map(|spline| (spline.dx, spline.dy, spline.dz))
        .ok_or_else(|| "Wailing exit emitted no durable Disciple movement spline".to_string())?;
    let naralex_target = ctx
        .db
        .game_creature_spline()
        .guid()
        .find(fixture_guid(3679, 42))
        .map(|spline| (spline.dx, spline.dy, spline.dz))
        .ok_or_else(|| "Wailing exit emitted no durable Naralex movement spline".to_string())?;
    let dx = disciple_target.0 - naralex_target.0;
    let dy = disciple_target.1 - naralex_target.1;
    require(
        ((dx * dx + dy * dy).sqrt() - 5.0).abs() < 0.01,
        "Naralex did not target the source five-yard exit follow distance",
    )?;

    clear_wailing_schedule(ctx, 921);
    let progress_table = ctx.db.wailing_escort_progress();
    let mut progress = progress_table
        .instance_id()
        .find(921)
        .ok_or_else(|| "Wailing exit progress disappeared".to_string())?;
    progress.phase = WAILING_DESPAWN_PHASE;
    let route_point = progress.route_point;
    progress_table.instance_id().update(progress);
    schedule_wailing_fixture(ctx, 921, WAILING_DESPAWN_PHASE, route_point, 100_000);
    Ok(())
}

/// Verifies the source cleanup boundary: players stay, every non-player creature and every escort
/// row in the instance is gone, and the completed encounter remains durable.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_wailing_cleanup(ctx: &ReducerContext) -> Result<(), String> {
    require(
        ctx.db
            .game_world_entity()
            .by_map()
            .filter(&43u32)
            .filter(|entity| entity.instance_id == 921 && !entity.is_player())
            .next()
            .is_none(),
        "Wailing cleanup left a non-player creature in the instance",
    )?;
    require(
        ctx.db
            .game_world_entity()
            .guid()
            .find(fixture_guid(0, 43))
            .is_some_and(|entity| entity.is_player()),
        "Wailing cleanup despawned the participating player",
    )?;
    require(
        ctx.db
            .wailing_escort_progress()
            .instance_id()
            .find(921)
            .is_none()
            && ctx
                .db
                .wailing_escort_schedule()
                .by_instance()
                .filter(&921u64)
                .next()
                .is_none(),
        "Wailing cleanup left durable escort state behind",
    )?;
    require(
        encounter::get_encounter_state(ctx, 921, 4) == ENCOUNTER_DONE,
        "Wailing cleanup lost the completed Disciple encounter",
    )
}

/// Verifies that the last first-corner death resumed the package-owned route in source order.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_wailing_first_corner_continue(ctx: &ReducerContext) -> Result<(), String> {
    require(
        ctx.db.game_chat_event().iter().any(|event| {
            event.message
                == "Come. We must continue. There is much to be done before we can pull Naralex from his nightmare."
        }),
        "Wailing first-corner wave did not resume its source dialogue",
    )?;
    let progress = ctx
        .db
        .wailing_escort_progress()
        .instance_id()
        .find(908)
        .ok_or_else(|| "Wailing progress disappeared after the first-corner wave".to_string())?;
    require(
        progress.route_point >= 13,
        "Wailing first-corner wave did not resume the pinned route",
    )
}

/// Runs EventAI's production encounter notification boundary against package-owned durable state
/// and world outcomes. The standalone integration test is the public caller.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_eventai_instance_packages(ctx: &ReducerContext) -> Result<(), String> {
    verify_map_and_instance_gates(ctx)?;
    verify_standard_states(ctx)?;
    verify_ward_keeper_aggregation(ctx)?;
    verify_tomb_of_seven_reset(ctx)?;
    verify_alzzin(ctx)?;
    verify_shadowfang(ctx)?;
    verify_wailing_caverns_gate(ctx)?;
    verify_avatar_suppression(ctx)?;
    verify_mandokir_movement(ctx)?;
    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn verify_map_and_instance_gates(ctx: &ReducerContext) -> Result<(), String> {
    let wrong_map = spawn_source(ctx, 3914, 48, 901, false, 1)?;
    let error = encounter::notify_from_eventai(
        ctx,
        wrong_map,
        EncounterBinding::ShadowfangKeepRethilgore,
        EncounterSignal::Begin,
    )
    .expect_err("wrong-map notification must refuse");
    require(
        error.contains("belongs to map 33"),
        "wrong-map refusal lost its map account",
    )?;
    require(
        encounter::get_encounter_state(ctx, 901, 2) == ENCOUNTER_NOT_STARTED,
        "wrong-map notification changed encounter state",
    )?;

    let open_world = spawn_source(ctx, 3914, 33, 0, false, 2)?;
    let error = encounter::notify_from_eventai(
        ctx,
        open_world,
        EncounterBinding::ShadowfangKeepRethilgore,
        EncounterSignal::Begin,
    )
    .expect_err("open-world notification must refuse");
    require(
        error.contains("instance-scoped source"),
        "open-world refusal lost its instance account",
    )?;

    let missing_instance = spawn_source(ctx, 3914, 33, 912, false, 26)?;
    ctx.db.game_instance().instance_id().delete(912);
    let error = encounter::notify_from_eventai(
        ctx,
        missing_instance,
        EncounterBinding::ShadowfangKeepRethilgore,
        EncounterSignal::Begin,
    )
    .expect_err("missing-instance notification must refuse");
    require(
        error.contains("source instance 912 is missing"),
        "missing-instance refusal lost its instance account",
    )?;

    let mismatched_instance = spawn_source(ctx, 3914, 33, 913, false, 27)?;
    let mut instance = ctx
        .db
        .game_instance()
        .instance_id()
        .find(913)
        .ok_or_else(|| "fixture instance 913 disappeared".to_string())?;
    instance.map_id = 48;
    ctx.db.game_instance().instance_id().update(instance);
    let error = encounter::notify_from_eventai(
        ctx,
        mismatched_instance,
        EncounterBinding::ShadowfangKeepRethilgore,
        EncounterSignal::Begin,
    )
    .expect_err("instance-map mismatch must refuse");
    require(
        error.contains("does not match instance 913 map 48"),
        "instance-map refusal lost its map account",
    )
}

#[cfg(feature = "debug_reducers")]
fn verify_standard_states(ctx: &ReducerContext) -> Result<(), String> {
    let rethilgore = spawn_source(ctx, 3914, 33, 902, false, 3)?;
    notify(
        ctx,
        rethilgore,
        EncounterBinding::ShadowfangKeepRethilgore,
        EncounterSignal::Begin,
    )?;
    require(
        encounter::get_encounter_state(ctx, 902, 2) == ENCOUNTER_IN_PROGRESS,
        "begin did not enter InProgress",
    )?;
    notify(
        ctx,
        rethilgore,
        EncounterBinding::ShadowfangKeepRethilgore,
        EncounterSignal::Fail,
    )?;
    require(
        encounter::get_encounter_state(ctx, 902, 2) == ENCOUNTER_FAILED,
        "fail did not enter Failed",
    )?;

    let kelris = spawn_source(ctx, 4832, 48, 903, true, 4)?;
    notify(
        ctx,
        kelris,
        EncounterBinding::BlackfathomDeepsKelris,
        EncounterSignal::Complete,
    )?;
    require(
        encounter::get_encounter_state(ctx, 903, 1) == ENCOUNTER_DONE,
        "complete did not enter Done",
    )
}

#[cfg(feature = "debug_reducers")]
fn verify_ward_keeper_aggregation(ctx: &ReducerContext) -> Result<(), String> {
    let first = spawn_source(ctx, 4625, 47, 904, true, 5)?;
    let second = spawn_source(ctx, 4625, 47, 904, false, 6)?;
    let ward = spawn_gameobject(ctx, 21099, 47, 904, 7)?;
    notify(
        ctx,
        first,
        EncounterBinding::RazorfenKraulWardKeepers,
        EncounterSignal::Complete,
    )?;
    require(
        encounter::get_encounter_state(ctx, 904, 1) == ENCOUNTER_NOT_STARTED,
        "ward opened before the last keeper died",
    )?;
    let mut last = ctx
        .db
        .game_world_entity()
        .guid()
        .find(second)
        .ok_or_else(|| "second Ward Keeper disappeared".to_string())?;
    last.dead = true;
    last.health = 0;
    ctx.db.game_world_entity().guid().update(last);
    notify(
        ctx,
        second,
        EncounterBinding::RazorfenKraulWardKeepers,
        EncounterSignal::Complete,
    )?;
    require(
        encounter::get_encounter_state(ctx, 904, 1) == ENCOUNTER_DONE,
        "last Ward Keeper did not complete the encounter",
    )?;
    require(
        gameobject_state(ctx, ward)? == DOOR_OPEN_STATE,
        "Ward stayed closed",
    )
}

#[cfg(feature = "debug_reducers")]
fn verify_tomb_of_seven_reset(ctx: &ReducerContext) -> Result<(), String> {
    let source = spawn_source(ctx, 9034, 230, 905, false, 8)?;
    let dead_dwarf = spawn_source(ctx, 9035, 230, 905, true, 9)?;
    let entrance = spawn_gameobject(ctx, 170576, 230, 905, 10)?;
    notify(
        ctx,
        source,
        EncounterBinding::BlackrockDepthsTombOfSeven,
        EncounterSignal::Fail,
    )?;
    let dwarf = ctx
        .db
        .game_world_entity()
        .guid()
        .find(dead_dwarf)
        .ok_or_else(|| "Tomb dwarf disappeared on reset".to_string())?;
    require(
        !dwarf.dead && dwarf.health == dwarf.max_health,
        "Tomb dwarf did not revive",
    )?;
    require(
        encounter::get_encounter_state(ctx, 905, 4) == ENCOUNTER_FAILED,
        "Tomb failure state was not durable",
    )?;
    require(
        gameobject_state(ctx, entrance)? == DOOR_OPEN_STATE,
        "Tomb entrance did not reopen on failure",
    )
}

#[cfg(feature = "debug_reducers")]
fn verify_alzzin(ctx: &ReducerContext) -> Result<(), String> {
    let source = spawn_source(ctx, 11492, 429, 906, false, 11)?;
    let wall = spawn_gameobject(ctx, 177220, 429, 906, 12)?;
    let vine = spawn_gameobject(ctx, 179502, 429, 906, 13)?;
    let shard = spawn_gameobject(ctx, 179559, 429, 906, 21)?;
    let mut depleted_shard = ctx
        .db
        .game_gameobject()
        .guid()
        .find(shard)
        .ok_or_else(|| "Felvine shard disappeared".to_string())?;
    depleted_shard.state = 1;
    depleted_shard.respawn_at_micros = 99;
    ctx.db.game_gameobject().guid().update(depleted_shard);
    notify(
        ctx,
        source,
        EncounterBinding::DireMaulAlzzin,
        EncounterSignal::BreakAlzzinCrumbleWall,
    )?;
    require(
        gameobject_state(ctx, wall)? == DOOR_OPEN_STATE,
        "Alzzin wall stayed closed",
    )?;
    notify(
        ctx,
        source,
        EncounterBinding::DireMaulAlzzin,
        EncounterSignal::Complete,
    )?;
    require(
        gameobject_state(ctx, vine)? == DOOR_OPEN_STATE,
        "Alzzin vine stayed closed",
    )?;
    let shard = ctx
        .db
        .game_gameobject()
        .guid()
        .find(shard)
        .ok_or_else(|| "Felvine shard disappeared on completion".to_string())?;
    require(
        shard.state == 0 && shard.respawn_at_micros == 0,
        "Alzzin completion did not respawn Felvine shards",
    )?;
    require(
        encounter::get_encounter_state(ctx, 906, 0) == ENCOUNTER_DONE,
        "Alzzin completion was not durable",
    )
}

#[cfg(feature = "debug_reducers")]
fn verify_shadowfang(ctx: &ReducerContext) -> Result<(), String> {
    install_creature_template(ctx, 4275)?;
    install_creature_template(ctx, 4627)?;
    set_creature_template_faction(ctx, 4627, 14)?;
    install_spell(ctx, 6422, "Archmage Arugal fire")?;
    install_heal_spell(ctx, 7154, "Dark Offering", 300, 10)?;
    let player = spawn_fixture_player(ctx, 33, 907, 46)?;
    set_fixture_position(ctx, player, -159.547, 2178.11, 128.944)?;
    let _ada = spawn_source(ctx, 3849, 33, 907, false, 22)?;
    let _ash = spawn_source(ctx, 3850, 33, 907, false, 23)?;
    let rethilgore = spawn_source(ctx, 3914, 33, 907, true, 24)?;
    notify(
        ctx,
        rethilgore,
        EncounterBinding::ShadowfangKeepRethilgore,
        EncounterSignal::Complete,
    )?;
    require(
        ctx.db
            .game_chat_event()
            .iter()
            .filter(|event| {
                event.message == "About time someone killed the wretch."
                    || event.message == "For once I agree with you... scum."
            })
            .count()
            == 2,
        "Rethilgore completion did not emit both prisoner lines",
    )?;
    let fenrus = spawn_source(ctx, 4274, 33, 907, true, 14)?;
    let _focus = spawn_gameobject(ctx, 18973, 33, 907, 26)?;
    let _sorcerer_door = spawn_gameobject(ctx, 18972, 33, 907, 27)?;
    notify(
        ctx,
        fenrus,
        EncounterBinding::ShadowfangKeepFenrus,
        EncounterSignal::Complete,
    )?;
    let arugal_spawned = ctx
        .db
        .game_world_entity()
        .by_map()
        .filter(&33u32)
        .any(|entity| entity.instance_id == 907 && entity.entry == 4275 && !entity.dead);
    require(
        !arugal_spawned,
        "Archmage Arugal was visible before his cue",
    )?;
    require(
        ctx.db
            .shadowfang_fenrus_choreography()
            .by_instance()
            .filter(&907u64)
            .count()
            == 1,
        "Fenrus completion did not arm one durable choreography step",
    )?;

    let nandos = spawn_source(ctx, 3927, 33, 907, true, 15)?;
    let door = spawn_gameobject(ctx, 18971, 33, 907, 16)?;
    notify(
        ctx,
        nandos,
        EncounterBinding::ShadowfangKeepNandos,
        EncounterSignal::Complete,
    )?;
    require(
        gameobject_state(ctx, door)? == DOOR_OPEN_STATE,
        "Nandos door stayed closed",
    )
}

#[cfg(feature = "debug_reducers")]
fn verify_wailing_caverns_gate(ctx: &ReducerContext) -> Result<(), String> {
    install_creature_template(ctx, 3678)?;
    let source = spawn_source(ctx, 3671, 43, 908, true, 17)?;
    let disciple = spawn_source(ctx, 3678, 43, 908, false, 25)?;
    set_fixture_position(ctx, disciple, -134.96526, 125.40187, -78.09446)?;
    let _naralex = spawn_source(ctx, 3679, 43, 908, false, 28)?;
    let player = spawn_fixture_player(ctx, 43, 908, 29)?;
    complete_wailing_leaders(ctx, source)?;
    require(
        encounter::get_encounter_data(ctx, 908, 4) == 1,
        "four Wailing Caverns leaders did not make the Disciple escort ready",
    )?;
    require(
        ctx.db.game_chat_event().iter().any(|event| {
            event.message == "At last! Naralex can be awakened! Come aid me, brave adventurers!"
        }),
        "Wailing Caverns gate did not emit the Disciple intro",
    )?;
    let menu = ctx
        .db
        .game_creature_gossip_menu_override()
        .creature_guid()
        .find(disciple)
        .ok_or_else(|| "Wailing Caverns gate did not render its start menu".to_string())?;
    require(
        menu.menu_id == 3_678
            && ctx
                .db
                .game_gossip_menu_profile_option()
                .row_id()
                .find(50_296)
                .is_some_and(|option| option.menu_id == menu.menu_id),
        "Wailing Caverns start menu is missing its package option",
    )?;
    crate::world::apply_gossip_select(ctx, player, disciple, 0, 50_296)?;
    require(
        ctx.db
            .game_creature_gossip_menu_override()
            .creature_guid()
            .find(disciple)
            .is_none(),
        "Wailing Caverns start menu remained active after selection",
    )?;
    let disciple = ctx
        .db
        .game_world_entity()
        .guid()
        .find(disciple)
        .ok_or_else(|| "Wailing Caverns Disciple disappeared".to_string())?;
    require(
        disciple.faction_template == 250
            && encounter::get_encounter_state(ctx, 908, 4) == ENCOUNTER_IN_PROGRESS,
        "Wailing start gossip did not begin the escort",
    )?;
    require(
        ctx.db
            .wailing_escort_schedule()
            .by_instance()
            .filter(&908u64)
            .count()
            == 1,
        "Wailing start gossip did not arm one durable escort step",
    )
}

#[cfg(feature = "debug_reducers")]
fn complete_wailing_leaders(ctx: &ReducerContext, source: u64) -> Result<(), String> {
    for binding in [
        EncounterBinding::WailingCavernsAnacondra,
        EncounterBinding::WailingCavernsCobrahn,
        EncounterBinding::WailingCavernsPythas,
        EncounterBinding::WailingCavernsSerpentis,
    ] {
        notify(ctx, source, binding, EncounterSignal::Complete)?;
    }
    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn clear_wailing_schedule(ctx: &ReducerContext, instance_id: u64) {
    let schedules = ctx.db.wailing_escort_schedule();
    let ids: Vec<u64> = schedules
        .by_instance()
        .filter(&instance_id)
        .map(|scheduled| scheduled.scheduled_id)
        .collect();
    for id in ids {
        schedules.scheduled_id().delete(id);
    }
}

#[cfg(feature = "debug_reducers")]
fn schedule_wailing_fixture(
    ctx: &ReducerContext,
    instance_id: u64,
    phase: u8,
    route_point: u8,
    delay_micros: i64,
) {
    let scheduled_at = ScheduleAt::Time(
        ctx.timestamp
            .checked_add(TimeDuration::from_micros(delay_micros))
            .unwrap_or(ctx.timestamp),
    );
    ctx.db
        .wailing_escort_schedule()
        .insert(WailingEscortSchedule {
            scheduled_id: 0,
            scheduled_at,
            instance_id,
            phase,
            route_point,
        });
}

#[cfg(feature = "debug_reducers")]
fn verify_avatar_suppression(ctx: &ReducerContext) -> Result<(), String> {
    let source = spawn_source(ctx, 8440, 109, 909, false, 18)?;
    notify(
        ctx,
        source,
        EncounterBinding::SunkenTempleAvatar,
        EncounterSignal::Begin,
    )?;
    notify(
        ctx,
        source,
        EncounterBinding::SunkenTempleAvatar,
        EncounterSignal::InterruptAvatarSuppression,
    )?;
    require(
        ctx.db
            .sunken_temple_suppression()
            .by_instance()
            .filter(&909u64)
            .count()
            == 1,
        "Avatar suppression did not arm exactly one durable timer",
    )?;
    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn verify_mandokir_movement(ctx: &ReducerContext) -> Result<(), String> {
    let source = spawn_source(ctx, 11391, 309, 910, true, 19)?;
    let mandokir = spawn_source(ctx, 11382, 309, 910, false, 20)?;
    notify(
        ctx,
        source,
        EncounterBinding::ZulGurubOhgan,
        EncounterSignal::SendMandokirDownstairs,
    )?;
    let mandokir = ctx
        .db
        .game_world_entity()
        .guid()
        .find(mandokir)
        .ok_or_else(|| "Mandokir disappeared".to_string())?;
    require(
        (mandokir.x - -12196.30).abs() < 0.01
            && (mandokir.y - -1948.37).abs() < 0.01
            && (mandokir.z - 130.31).abs() < 0.01,
        "Mandokir did not move downstairs",
    )
}

#[cfg(feature = "debug_reducers")]
fn notify(
    ctx: &ReducerContext,
    source_guid: u64,
    binding: EncounterBinding,
    signal: EncounterSignal,
) -> Result<(), String> {
    encounter::notify_from_eventai(ctx, source_guid, binding, signal).map_err(|error| {
        format!("{binding:?} refused installed package signal {signal:?}: {error}")
    })
}

#[cfg(feature = "debug_reducers")]
fn spawn_source(
    ctx: &ReducerContext,
    entry: u32,
    map_id: u32,
    instance_id: u64,
    dead: bool,
    sequence: u64,
) -> Result<u64, String> {
    if instance_id != 0 {
        install_instance(ctx, map_id, instance_id)?;
    }
    let entities = ctx.db.game_world_entity();
    let mut source = entities
        .by_map()
        .filter(&0u32)
        .find(|entity| !entity.is_player())
        .ok_or_else(|| "fixture needs one seeded creature".to_string())?;
    let guid = fixture_guid(entry, sequence);
    entities.guid().delete(guid);
    source.guid = guid;
    source.entry = entry;
    source.map_id = map_id;
    source.instance_id = instance_id;
    source.dead = dead;
    source.health = if dead { 0 } else { source.max_health.max(1) };
    entities.insert(source);
    Ok(guid)
}

#[cfg(feature = "debug_reducers")]
fn spawn_fixture_player(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    sequence: u64,
) -> Result<u64, String> {
    install_instance(ctx, map_id, instance_id)?;
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .by_map()
        .filter(&0u32)
        .find(|entity| !entity.is_player())
        .ok_or_else(|| "fixture needs one seeded creature".to_string())?;
    let guid = fixture_guid(0, sequence);
    entities.guid().delete(guid);
    player.guid = guid;
    player.entry = 0;
    player.map_id = map_id;
    player.instance_id = instance_id;
    player.type_mask = lyracore_shared::constants::type_mask::PLAYER;
    player.dead = false;
    player.health = 1_000_000;
    player.max_health = 1_000_000;
    player.faction_template = 1;
    entities.insert(player);
    Ok(guid)
}

#[cfg(feature = "debug_reducers")]
fn set_fixture_position(
    ctx: &ReducerContext,
    guid: u64,
    x: f32,
    y: f32,
    z: f32,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut entity = entities
        .guid()
        .find(guid)
        .ok_or_else(|| format!("fixture entity {guid} disappeared"))?;
    let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(x, y);
    entity.x = x;
    entity.y = y;
    entity.z = z;
    entity.grid_x = grid_x;
    entity.grid_y = grid_y;
    entity.cell = lyracore_shared::spatial::grid_cell_id(grid_x, grid_y);
    entities.guid().update(entity);
    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn fixture_guid(entry: u32, sequence: u64) -> u64 {
    encounter::wave_guid(entry, FIXTURE_LOW_BAND | sequence)
}

#[cfg(feature = "debug_reducers")]
fn install_instance(ctx: &ReducerContext, map_id: u32, instance_id: u64) -> Result<(), String> {
    let instances = ctx.db.game_instance();
    match instances.instance_id().find(instance_id) {
        Some(instance) if instance.map_id == map_id => Ok(()),
        Some(instance) => Err(format!(
            "fixture instance {instance_id} is already on map {}, not {map_id}",
            instance.map_id
        )),
        None => {
            instances.insert(GameInstance {
                instance_id,
                map_id,
                party_id: 0,
                created_at: ctx.timestamp,
                last_empty_at_micros: 0,
                reset_requested: false,
            });
            Ok(())
        }
    }
}

#[cfg(feature = "debug_reducers")]
fn install_creature_template(ctx: &ReducerContext, entry: u32) -> Result<(), String> {
    let templates = ctx.db.game_creature_template();
    if templates.entry().find(entry).is_some() {
        return Ok(());
    }
    let mut template = templates
        .iter()
        .next()
        .ok_or_else(|| "fixture needs one seeded creature template".to_string())?;
    template.entry = entry;
    template.name = format!("Encounter fixture {entry}");
    templates.insert(template);
    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn set_creature_template_faction(
    ctx: &ReducerContext,
    entry: u32,
    faction_template: u32,
) -> Result<(), String> {
    let templates = ctx.db.game_creature_template();
    let mut template = templates
        .entry()
        .find(entry)
        .ok_or_else(|| format!("fixture creature template {entry} disappeared"))?;
    template.faction_template = faction_template;
    templates.entry().update(template);
    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn install_spell(ctx: &ReducerContext, spell_id: u32, name: &str) -> Result<(), String> {
    let spells = ctx.db.game_spell();
    if spells.spell_id().find(spell_id).is_some() {
        return Ok(());
    }
    let mut spell = spells
        .iter()
        .next()
        .ok_or_else(|| "fixture needs one seeded spell".to_string())?;
    spell.spell_id = spell_id;
    spell.name = name.to_string();
    spell.cost = 0;
    spell.cast_time_ms = 0;
    spell.gcd_ms = 0;
    spell.cooldown_ms = 0;
    spell.range_yd = 0;
    spell.attributes = 0;
    spell.cast_flags = 0;
    spell.stances = 0;
    spells.insert(spell);
    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn install_heal_spell(
    ctx: &ReducerContext,
    spell_id: u32,
    name: &str,
    healing: i32,
    range_yd: u32,
) -> Result<(), String> {
    install_spell(ctx, spell_id, name)?;
    let spells = ctx.db.game_spell();
    let mut spell = spells
        .spell_id()
        .find(spell_id)
        .ok_or_else(|| format!("fixture spell {spell_id} disappeared"))?;
    spell.range_yd = range_yd;
    spell.school_mask = 32;
    spell.is_negative = false;
    spells.spell_id().update(spell);

    let effects = ctx.db.game_spell_effect();
    let id = (u64::from(spell_id) << 2) | 0;
    effects.id().delete(id);
    effects.insert(SpellEffect {
        id,
        spell_id,
        effect_index: 0,
        kind: 0x02,
        base_points: healing,
        die_sides: 0,
        per_level: 0.0,
        period_ms: 0,
        target: 2,
        radius_yd: 0.0,
        chain_targets: 0,
        trigger_spell: 0,
        effect_mechanic: 0,
        p0: 0,
        p0_kind: 0,
        p1: 0,
        script_id: 0,
        enters_combat: false,
    });
    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn spawn_gameobject(
    ctx: &ReducerContext,
    entry: u32,
    map_id: u32,
    instance_id: u64,
    sequence: u64,
) -> Result<u64, String> {
    let gameobjects = ctx.db.game_gameobject();
    let mut gameobject = gameobjects
        .by_map()
        .filter(&0u32)
        .next()
        .ok_or_else(|| "fixture needs one seeded gameobject".to_string())?;
    let guid = fixture_gameobject_guid(sequence);
    gameobjects.guid().delete(guid);
    gameobject.guid = guid;
    gameobject.template_entry = entry;
    gameobject.map_id = map_id;
    gameobject.instance_id = instance_id;
    gameobject.state = 0;
    gameobject.respawn_at_micros = 0;
    gameobjects.insert(gameobject);
    Ok(guid)
}

#[cfg(feature = "debug_reducers")]
fn fixture_gameobject_guid(sequence: u64) -> u64 {
    (0xF110u64 << 48) | FIXTURE_LOW_BAND | sequence
}

#[cfg(feature = "debug_reducers")]
fn gameobject_state(ctx: &ReducerContext, guid: u64) -> Result<u8, String> {
    ctx.db
        .game_gameobject()
        .guid()
        .find(guid)
        .map(|gameobject| gameobject.state)
        .ok_or_else(|| format!("fixture gameobject {guid} disappeared"))
}

#[cfg(feature = "debug_reducers")]
fn require(condition: bool, error: &str) -> Result<(), String> {
    condition.then_some(()).ok_or_else(|| error.to_string())
}
