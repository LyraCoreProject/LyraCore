#[cfg(feature = "debug_reducers")]
use spacetimedb::{reducer, ReducerContext, Table};

#[cfg(feature = "debug_reducers")]
use crate::encounter::{
    self, EncounterBinding, EncounterSignal, DOOR_OPEN_STATE, ENCOUNTER_DONE, ENCOUNTER_FAILED,
    ENCOUNTER_IN_PROGRESS, ENCOUNTER_NOT_STARTED,
};
#[cfg(feature = "debug_reducers")]
use crate::pkg_blackrock_depths::blackrock_tomb_round;
#[cfg(feature = "debug_reducers")]
use crate::pkg_shadowfang_keep::shadowfang_fenrus_choreography;
#[cfg(feature = "debug_reducers")]
use crate::pkg_sunken_temple::sunken_temple_suppression;
#[cfg(feature = "debug_reducers")]
use crate::pkg_wailing_caverns::{wailing_escort_progress, wailing_escort_schedule};
#[cfg(feature = "debug_reducers")]
use crate::{
    game_chat_event, game_creature_spline, game_creature_template, game_gameobject, game_instance,
    game_spell, game_world_entity, GameInstance,
};

#[cfg(feature = "debug_reducers")]
const FIXTURE_LOW_BAND: u64 = 0x10_0000;
#[cfg(feature = "debug_reducers")]
const TOMB_SCHEDULER_INSTANCE: u64 = 920;

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
    for (x, y, z) in [
        (-155.352, 2172.780, 128.448),
        (-147.059, 2163.193, 128.696),
        (-148.869, 2180.859, 128.448),
        (-140.203, 2175.263, 128.448),
    ] {
        require(
            voidwalkers.iter().any(|entity| {
                (entity.x - x).abs() < 0.01
                    && (entity.y - y).abs() < 0.01
                    && (entity.z - z).abs() < 0.01
            }),
            "an Arugal Voidwalker was not at its source position",
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
    let targets_first_corner = (spline.dx - -104.28827).abs() < 0.01
        && (spline.dy - 234.40804).abs() < 0.01
        && (spline.dz - -91.64163).abs() < 0.01;
    let targets_circle = (spline.dx - -54.713943).abs() < 0.01
        && (spline.dy - 273.85025).abs() < 0.01
        && (spline.dz - -92.84426).abs() < 0.01;
    if !targets_first_corner && !targets_circle {
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
        progress.phase == 12,
        "Wailing awakening did not advance to the Disciple response",
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
    install_spell(ctx, 6422, "Archmage Arugal fire")?;
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
    let source = spawn_source(ctx, 3671, 43, 908, true, 17)?;
    let disciple = spawn_source(ctx, 3678, 43, 908, false, 25)?;
    set_fixture_position(ctx, disciple, -105.0, 233.0, -91.6)?;
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
    crate::world::apply_gossip_select(ctx, player, disciple, 0, 50_296)?;
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
