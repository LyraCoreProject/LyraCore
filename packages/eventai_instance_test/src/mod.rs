#[cfg(feature = "debug_reducers")]
use spacetimedb::{reducer, ReducerContext, Table};

#[cfg(feature = "debug_reducers")]
use crate::encounter::{
    self, EncounterBinding, EncounterSignal, DOOR_OPEN_STATE, ENCOUNTER_DONE, ENCOUNTER_FAILED,
    ENCOUNTER_IN_PROGRESS, ENCOUNTER_NOT_STARTED,
};
#[cfg(feature = "debug_reducers")]
use crate::pkg_sunken_temple::sunken_temple_suppression;
#[cfg(feature = "debug_reducers")]
use crate::{
    game_chat_event, game_creature_template, game_gameobject, game_instance, game_world_entity,
    GameInstance,
};

#[cfg(feature = "debug_reducers")]
const FIXTURE_LOW_BAND: u64 = 0x10_0000;

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
        arugal_spawned,
        "Fenrus completion did not summon Archmage Arugal",
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
    let _disciple = spawn_source(ctx, 3678, 43, 908, false, 25)?;
    for binding in [
        EncounterBinding::WailingCavernsAnacondra,
        EncounterBinding::WailingCavernsCobrahn,
        EncounterBinding::WailingCavernsPythas,
        EncounterBinding::WailingCavernsSerpentis,
    ] {
        notify(ctx, source, binding, EncounterSignal::Complete)?;
    }
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
    notify(
        ctx,
        source,
        EncounterBinding::WailingCavernsMutanus,
        EncounterSignal::Complete,
    )?;
    require(
        encounter::get_encounter_state(ctx, 908, 5) == ENCOUNTER_DONE,
        "Mutanus completion was not durable",
    )
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
    let guid = encounter::wave_guid(entry, FIXTURE_LOW_BAND | sequence);
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
    let guid = (0xF110u64 << 48) | FIXTURE_LOW_BAND | sequence;
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
