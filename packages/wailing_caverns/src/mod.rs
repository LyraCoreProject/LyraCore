use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::encounter::{
    self, EncounterSignal, ENCOUNTER_DONE, ENCOUNTER_FAILED, ENCOUNTER_IN_PROGRESS,
    ENCOUNTER_NOT_STARTED,
};
use crate::{
    game_encounter_spawn, game_gossip_menu_profile, game_gossip_menu_profile_option, game_instance,
    game_melee_attack, game_world_entity, GossipMenuProfile, GossipMenuProfileOption,
};

const MAP_ID: u32 = 43;
const DISCIPLE_ENCOUNTER_ID: u32 = 4;
const DISCIPLE_ESCORT_READY: u32 = 1;
const DISCIPLE_OF_NARALEX: u32 = 3678;
const NARALEX: u32 = 3679;
const MUTANUS: u32 = 3654;
const DEVIATE_RAPTOR: u32 = 3636;
const DEVIATE_VIPER: u32 = 5755;
const DEVIATE_ADDER: u32 = 5048;
const DEVIATE_MOCCASIN: u32 = 5762;
const NIGHTMARE_ECTOPLASM: u32 = 5763;
const WAILING_START_OPTION_ROW: u32 = 50_296;
const WAILING_START_MENU: u32 = 3_678;
const WAILING_START_TEXT: u32 = 699;
const ESCORT_FACTION: u32 = 250;
const SLEEP: u32 = 1090;
const POTION: u32 = 8141;
const CLEANSING: u32 = 6270;
const AWAKENING: u32 = 6271;
const SHAPESHIFT: u32 = 8153;

const EMOTE_TALK: u32 = 1;
const EMOTE_BOW: u32 = 2;
const EMOTE_BEG: u32 = 20;
const EMOTE_POINT: u32 = 25;

const PHASE_PREPARE: u8 = 0;
const PHASE_ROUTE_MOVE: u8 = 1;
const PHASE_ROUTE_ARRIVE: u8 = 2;
const PHASE_SPAWN_RAPTORS: u8 = 3;
const PHASE_WAIT_RAPTORS: u8 = 4;
const PHASE_CONTINUE_AFTER_RAPTORS: u8 = 5;
const PHASE_CLEANSE_CIRCLE: u8 = 6;
const PHASE_SPAWN_CIRCLE_WAVE: u8 = 7;
const PHASE_WAIT_CIRCLE_WAVE: u8 = 8;
const PHASE_CIRCLE_PURIFIED: u8 = 9;
const PHASE_BEGIN_RITUAL: u8 = 10;
const PHASE_CAST_AWAKENING: u8 = 11;
const PHASE_SPAWN_MOCCASINS: u8 = 12;
const PHASE_NARALEX_RESTLESS: u8 = 13;
const PHASE_WAIT_MOCCASINS: u8 = 14;
const PHASE_SPAWN_ECTOPLASMS: u8 = 15;
const PHASE_ECTOPLASM_BREAKTHROUGH: u8 = 16;
const PHASE_WAIT_ECTOPLASMS: u8 = 17;
const PHASE_SPAWN_MUTANUS: u8 = 18;
const PHASE_WAIT_MUTANUS: u8 = 19;
const PHASE_NARALEX_AWAKE: u8 = 20;
const PHASE_DISCIPLE_AWAKE: u8 = 21;
const PHASE_NARALEX_THANKS: u8 = 22;
const PHASE_FAREWELL: u8 = 23;
const PHASE_SHAPESHIFT: u8 = 24;
const PHASE_EXIT: u8 = 25;
const PHASE_DESPAWN: u8 = 26;

const RAPTOR_WAVE: [(u32, f32, f32, f32, f32); 2] = [
    (DEVIATE_RAPTOR, -67.44779, 214.5348, -93.42037, 0.0),
    (DEVIATE_RAPTOR, -67.85276, 203.7873, -93.57328, 0.0),
];

const CIRCLE_WAVE_POSITIONS: [(f32, f32, f32, f32); 3] = [
    (-50.1237, 274.7166, -92.7608, 3.0368),
    (-60.2538, 273.0981, -92.7608, 0.4014),
    (-57.5452, 280.2068, -92.7608, 5.0789),
];

const MOCCASIN_WAVE: [(u32, f32, f32, f32, f32); 3] = [
    (DEVIATE_MOCCASIN, 171.39545, 213.76605, -105.50746, 0.0),
    (DEVIATE_MOCCASIN, 156.72229, 189.91829, -107.48995, 0.0),
    (DEVIATE_MOCCASIN, 121.39977, 166.31746, -105.54061, 0.0),
];

const ECTOPLASM_WAVE: [(u32, f32, f32, f32, f32); 7] = [
    (NIGHTMARE_ECTOPLASM, 162.06705, 218.71494, -105.36240, 0.0),
    (NIGHTMARE_ECTOPLASM, 115.55489, 168.22847, -105.68655, 0.0),
    (NIGHTMARE_ECTOPLASM, 82.065025, 280.37723, -103.29671, 0.0),
    (NIGHTMARE_ECTOPLASM, 144.84305, 278.07928, -104.57445, 0.0),
    (NIGHTMARE_ECTOPLASM, 155.84459, 186.68817, -107.08412, 0.0),
    (NIGHTMARE_ECTOPLASM, 145.35356, 219.34600, -102.98572, 0.0),
    (NIGHTMARE_ECTOPLASM, 164.62735, 274.12335, -107.29780, 0.0),
];

const MUTANUS_WAVE: [(u32, f32, f32, f32, f32); 1] =
    [(MUTANUS, 150.94276, 262.79715, -103.90348, 0.0)];

const WAILING_ROUTE: [(f32, f32, f32); 79] = [
    (-134.96526, 125.40187, -78.09446),
    (-124.4064, 131.07953, -78.71027),
    (-113.91917, 142.769, -80.91416),
    (-111.16669, 153.64728, -80.55562),
    (-110.97073, 165.60736, -79.444725),
    (-109.30049, 181.25143, -79.76007),
    (-110.1942, 190.9626, -80.42992),
    (-109.58964, 199.15425, -81.23881),
    (-110.56909, 206.86935, -82.88934),
    (-110.30787, 216.23227, -85.9362),
    (-108.06385, 227.87166, -89.92641),
    (-104.28827, 234.40804, -91.64163),
    (-104.28827, 234.40804, -91.64163),
    (-98.08711, 229.5188, -91.07548),
    (-93.777916, 228.44995, -90.61347),
    (-85.272385, 227.1592, -93.12241),
    (-81.619774, 223.63588, -93.59701),
    (-78.13694, 219.21017, -94.11092),
    (-71.02429, 212.48766, -93.52012),
    (-66.8499, 209.88943, -93.305),
    (-61.41215, 207.00401, -93.55031),
    (-49.681805, 204.15556, -95.96281),
    (-41.188667, 204.99422, -96.51605),
    (-35.381386, 212.98494, -96.097084),
    (-33.46709, 223.27979, -95.67591),
    (-31.895458, 231.73523, -94.46018),
    (-33.1077, 240.3214, -93.595955),
    (-38.079693, 250.99289, -93.11742),
    (-43.072884, 259.32315, -92.84187),
    (-54.713943, 273.85025, -92.84426),
    (-50.375507, 279.56708, -92.84426),
    (-48.92133, 284.29807, -92.84426),
    (-49.880116, 287.8062, -92.245026),
    (-50.754616, 291.34595, -91.38129),
    (-47.99677, 295.32455, -90.81825),
    (-44.49994, 299.743, -90.212395),
    (-38.468487, 306.85004, -89.96176),
    (-34.199474, 309.35944, -89.575645),
    (-27.884659, 311.9566, -89.1139),
    (-23.459465, 310.78976, -88.51482),
    (-17.556763, 308.8638, -88.62951),
    (-9.611309, 305.38797, -88.19709),
    (-3.915017, 301.293, -86.81481),
    (-0.235204, 294.51816, -85.46128),
    (3.204272, 288.31448, -85.4905),
    (6.725203, 282.79242, -85.65187),
    (10.753059, 278.38467, -85.8368),
    (16.977774, 273.39676, -86.238335),
    (25.977861, 264.6067, -86.788),
    (29.228964, 257.175, -87.57598),
    (30.13821, 248.86757, -87.37655),
    (34.11451, 244.73685, -87.19066),
    (38.86757, 240.09755, -87.61623),
    (43.998184, 234.17651, -88.02119),
    (48.403416, 229.11066, -88.38168),
    (49.750046, 222.36626, -88.75966),
    (54.5601, 210.85043, -89.80927),
    (67.69076, 205.52835, -92.56347),
    (71.46944, 206.84753, -93.10892),
    (77.15617, 209.72255, -93.07799),
    (80.431854, 214.86958, -93.177925),
    (83.29396, 220.0915, -93.72824),
    (86.2444, 225.64459, -94.54216),
    (89.99905, 229.58038, -95.01256),
    (94.13328, 232.7041, -95.36724),
    (99.00888, 233.9866, -95.57549),
    (104.59536, 233.409, -95.845955),
    (109.07845, 232.6175, -96.046),
    (112.8708, 233.57512, -96.32111),
    (114.51453, 235.30222, -96.1607),
    (127.385, 252.279, -90.07),
    (121.595, 264.488, -91.55),
    (115.472, 264.253, -91.5),
    (99.988, 252.79, -91.51),
    (96.347, 245.038, -90.34),
    (82.201, 216.273, -86.1),
    (75.112, 206.494, -84.8),
    (27.174, 201.064, -72.31),
    (-41.114, 204.149, -78.94),
];

#[derive(Clone)]
#[table(accessor = wailing_escort_progress)]
pub struct WailingEscortProgress {
    #[primary_key]
    pub instance_id: u64,
    pub disciple_guid: u64,
    pub naralex_guid: u64,
    pub phase: u8,
    #[default(1u8)]
    pub route_point: u8,
    #[default(false)]
    pub running: bool,
    #[default(0i64)]
    pub next_potion_at_micros: i64,
    #[default(0i64)]
    pub next_sleep_at_micros: i64,
}

#[table(
    accessor = wailing_escort_schedule,
    scheduled(advance_wailing_escort),
    index(accessor = by_instance, btree(columns = [instance_id]))
)]
pub struct WailingEscortSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub instance_id: u64,
    pub phase: u8,
    #[default(0u8)]
    pub route_point: u8,
}

crate::encounter_package!(WailingCavernsAnacondra, fn anacondra(ctx, instance_id, signal) {
    set_boss_state(ctx, instance_id, 0, signal, "Anacondra")
});

crate::encounter_package!(WailingCavernsCobrahn, fn cobrahn(ctx, instance_id, signal) {
    set_boss_state(ctx, instance_id, 1, signal, "Cobrahn")
});

crate::encounter_package!(WailingCavernsPythas, fn pythas(ctx, instance_id, signal) {
    set_boss_state(ctx, instance_id, 2, signal, "Pythas")
});

crate::encounter_package!(WailingCavernsSerpentis, fn serpentis(ctx, instance_id, signal) {
    set_boss_state(ctx, instance_id, 3, signal, "Serpentis")
});

crate::encounter_package!(WailingCavernsMutanus, fn mutanus(ctx, instance_id, signal) {
    set_boss_state(ctx, instance_id, 5, signal, "Mutanus")?;
    if signal == EncounterSignal::Complete {
        begin_awakening(ctx, instance_id);
    }
    Ok(())
});

crate::game_hook!(on_gossip_select, fn disciple_start_selected(ctx, payload) {
    if payload.option_row_id != WAILING_START_OPTION_ROW {
        return;
    }
    let entities = ctx.db.game_world_entity();
    let (Some(player), Some(disciple)) = (
        entities.guid().find(payload.character_guid),
        entities.guid().find(payload.npc_guid),
    ) else {
        return;
    };
    if !player.is_player()
        || player.dead
        || disciple.dead
        || disciple.entry != DISCIPLE_OF_NARALEX
        || disciple.map_id != MAP_ID
        || player.map_id != MAP_ID
        || disciple.instance_id == 0
        || player.instance_id != disciple.instance_id
        || !instance_belongs_to_wailing(ctx, disciple.instance_id)
        || encounter::get_encounter_data(ctx, disciple.instance_id, DISCIPLE_ENCOUNTER_ID)
            != DISCIPLE_ESCORT_READY
        || !matches!(
            encounter::get_encounter_state(ctx, disciple.instance_id, DISCIPLE_ENCOUNTER_ID),
            ENCOUNTER_NOT_STARTED | ENCOUNTER_FAILED
        )
    {
        return;
    }
    start_escort(ctx, disciple.instance_id, disciple.guid);
});

crate::game_hook!(on_creature_death, fn wailing_ritual_add_died(ctx, payload) {
    if payload.instance_id == 0
        || !instance_belongs_to_wailing(ctx, payload.instance_id)
        || !matches!(
            payload.entry,
            DEVIATE_RAPTOR
                | DEVIATE_VIPER
                | DEVIATE_ADDER
                | DEVIATE_MOCCASIN
                | NIGHTMARE_ECTOPLASM
        )
        || encounter::get_encounter_state(ctx, payload.instance_id, DISCIPLE_ENCOUNTER_ID)
            != ENCOUNTER_IN_PROGRESS
    {
        return;
    }
    let Some(progress) = ctx
        .db
        .wailing_escort_progress()
        .instance_id()
        .find(payload.instance_id)
    else {
        return;
    };
    let (waiting_phase, entries, next_phase, delay_micros) = match payload.entry {
        DEVIATE_RAPTOR => (
            PHASE_WAIT_RAPTORS,
            &[DEVIATE_RAPTOR][..],
            PHASE_CONTINUE_AFTER_RAPTORS,
            2_000_000,
        ),
        DEVIATE_VIPER | DEVIATE_ADDER => (
            PHASE_WAIT_CIRCLE_WAVE,
            &[DEVIATE_VIPER, DEVIATE_ADDER][..],
            PHASE_CIRCLE_PURIFIED,
            12_000_000,
        ),
        DEVIATE_MOCCASIN => (
            PHASE_WAIT_MOCCASINS,
            &[DEVIATE_MOCCASIN][..],
            PHASE_SPAWN_ECTOPLASMS,
            2_000_000,
        ),
        NIGHTMARE_ECTOPLASM => (
            PHASE_WAIT_ECTOPLASMS,
            &[NIGHTMARE_ECTOPLASM][..],
            PHASE_SPAWN_MUTANUS,
            2_000_000,
        ),
        _ => return,
    };
    if progress.phase != waiting_phase
        || tracked_wave_lives(ctx, payload.instance_id, entries)
    {
        return;
    }
    continue_after(ctx, progress, next_phase, delay_micros);
});

crate::game_hook!(on_creature_spawn, fn disciple_respawned(ctx, payload) {
    if payload.entry != DISCIPLE_OF_NARALEX {
        return;
    }
    let Some(disciple) = ctx.db.game_world_entity().guid().find(payload.guid) else {
        return;
    };
    if disciple.map_id != MAP_ID
        || disciple.instance_id == 0
        || !instance_belongs_to_wailing(ctx, disciple.instance_id)
        || encounter::get_encounter_state(ctx, disciple.instance_id, DISCIPLE_ENCOUNTER_ID)
            == ENCOUNTER_DONE
    {
        return;
    }
    reset_escort(ctx, disciple.instance_id);
});

crate::game_hook!(on_aggro, fn disciple_entered_combat(ctx, payload) {
    let Some(disciple) = ctx.db.game_world_entity().guid().find(payload.creature_guid) else {
        return;
    };
    if disciple.entry != DISCIPLE_OF_NARALEX
        || disciple.map_id != MAP_ID
        || disciple.instance_id == 0
        || !instance_belongs_to_wailing(ctx, disciple.instance_id)
    {
        return;
    }
    let Some(progress) = ctx
        .db
        .wailing_escort_progress()
        .instance_id()
        .find(disciple.instance_id)
    else {
        return;
    };
    let attacker_is_mutanus = ctx
        .db
        .game_world_entity()
        .guid()
        .find(payload.target_guid)
        .is_some_and(|target| target.entry == MUTANUS);
    let message = if attacker_is_mutanus {
        "This creature is a minion from Naralex's nightmare, no doubt!"
    } else if progress.route_point >= 30 {
        "Deal with this creature! I need to prepare to awake Naralex!"
    } else if ctx.random::<u32>() % 100 < 90 {
        "Attacked! Help get this creature off of me!"
    } else {
        "Help!"
    };
    speak_guid(ctx, progress.disciple_guid, message);
});

crate::game_tick_pass!(fn wailing_escort_combat_pass(ctx) {
    let instance_ids: Vec<u64> = ctx
        .db
        .wailing_escort_progress()
        .iter()
        .map(|progress| progress.instance_id)
        .collect();
    for instance_id in instance_ids {
        tick_disciple_combat(ctx, instance_id);
    }
});

#[reducer]
pub fn advance_wailing_escort(ctx: &ReducerContext, scheduled: WailingEscortSchedule) {
    if ctx.sender() != ctx.database_identity()
        || !instance_belongs_to_wailing(ctx, scheduled.instance_id)
    {
        return;
    }
    let Some(progress) = ctx
        .db
        .wailing_escort_progress()
        .instance_id()
        .find(scheduled.instance_id)
    else {
        return;
    };
    if scheduled.phase == PHASE_DESPAWN {
        if let Err(error) = finish_escort(ctx, &progress) {
            spacetimedb::log::warn!("Wailing Caverns escort cleanup stopped: {error}");
        }
        return;
    }
    if progress.phase != scheduled.phase || progress.route_point != scheduled.route_point {
        return;
    }
    if let Err(error) = perform_escort_phase(ctx, progress) {
        spacetimedb::log::warn!("Wailing Caverns escort stopped: {error}");
    }
}

fn set_boss_state(
    ctx: &spacetimedb::ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    signal: EncounterSignal,
    name: &str,
) -> Result<(), String> {
    let state = match signal {
        EncounterSignal::Begin => ENCOUNTER_IN_PROGRESS,
        EncounterSignal::Fail => ENCOUNTER_FAILED,
        EncounterSignal::Complete => ENCOUNTER_DONE,
        other => return Err(format!("{name} does not accept encounter signal {other:?}")),
    };
    encounter::set_encounter_state(ctx, instance_id, encounter_id, state)?;
    refresh_disciple_gate(ctx, instance_id)
}

fn refresh_disciple_gate(
    ctx: &spacetimedb::ReducerContext,
    instance_id: u64,
) -> Result<(), String> {
    let all_leaders_done = (0..=3).all(|encounter_id| {
        encounter::get_encounter_state(ctx, instance_id, encounter_id) == ENCOUNTER_DONE
    });
    let disciple_state = encounter::get_encounter_state(ctx, instance_id, DISCIPLE_ENCOUNTER_ID);
    if all_leaders_done && matches!(disciple_state, ENCOUNTER_NOT_STARTED | ENCOUNTER_FAILED) {
        if disciple_state == ENCOUNTER_NOT_STARTED {
            speak_disciple_intro(ctx, instance_id);
        }
        encounter::set_encounter_data(
            ctx,
            instance_id,
            DISCIPLE_ENCOUNTER_ID,
            DISCIPLE_ESCORT_READY,
        )?;
        if let Some(disciple) = live_instance_creature(ctx, instance_id, DISCIPLE_OF_NARALEX) {
            install_start_menu(ctx, disciple.guid)?;
        }
    }
    Ok(())
}

fn install_start_menu(ctx: &ReducerContext, disciple_guid: u64) -> Result<(), String> {
    let profiles = ctx.db.game_gossip_menu_profile();
    match profiles.menu_id().find(WAILING_START_MENU) {
        Some(profile) if profile.text_id != WAILING_START_TEXT => {
            return Err(format!(
                "Wailing start menu {} collides with text {}",
                WAILING_START_MENU, profile.text_id
            ));
        }
        Some(_) => {}
        None => {
            profiles.insert(GossipMenuProfile {
                menu_id: WAILING_START_MENU,
                text_id: WAILING_START_TEXT,
            });
        }
    }
    let options = ctx.db.game_gossip_menu_profile_option();
    match options.row_id().find(WAILING_START_OPTION_ROW) {
        Some(option)
            if option.menu_id != WAILING_START_MENU || option.text != "Let the event begin!" =>
        {
            return Err(format!(
                "Wailing start option row {} collides with menu {}",
                WAILING_START_OPTION_ROW, option.menu_id
            ));
        }
        Some(_) => {}
        None => {
            options.insert(GossipMenuProfileOption {
                row_id: WAILING_START_OPTION_ROW,
                menu_id: WAILING_START_MENU,
                option_index: 0,
                icon: 0,
                text: "Let the event begin!".to_string(),
                action: lyracore_shared::constants::gossip_option::GOSSIP,
                action_menu_id: 0,
                cond_type: 0,
                cond_value1: 0,
                cond_value2: 0,
            });
        }
    }
    crate::creatures::set_creature_gossip_menu(ctx, disciple_guid, Some(WAILING_START_MENU))
}

fn speak_disciple_intro(ctx: &spacetimedb::ReducerContext, instance_id: u64) {
    speak_entry(
        ctx,
        instance_id,
        DISCIPLE_OF_NARALEX,
        "At last! Naralex can be awakened! Come aid me, brave adventurers!",
    );
}

fn start_escort(ctx: &ReducerContext, instance_id: u64, disciple_guid: u64) {
    let Some(naralex) = live_instance_creature(ctx, instance_id, NARALEX) else {
        return;
    };
    clear_escort_schedule(ctx, instance_id);
    if crate::creatures::presentation::apply_relay_faction(
        ctx,
        disciple_guid,
        ESCORT_FACTION,
        false,
    )
    .is_err()
    {
        return;
    }
    if crate::creatures::set_creature_gossip_menu(ctx, disciple_guid, None).is_err() {
        return;
    }
    if encounter::set_encounter_state(
        ctx,
        instance_id,
        DISCIPLE_ENCOUNTER_ID,
        ENCOUNTER_IN_PROGRESS,
    )
    .is_err()
    {
        return;
    }
    let progress = WailingEscortProgress {
        instance_id,
        disciple_guid,
        naralex_guid: naralex.guid,
        phase: PHASE_PREPARE,
        route_point: 1,
        running: false,
        next_potion_at_micros: ctx
            .timestamp
            .to_micros_since_unix_epoch()
            .saturating_add(5_000_000),
        next_sleep_at_micros: ctx
            .timestamp
            .to_micros_since_unix_epoch()
            .saturating_add(5_000_000),
    };
    let table = ctx.db.wailing_escort_progress();
    if table.instance_id().find(instance_id).is_some() {
        table.instance_id().update(progress);
    } else {
        table.insert(progress);
    }
    schedule_escort_phase(ctx, instance_id, PHASE_PREPARE, 1, 10_000_000);
}

fn begin_awakening(ctx: &ReducerContext, instance_id: u64) {
    let Some(progress) = ctx
        .db
        .wailing_escort_progress()
        .instance_id()
        .find(instance_id)
    else {
        return;
    };
    if encounter::get_encounter_state(ctx, instance_id, DISCIPLE_ENCOUNTER_ID)
        != ENCOUNTER_IN_PROGRESS
    {
        return;
    }
    if progress.phase != PHASE_WAIT_MUTANUS {
        return;
    }
    clear_escort_schedule(ctx, instance_id);
    continue_after(ctx, progress, PHASE_NARALEX_AWAKE, 100_000);
}

fn perform_escort_phase(
    ctx: &ReducerContext,
    progress: WailingEscortProgress,
) -> Result<(), String> {
    match progress.phase {
        PHASE_PREPARE => {
            send_emote_guid(ctx, progress.disciple_guid, EMOTE_TALK, 0);
            speak_guid(
                ctx,
                progress.disciple_guid,
                "I must make the necessary preparations before the awakening ritual can begin. You must protect me!",
            );
            continue_route_after(ctx, progress, 2, 3_000_000);
            Ok(())
        }
        PHASE_ROUTE_MOVE => begin_route_leg(ctx, progress),
        PHASE_ROUTE_ARRIVE => finish_route_leg(ctx, progress),
        PHASE_SPAWN_RAPTORS => {
            let guids = spawn_source_wave(ctx, &progress, &RAPTOR_WAVE);
            set_progress_phase(ctx, progress.clone(), PHASE_WAIT_RAPTORS);
            if guids.is_empty() {
                continue_after(ctx, progress, PHASE_CONTINUE_AFTER_RAPTORS, 2_000_000);
            }
            Ok(())
        }
        PHASE_CONTINUE_AFTER_RAPTORS => {
            send_emote_guid(ctx, progress.disciple_guid, EMOTE_POINT, 0);
            speak_guid(
                ctx,
                progress.disciple_guid,
                "Come. We must continue. There is much to be done before we can pull Naralex from his nightmare.",
            );
            continue_route_after(ctx, progress, 13, 0);
            Ok(())
        }
        PHASE_CLEANSE_CIRCLE => {
            speak_guid(
                ctx,
                progress.disciple_guid,
                "Within this circle of fire I must cast the spell to banish the spirits of the slain Fanglords.",
            );
            cast_for_choreography(
                ctx,
                progress.disciple_guid,
                CLEANSING,
                progress.disciple_guid,
                "cleansing",
            );
            continue_after(ctx, progress, PHASE_SPAWN_CIRCLE_WAVE, 20_000_000);
            Ok(())
        }
        PHASE_SPAWN_CIRCLE_WAVE => {
            let wave: Vec<_> = CIRCLE_WAVE_POSITIONS
                .into_iter()
                .map(|(x, y, z, orientation)| {
                    let entry = if ctx.random::<u32>() % 2 == 0 {
                        DEVIATE_VIPER
                    } else {
                        DEVIATE_ADDER
                    };
                    (entry, x, y, z, orientation)
                })
                .collect();
            let guids = spawn_source_wave(ctx, &progress, &wave);
            set_progress_phase(ctx, progress.clone(), PHASE_WAIT_CIRCLE_WAVE);
            if guids.is_empty() {
                continue_after(ctx, progress, PHASE_CIRCLE_PURIFIED, 12_000_000);
            }
            Ok(())
        }
        PHASE_CIRCLE_PURIFIED => {
            send_emote_guid(ctx, progress.disciple_guid, EMOTE_TALK, 0);
            speak_guid(
                ctx,
                progress.disciple_guid,
                "The caverns have been purified. To Naralex's chamber we go!",
            );
            continue_route_after(ctx, progress, 31, 0);
            Ok(())
        }
        PHASE_BEGIN_RITUAL => {
            speak_guid(
                ctx,
                progress.disciple_guid,
                "Protect me brave souls as I delve into the Emerald Dream to rescue Naralex and put an end to this corruption!",
            );
            send_emote_guid(ctx, progress.disciple_guid, EMOTE_BEG, 0);
            continue_after(ctx, progress, PHASE_CAST_AWAKENING, 5_000_000);
            Ok(())
        }
        PHASE_CAST_AWAKENING => {
            cast_for_choreography(
                ctx,
                progress.disciple_guid,
                AWAKENING,
                progress.disciple_guid,
                "awakening",
            );
            continue_after(ctx, progress, PHASE_SPAWN_MOCCASINS, 3_000_000);
            Ok(())
        }
        PHASE_SPAWN_MOCCASINS => {
            let guids = spawn_source_wave(ctx, &progress, &MOCCASIN_WAVE);
            continue_after(ctx, progress, PHASE_NARALEX_RESTLESS, 5_000_000);
            if guids.is_empty() {
                spacetimedb::log::warn!("Wailing Caverns moccasin wave spawned no creatures");
            }
            Ok(())
        }
        PHASE_NARALEX_RESTLESS => {
            if tracked_wave_lives(ctx, progress.instance_id, &[DEVIATE_MOCCASIN]) {
                set_progress_phase(ctx, progress, PHASE_WAIT_MOCCASINS);
            } else {
                continue_after(ctx, progress, PHASE_SPAWN_ECTOPLASMS, 0);
            }
            Ok(())
        }
        PHASE_SPAWN_ECTOPLASMS => {
            let guids = spawn_source_wave(ctx, &progress, &ECTOPLASM_WAVE);
            continue_after(ctx, progress, PHASE_ECTOPLASM_BREAKTHROUGH, 20_000_000);
            if guids.is_empty() {
                spacetimedb::log::warn!("Wailing Caverns ectoplasm wave spawned no creatures");
            }
            Ok(())
        }
        PHASE_ECTOPLASM_BREAKTHROUGH => {
            if tracked_wave_lives(ctx, progress.instance_id, &[NIGHTMARE_ECTOPLASM]) {
                set_progress_phase(ctx, progress, PHASE_WAIT_ECTOPLASMS);
            } else {
                continue_after(ctx, progress, PHASE_SPAWN_MUTANUS, 0);
            }
            Ok(())
        }
        PHASE_SPAWN_MUTANUS => {
            let guids = spawn_source_wave(ctx, &progress, &MUTANUS_WAVE);
            encounter::set_encounter_state(ctx, progress.instance_id, 5, ENCOUNTER_IN_PROGRESS)?;
            set_progress_phase(ctx, progress, PHASE_WAIT_MUTANUS);
            if guids.is_empty() {
                spacetimedb::log::warn!("Wailing Caverns Mutanus wave spawned no creature");
            }
            Ok(())
        }
        PHASE_NARALEX_AWAKE => {
            crate::creatures::presentation::apply_relay_stand_state(ctx, progress.naralex_guid, 0)?;
            speak_yell_guid(ctx, progress.naralex_guid, "I AM AWAKE, AT LAST!");
            encounter::set_encounter_state(
                ctx,
                progress.instance_id,
                DISCIPLE_ENCOUNTER_ID,
                ENCOUNTER_DONE,
            )?;
            continue_after(ctx, progress, PHASE_DISCIPLE_AWAKE, 5_000_000);
            Ok(())
        }
        PHASE_DISCIPLE_AWAKE => {
            crate::spell::interrupt_cast(ctx, progress.disciple_guid);
            crate::spell::strip_spell_auras(ctx, progress.disciple_guid, AWAKENING);
            send_emote_guid(ctx, progress.disciple_guid, EMOTE_POINT, 0);
            speak_guid(
                ctx,
                progress.disciple_guid,
                "At last! Naralex can be awakened! Come aid me, brave adventurers!",
            );
            continue_after(ctx, progress, PHASE_NARALEX_THANKS, 1_000_000);
            Ok(())
        }
        PHASE_NARALEX_THANKS => {
            send_emote_guid(ctx, progress.naralex_guid, EMOTE_BOW, 0);
            speak_guid(
                ctx,
                progress.naralex_guid,
                "Ah, to be pulled from the dreaded nightmare! I thank you, my loyal Disciple, along with your brave companions.",
            );
            continue_after(ctx, progress, PHASE_FAREWELL, 7_000_000);
            Ok(())
        }
        PHASE_FAREWELL => {
            send_emote_guid(ctx, progress.naralex_guid, EMOTE_TALK, 0);
            speak_guid(
                ctx,
                progress.naralex_guid,
                "We must go and gather with the other Disciples. There is much work to be done before I can make another attempt to restore the Barrens. Farewell, brave souls!",
            );
            continue_after(ctx, progress, PHASE_SHAPESHIFT, 3_000_000);
            Ok(())
        }
        PHASE_SHAPESHIFT => {
            cast_for_choreography(
                ctx,
                progress.naralex_guid,
                SHAPESHIFT,
                progress.naralex_guid,
                "Naralex shapeshift",
            );
            cast_for_choreography(
                ctx,
                progress.disciple_guid,
                SHAPESHIFT,
                progress.disciple_guid,
                "Disciple shapeshift",
            );
            continue_after(ctx, progress, PHASE_EXIT, 8_000_000);
            Ok(())
        }
        PHASE_EXIT => {
            let mut exit = progress;
            exit.running = true;
            exit.phase = PHASE_ROUTE_MOVE;
            exit.route_point = 71;
            ctx.db
                .wailing_escort_progress()
                .instance_id()
                .update(exit.clone());
            schedule_escort_phase(ctx, exit.instance_id, PHASE_ROUTE_MOVE, exit.route_point, 0);
            schedule_escort_phase(ctx, exit.instance_id, PHASE_DESPAWN, 0, 30_000_000);
            Ok(())
        }
        phase => Err(format!("unsupported Wailing Caverns escort phase {phase}")),
    }
}

fn finish_escort(ctx: &ReducerContext, progress: &WailingEscortProgress) -> Result<(), String> {
    clear_escort_schedule(ctx, progress.instance_id);
    encounter::encounter_reset(ctx, progress.instance_id, DISCIPLE_ENCOUNTER_ID);
    encounter::set_encounter_state(
        ctx,
        progress.instance_id,
        DISCIPLE_ENCOUNTER_ID,
        ENCOUNTER_DONE,
    )?;
    let creature_guids: Vec<u64> = ctx
        .db
        .game_world_entity()
        .by_map()
        .filter(&MAP_ID)
        .filter(|entity| entity.instance_id == progress.instance_id && !entity.is_player())
        .map(|entity| entity.guid)
        .collect();
    for guid in creature_guids {
        crate::creatures::despawn_creature_entity(ctx, guid);
    }
    ctx.db
        .wailing_escort_progress()
        .instance_id()
        .delete(progress.instance_id);
    Ok(())
}

fn begin_route_leg(ctx: &ReducerContext, progress: WailingEscortProgress) -> Result<(), String> {
    let point = usize::from(progress.route_point);
    let destination = WAILING_ROUTE
        .get(point.saturating_sub(1))
        .copied()
        .ok_or_else(|| format!("unsupported Wailing Caverns route point {point}"))?;
    let mover = ctx
        .db
        .game_world_entity()
        .guid()
        .find(progress.disciple_guid)
        .ok_or_else(|| format!("Disciple {} is missing", progress.disciple_guid))?;
    let dx = destination.0 - mover.x;
    let dy = destination.1 - mover.y;
    let base_speed = if progress.running {
        lyracore_shared::constants::speeds::RUN
    } else {
        lyracore_shared::constants::speeds::WALK
    };
    let speed = crate::combat::effective_move_speed(ctx, progress.disciple_guid, base_speed);
    if speed <= 0.0 {
        return Err("Disciple cannot move while immobilized".to_string());
    }
    let mut movement_micros = (((dx * dx + dy * dy).sqrt() / speed) * 1_000_000.0) as i64;
    encounter::move_to_point(
        ctx,
        progress.disciple_guid,
        destination.0,
        destination.1,
        destination.2,
        progress.running,
    )?;
    if progress.running && point >= 71 {
        if let Some(naralex) = ctx
            .db
            .game_world_entity()
            .guid()
            .find(progress.naralex_guid)
            .filter(|entity| !entity.dead)
        {
            let previous = WAILING_ROUTE[point.saturating_sub(2)];
            let follower = exit_follower_point(previous, destination, 5.0);
            let follow_dx = follower.0 - naralex.x;
            let follow_dy = follower.1 - naralex.y;
            let follow_speed = crate::combat::effective_move_speed(
                ctx,
                progress.naralex_guid,
                lyracore_shared::constants::speeds::RUN,
            );
            if follow_speed > 0.0 {
                movement_micros = movement_micros.max(
                    (((follow_dx * follow_dx + follow_dy * follow_dy).sqrt() / follow_speed)
                        * 1_000_000.0) as i64,
                );
                if let Err(error) = encounter::move_to_point(
                    ctx,
                    progress.naralex_guid,
                    follower.0,
                    follower.1,
                    follower.2,
                    true,
                ) {
                    spacetimedb::log::warn!("Naralex could not follow the Disciple: {error}");
                }
            }
        }
    }
    continue_after(ctx, progress, PHASE_ROUTE_ARRIVE, movement_micros);
    Ok(())
}

fn finish_route_leg(ctx: &ReducerContext, progress: WailingEscortProgress) -> Result<(), String> {
    match progress.route_point {
        12 => {
            speak_guid(
                ctx,
                progress.disciple_guid,
                "These caverns were once a temple of promise for regrowth in the Barrens. Now, they are the halls of nightmares.",
            );
            continue_after(ctx, progress, PHASE_SPAWN_RAPTORS, 2_000_000);
        }
        30 => continue_after(ctx, progress, PHASE_CLEANSE_CIRCLE, 1_000_000),
        57 => {
            send_emote_guid(ctx, progress.disciple_guid, EMOTE_POINT, 0);
            speak_guid(
                ctx,
                progress.disciple_guid,
                "Beyond this corridor, Naralex lies in fitful sleep. Let us go awaken him before it is too late.",
            );
            continue_route_after(ctx, progress, 58, 5_000_000);
        }
        70 => continue_after(ctx, progress, PHASE_BEGIN_RITUAL, 1_000_000),
        79 => {}
        point => {
            let wait_micros = if point == 13 { 3_000_000 } else { 0 };
            continue_route_after(ctx, progress, point.saturating_add(1), wait_micros);
        }
    }
    Ok(())
}

fn continue_route_after(
    ctx: &ReducerContext,
    mut progress: WailingEscortProgress,
    route_point: u8,
    delay_micros: i64,
) {
    progress.phase = PHASE_ROUTE_MOVE;
    progress.route_point = route_point;
    let instance_id = progress.instance_id;
    ctx.db
        .wailing_escort_progress()
        .instance_id()
        .update(progress);
    schedule_escort_phase(
        ctx,
        instance_id,
        PHASE_ROUTE_MOVE,
        route_point,
        delay_micros,
    );
}

fn continue_after(
    ctx: &ReducerContext,
    progress: WailingEscortProgress,
    next_phase: u8,
    delay_micros: i64,
) {
    let instance_id = progress.instance_id;
    let route_point = progress.route_point;
    set_progress_phase(ctx, progress, next_phase);
    schedule_escort_phase(ctx, instance_id, next_phase, route_point, delay_micros);
}

fn set_progress_phase(ctx: &ReducerContext, mut progress: WailingEscortProgress, phase: u8) {
    progress.phase = phase;
    ctx.db
        .wailing_escort_progress()
        .instance_id()
        .update(progress);
}

fn schedule_escort_phase(
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

fn clear_escort_schedule(ctx: &ReducerContext, instance_id: u64) {
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

fn spawn_source_wave(
    ctx: &ReducerContext,
    progress: &WailingEscortProgress,
    creatures: &[(u32, f32, f32, f32, f32)],
) -> Vec<u64> {
    let mut spawned = Vec::with_capacity(creatures.len());
    for &(entry, x, y, z, orientation) in creatures {
        let guids = encounter::spawn_wave(
            ctx,
            progress.instance_id,
            DISCIPLE_ENCOUNTER_ID,
            MAP_ID,
            &[entry],
            x + 2.0,
            y,
            z,
            orientation,
        );
        for guid in guids {
            if !crate::combat::arm_creature_engagement(ctx, guid, progress.disciple_guid, false) {
                spacetimedb::log::warn!(
                    "Wailing Caverns wave creature {guid} already had an engagement"
                );
            }
            spawned.push(guid);
        }
    }
    spawned
}

fn tracked_wave_lives(ctx: &ReducerContext, instance_id: u64, entries: &[u32]) -> bool {
    ctx.db
        .game_encounter_spawn()
        .by_instance()
        .filter(&instance_id)
        .filter(|spawn| entries.contains(&encounter::entry_of_unit_guid(spawn.guid)))
        .any(|spawn| {
            ctx.db
                .game_world_entity()
                .guid()
                .find(spawn.guid)
                .is_some_and(|entity| !entity.dead)
        })
}

fn reset_escort(ctx: &ReducerContext, instance_id: u64) {
    clear_escort_schedule(ctx, instance_id);
    ctx.db
        .wailing_escort_progress()
        .instance_id()
        .delete(instance_id);
    encounter::encounter_reset(ctx, instance_id, DISCIPLE_ENCOUNTER_ID);
    if encounter::set_encounter_state(ctx, instance_id, DISCIPLE_ENCOUNTER_ID, ENCOUNTER_FAILED)
        .is_ok()
    {
        let _ = refresh_disciple_gate(ctx, instance_id);
    }
}

fn cast_for_choreography(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
    label: &str,
) {
    if let Err(error) = crate::actor::cast_at(ctx, caster_guid, spell_id, target_guid) {
        spacetimedb::log::warn!("Wailing Caverns {label} cast refused: {error}");
    }
}

fn exit_follower_point(
    previous: (f32, f32, f32),
    destination: (f32, f32, f32),
    distance: f32,
) -> (f32, f32, f32) {
    let dx = destination.0 - previous.0;
    let dy = destination.1 - previous.1;
    let length = (dx * dx + dy * dy).sqrt();
    if length <= f32::EPSILON {
        return destination;
    }
    (
        destination.0 - dx / length * distance,
        destination.1 - dy / length * distance,
        destination.2,
    )
}

fn tick_disciple_combat(ctx: &ReducerContext, instance_id: u64) {
    if !instance_belongs_to_wailing(ctx, instance_id) {
        clear_escort_schedule(ctx, instance_id);
        ctx.db
            .wailing_escort_progress()
            .instance_id()
            .delete(instance_id);
        return;
    }
    if encounter::get_encounter_state(ctx, instance_id, DISCIPLE_ENCOUNTER_ID)
        != ENCOUNTER_IN_PROGRESS
    {
        return;
    }
    let table = ctx.db.wailing_escort_progress();
    let Some(mut progress) = table.instance_id().find(instance_id) else {
        return;
    };
    let Some(disciple) = ctx
        .db
        .game_world_entity()
        .guid()
        .find(progress.disciple_guid)
        .filter(|entity| !entity.dead)
    else {
        return;
    };
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let mut changed = false;
    if now >= progress.next_potion_at_micros {
        if disciple.health.saturating_mul(100) < disciple.max_health.saturating_mul(80) {
            progress.next_potion_at_micros =
                if crate::actor::cast_at(ctx, disciple.guid, POTION, disciple.guid).is_ok() {
                    now.saturating_add(45_000_000)
                } else {
                    now.saturating_add(500_000)
                };
        } else {
            progress.next_potion_at_micros = now.saturating_add(5_000_000);
        }
        changed = true;
    }
    if now >= progress.next_sleep_at_micros && crate::combat::is_engaged(ctx, disciple.guid) {
        let current = disciple.target_guid;
        let candidates: Vec<u64> = ctx
            .db
            .game_melee_attack()
            .by_target()
            .filter(&disciple.guid)
            .map(|attack| attack.attacker_guid)
            .filter(|guid| *guid != current)
            .filter(|guid| {
                ctx.db
                    .game_world_entity()
                    .guid()
                    .find(*guid)
                    .is_some_and(|entity| {
                        !entity.dead && entity.map_id == MAP_ID && entity.instance_id == instance_id
                    })
            })
            .collect();
        if !candidates.is_empty() {
            let target = candidates[ctx.random::<u32>() as usize % candidates.len()];
            progress.next_sleep_at_micros =
                if crate::actor::cast_at(ctx, disciple.guid, SLEEP, target).is_ok() {
                    now.saturating_add(30_000_000)
                } else {
                    now.saturating_add(500_000)
                };
            changed = true;
        }
    }
    if changed {
        table.instance_id().update(progress);
    }
}

fn live_instance_creature(
    ctx: &ReducerContext,
    instance_id: u64,
    entry: u32,
) -> Option<crate::WorldEntity> {
    ctx.db
        .game_world_entity()
        .by_map()
        .filter(&MAP_ID)
        .find(|entity| entity.instance_id == instance_id && entity.entry == entry && !entity.dead)
}

fn speak_entry(ctx: &ReducerContext, instance_id: u64, entry: u32, message: &str) {
    if let Some(speaker) = live_instance_creature(ctx, instance_id, entry) {
        let _ = crate::chat::apply_send_chat(
            ctx,
            speaker,
            crate::chat::CHAT_SAY,
            0,
            message.to_string(),
        );
    }
}

fn speak_guid(ctx: &ReducerContext, guid: u64, message: &str) {
    if let Some(speaker) = ctx.db.game_world_entity().guid().find(guid) {
        let _ = crate::chat::apply_send_chat(
            ctx,
            speaker,
            crate::chat::CHAT_SAY,
            0,
            message.to_string(),
        );
    }
}

fn speak_yell_guid(ctx: &ReducerContext, guid: u64, message: &str) {
    if let Some(speaker) = ctx.db.game_world_entity().guid().find(guid) {
        let _ = crate::chat::apply_send_chat(
            ctx,
            speaker,
            crate::chat::CHAT_YELL,
            0,
            message.to_string(),
        );
    }
}

fn send_emote_guid(ctx: &ReducerContext, guid: u64, emote_anim: u32, target_guid: u64) {
    if let Some(speaker) = ctx.db.game_world_entity().guid().find(guid) {
        let _ = crate::chat::apply_send_emote(ctx, speaker, 0, emote_anim, target_guid);
    }
}

fn instance_belongs_to_wailing(ctx: &ReducerContext, instance_id: u64) -> bool {
    ctx.db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .is_some_and(|instance| instance.map_id == MAP_ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naralex_route_matches_the_pinned_source_account() {
        assert_eq!(WAILING_ROUTE.len(), 79);
        assert_eq!(WAILING_ROUTE[11], (-104.28827, 234.40804, -91.64163));
        assert_eq!(WAILING_ROUTE[29], (-54.713943, 273.85025, -92.84426));
        assert_eq!(WAILING_ROUTE[56], (54.5601, 210.85043, -89.80927));
        assert_eq!(WAILING_ROUTE[69], (114.51453, 235.30222, -96.1607));
        assert_eq!(WAILING_ROUTE[78], (-41.114, 204.149, -78.94));
    }

    #[test]
    fn ritual_waves_match_the_pinned_source_account() {
        assert_eq!(RAPTOR_WAVE.len(), 2);
        assert_eq!(CIRCLE_WAVE_POSITIONS.len(), 3);
        assert_eq!(MOCCASIN_WAVE.len(), 3);
        assert_eq!(ECTOPLASM_WAVE.len(), 7);
        assert_eq!(MUTANUS_WAVE.len(), 1);
        assert_eq!(RAPTOR_WAVE[0].1, -67.44779);
        assert_eq!(MOCCASIN_WAVE[2].3, -105.54061);
        assert_eq!(ECTOPLASM_WAVE[6].2, 274.12335);
        assert_eq!(MUTANUS_WAVE[0].1, 150.94276);
    }

    #[test]
    fn naralex_exit_target_stays_five_yards_behind_the_disciple() {
        let previous = WAILING_ROUTE[69];
        let destination = WAILING_ROUTE[70];
        let follower = exit_follower_point(previous, destination, 5.0);
        let dx = destination.0 - follower.0;
        let dy = destination.1 - follower.1;
        assert!(((dx * dx + dy * dy).sqrt() - 5.0).abs() < 0.001);
        assert_eq!(follower.2, destination.2);
    }
}
