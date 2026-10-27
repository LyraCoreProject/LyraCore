use spacetimedb::{table, ReducerContext, Table};

use lyracore_package_delta::packed_creature_spawn_guid;
use lyracore_shared::constants;

use crate::game_item_template;

use crate::{
    game_creature_spawn, game_creature_template, game_gameobject, game_gameobject_template,
    game_instance, game_world_entity,
};

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncounterBinding {
    BlackfathomDeepsKelris,
    BlackrockDepthsTombOfSeven,
    DireMaulAlzzin,
    RazorfenKraulWardKeepers,
    ShadowfangKeepRethilgore,
    ShadowfangKeepFenrus,
    ShadowfangKeepNandos,
    SunkenTempleAvatar,
    WailingCavernsAnacondra,
    WailingCavernsCobrahn,
    WailingCavernsPythas,
    WailingCavernsSerpentis,
    WailingCavernsMutanus,
    ZulGurubOhgan,
}

pub(crate) type EncounterPackageHandler =
    fn(&ReducerContext, u64, EncounterSignal) -> Result<(), String>;

impl EncounterBinding {
    pub const ALL: [Self; 14] = [
        Self::BlackfathomDeepsKelris,
        Self::BlackrockDepthsTombOfSeven,
        Self::DireMaulAlzzin,
        Self::RazorfenKraulWardKeepers,
        Self::ShadowfangKeepRethilgore,
        Self::ShadowfangKeepFenrus,
        Self::ShadowfangKeepNandos,
        Self::SunkenTempleAvatar,
        Self::WailingCavernsAnacondra,
        Self::WailingCavernsCobrahn,
        Self::WailingCavernsPythas,
        Self::WailingCavernsSerpentis,
        Self::WailingCavernsMutanus,
        Self::ZulGurubOhgan,
    ];

    pub(crate) fn map_id(self) -> u32 {
        match self {
            Self::BlackfathomDeepsKelris => 48,
            Self::BlackrockDepthsTombOfSeven => 230,
            Self::DireMaulAlzzin => 429,
            Self::RazorfenKraulWardKeepers => 47,
            Self::ShadowfangKeepRethilgore
            | Self::ShadowfangKeepFenrus
            | Self::ShadowfangKeepNandos => 33,
            Self::SunkenTempleAvatar => 109,
            Self::WailingCavernsAnacondra
            | Self::WailingCavernsCobrahn
            | Self::WailingCavernsPythas
            | Self::WailingCavernsSerpentis
            | Self::WailingCavernsMutanus => 43,
            Self::ZulGurubOhgan => 309,
        }
    }
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncounterSignal {
    Begin,
    Fail,
    Complete,
    BreakAlzzinCrumbleWall,
    InterruptAvatarSuppression,
    SendMandokirDownstairs,
}

pub(crate) fn notify_from_eventai(
    ctx: &ReducerContext,
    source_guid: u64,
    binding: EncounterBinding,
    signal: EncounterSignal,
) -> Result<(), String> {
    let source = ctx
        .db
        .game_world_entity()
        .guid()
        .find(source_guid)
        .ok_or_else(|| format!("encounter notification source {source_guid} is missing"))?;
    if source.map_id != binding.map_id() {
        return Err(format!(
            "encounter binding {binding:?} belongs to map {}, not source map {}",
            binding.map_id(),
            source.map_id
        ));
    }
    if source.instance_id == 0 {
        return Err(format!(
            "encounter binding {binding:?} needs an instance-scoped source"
        ));
    }
    let instance = ctx
        .db
        .game_instance()
        .instance_id()
        .find(source.instance_id)
        .ok_or_else(|| {
            format!(
                "encounter source instance {} is missing",
                source.instance_id
            )
        })?;
    if instance.map_id != source.map_id {
        return Err(format!(
            "encounter source map {} does not match instance {} map {}",
            source.map_id, source.instance_id, instance.map_id
        ));
    }
    let Some(handler) = package_handler_for(binding, crate::GAME_ENCOUNTER_PACKAGES)? else {
        spacetimedb::log::warn!(
            "encounter binding {binding:?} has no installed package authority — {signal:?} not consumed"
        );
        return Ok(());
    };
    handler(ctx, source.instance_id, signal)
}

fn package_handler_for(
    binding: EncounterBinding,
    installed: &[(EncounterBinding, EncounterPackageHandler)],
) -> Result<Option<EncounterPackageHandler>, String> {
    let mut handlers = installed
        .iter()
        .filter(|(installed, _)| *installed == binding)
        .map(|(_, handler)| *handler);
    let Some(handler) = handlers.next() else {
        return Ok(None);
    };
    if handlers.next().is_some() {
        return Err(format!(
            "encounter binding {binding:?} has more than one installed package authority"
        ));
    }
    Ok(Some(handler))
}

#[cfg(test)]
fn encounter_state_for_signal(signal: EncounterSignal) -> Option<u8> {
    match signal {
        EncounterSignal::Begin => Some(ENCOUNTER_IN_PROGRESS),
        EncounterSignal::Fail => Some(ENCOUNTER_FAILED),
        EncounterSignal::Complete => Some(ENCOUNTER_DONE),
        EncounterSignal::BreakAlzzinCrumbleWall
        | EncounterSignal::InterruptAvatarSuppression
        | EncounterSignal::SendMandokirDownstairs => None,
    }
}

pub const ENCOUNTER_NOT_STARTED: u8 = 0;
#[allow(dead_code, reason = "Packages select the encounter states they need")]
pub const ENCOUNTER_IN_PROGRESS: u8 = 1;
#[allow(dead_code, reason = "Packages select the encounter states they need")]
pub const ENCOUNTER_DONE: u8 = 2;
#[allow(dead_code, reason = "Packages select the encounter states they need")]
pub const ENCOUNTER_FAILED: u8 = 3;

// Encounter ids with this bit set belong to kernel bookkeeping.
pub const RESERVED_ENCOUNTER_ID_BIT: u32 = 0x8000_0000;

pub const HP_FIRED_NONE: u32 = 101;

#[table(accessor = game_encounter_state, index(accessor = by_pair, btree(columns = [instance_id, encounter_id])))]
pub struct EncounterState {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub instance_id: u64,
    pub encounter_id: u32,
    pub state: u8,
    pub payload: u32,
}

#[table(accessor = game_encounter_hp_watch, index(accessor = by_entry, btree(columns = [entry])))]
pub struct EncounterHpWatch {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub entry: u32,
    pub pct: u8,
}

#[table(accessor = game_encounter_spawn, index(accessor = by_instance, btree(columns = [instance_id])))]
pub struct EncounterSpawn {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub instance_id: u64,
    pub encounter_id: u32,
    pub guid: u64,
}

#[table(accessor = game_encounter_equip, public, index(accessor = by_instance, btree(columns = [instance_id])))]
pub struct EncounterEquip {
    #[primary_key]
    pub creature_guid: u64,
    pub instance_id: u64,
    pub main_hand: u32,
    pub off_hand: u32,
    pub ranged: u32,
}

impl EncounterEquip {
    fn replace(self, ctx: &ReducerContext) {
        let table = ctx.db.game_encounter_equip();
        let row = if table.creature_guid().find(self.creature_guid).is_some() {
            table.creature_guid().update(self)
        } else {
            table.insert(self)
        };
        crate::native::creature_equipment_changed(ctx, &row, false);
    }

    fn delete(ctx: &ReducerContext, guid: u64) {
        let table = ctx.db.game_encounter_equip();
        if let Some(row) = table.creature_guid().find(guid) {
            table.creature_guid().delete(guid);
            crate::native::creature_equipment_changed(ctx, &row, true);
        }
    }
}

pub(crate) fn hp_fired_key(entry: u32) -> u32 {
    RESERVED_ENCOUNTER_ID_BIT | entry
}

pub(crate) fn is_reserved_encounter_id(encounter_id: u32) -> bool {
    encounter_id & RESERVED_ENCOUNTER_ID_BIT != 0
}

pub(crate) fn hp_pct(health: u32, max_health: u32) -> u32 {
    if max_health == 0 {
        return 100;
    }
    ((health as u64 * 100) / max_health as u64) as u32
}

pub(crate) fn crossed_thresholds(watches: &[u8], lowest_fired: u32, new_pct: u32) -> Vec<u8> {
    let mut fired: Vec<u8> = watches
        .iter()
        .copied()
        .filter(|&w| new_pct <= w as u32 && (w as u32) < lowest_fired)
        .collect();
    fired.sort_unstable_by(|a, b| b.cmp(a));
    fired.dedup();
    fired
}

pub(crate) fn wave_guid(entry: u32, next_low: u64) -> u64 {
    packed_creature_spawn_guid(entry, next_low as u32)
}

#[allow(dead_code, reason = "retained Package API encounter choreography")]
pub(crate) fn wave_offset(index: usize) -> (f32, f32) {
    (2.0 * ((index % 3) as f32 - 1.0), 2.0 * ((index / 3) as f32))
}

#[allow(dead_code, reason = "retained Package API encounter state")]
pub const DOOR_OPEN_STATE: u8 = 1;

fn state_row(ctx: &ReducerContext, instance_id: u64, encounter_id: u32) -> Option<EncounterState> {
    ctx.db
        .game_encounter_state()
        .by_pair()
        .filter((instance_id, encounter_id))
        .next()
}

fn upsert_state_row(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    state: u8,
    payload: u32,
) {
    let t = ctx.db.game_encounter_state();
    match state_row(ctx, instance_id, encounter_id) {
        Some(mut row) => {
            row.state = state;
            row.payload = payload;
            t.id().update(row);
        }
        None => {
            t.insert(EncounterState {
                id: 0,
                instance_id,
                encounter_id,
                state,
                payload,
            });
        }
    }
}

#[allow(dead_code, reason = "retained Package API operation")]
pub(crate) fn get_encounter_state(ctx: &ReducerContext, instance_id: u64, encounter_id: u32) -> u8 {
    state_row(ctx, instance_id, encounter_id)
        .map(|r| r.state)
        .unwrap_or(ENCOUNTER_NOT_STARTED)
}

#[allow(dead_code, reason = "retained Package API encounter choreography")]
pub(crate) fn set_encounter_state(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    state: u8,
) -> Result<(), String> {
    if is_reserved_encounter_id(encounter_id) {
        return Err(format!(
            "encounter_id {encounter_id:#x} is in the kernel-reserved bit-31 namespace (hp-fired ratchets)"
        ));
    }
    let payload = get_encounter_data(ctx, instance_id, encounter_id);
    upsert_state_row(ctx, instance_id, encounter_id, state, payload);
    Ok(())
}

#[allow(dead_code, reason = "retained Package API encounter choreography")]
pub(crate) fn get_encounter_data(ctx: &ReducerContext, instance_id: u64, encounter_id: u32) -> u32 {
    state_row(ctx, instance_id, encounter_id)
        .map(|r| r.payload)
        .unwrap_or(0)
}

#[allow(dead_code, reason = "retained Package API operation")]
pub(crate) fn set_encounter_data(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    data: u32,
) -> Result<(), String> {
    if is_reserved_encounter_id(encounter_id) {
        return Err(format!(
            "encounter_id {encounter_id:#x} is in the kernel-reserved bit-31 namespace (hp-fired ratchets)"
        ));
    }
    let state = get_encounter_state(ctx, instance_id, encounter_id);
    upsert_state_row(ctx, instance_id, encounter_id, state, data);
    Ok(())
}

#[allow(dead_code, reason = "retained Package API encounter choreography")]
pub(crate) fn watch_hp_threshold(ctx: &ReducerContext, entry: u32, pct: u8) -> Result<(), String> {
    if !(1..=99).contains(&pct) {
        return Err(format!("hp watch pct must be 1..=99, got {pct}"));
    }
    if entry & RESERVED_ENCOUNTER_ID_BIT != 0 {
        return Err(format!(
            "hp watch entry {entry:#x} has bit 31 set (reserved-key fold)"
        ));
    }
    let t = ctx.db.game_encounter_hp_watch();
    if t.by_entry().filter(&entry).any(|w| w.pct == pct) {
        return Ok(());
    }
    t.insert(EncounterHpWatch { id: 0, entry, pct });
    Ok(())
}

#[allow(dead_code, reason = "retained Package API operation")]
pub(crate) fn reset_hp_fired(ctx: &ReducerContext, instance_id: u64, entry: u32) {
    let t = ctx.db.game_encounter_state();
    if let Some(row) = state_row(ctx, instance_id, hp_fired_key(entry)) {
        t.id().delete(row.id);
    }
}

crate::game_hook!(on_damage_taken, fn encounter_hp_probe(ctx, payload) {
    if crate::GAME_HOOKS_ON_HP_THRESHOLD.is_empty() {
        return;
    }
    probe_hp_thresholds(ctx, payload.target_guid);
});

// Save fired thresholds before callbacks so reentrant damage cannot fire them twice.
fn probe_hp_thresholds(ctx: &ReducerContext, target_guid: u64) {
    let Some(e) = ctx.db.game_world_entity().guid().find(target_guid) else {
        return;
    };
    if e.is_player() || e.owner_guid != 0 || e.dead || e.max_health == 0 {
        return;
    }
    let watches: Vec<u8> = ctx
        .db
        .game_encounter_hp_watch()
        .by_entry()
        .filter(&e.entry)
        .map(|w| w.pct)
        .collect();
    if watches.is_empty() {
        return;
    }
    let key = hp_fired_key(e.entry);
    let lowest_fired = state_row(ctx, e.instance_id, key)
        .map(|r| r.payload)
        .unwrap_or(HP_FIRED_NONE);
    let fired = crossed_thresholds(&watches, lowest_fired, hp_pct(e.health, e.max_health));
    let Some(&new_lowest) = fired.last() else {
        return;
    };
    upsert_state_row(
        ctx,
        e.instance_id,
        key,
        ENCOUNTER_NOT_STARTED,
        new_lowest as u32,
    );
    for pct in fired {
        crate::hooks::fire_on_hp_threshold(
            ctx,
            &crate::hooks::HpThresholdPayload {
                creature_guid: target_guid,
                entry: e.entry,
                instance_id: e.instance_id,
                pct,
            },
        );
    }
}

#[allow(dead_code, reason = "retained Package API encounter choreography")]
pub(crate) fn open_door(
    ctx: &ReducerContext,
    go_entry: u32,
    instance_id: u64,
) -> Result<u32, String> {
    let tmpl = ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(go_entry)
        .ok_or_else(|| format!("no gameobject template {go_entry}"))?;
    if tmpl.type_id != crate::gameobject::go_type::DOOR
        && tmpl.type_id != crate::gameobject::go_type::BUTTON
    {
        return Err(format!(
            "gameobject {go_entry} is type {} — open_door flips DOOR/BUTTON only",
            tmpl.type_id
        ));
    }
    let gos = ctx.db.game_gameobject();

    let mut any_of_entry = false;
    let mut targets: Vec<u64> = Vec::new();
    for g in gos.iter() {
        if g.template_entry == go_entry && g.instance_id == instance_id {
            any_of_entry = true;
            if g.state != DOOR_OPEN_STATE {
                targets.push(g.guid);
            }
        }
    }
    if !any_of_entry {
        return Err(format!(
            "no spawned gameobject with entry {go_entry} in instance {instance_id}"
        ));
    }
    let opened = targets.len() as u32;
    for guid in targets {
        if let Some(mut g) = gos.guid().find(guid) {
            g.state = DOOR_OPEN_STATE;
            g.update(ctx);
        }
    }
    Ok(opened)
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code, reason = "retained Package API encounter choreography")]
pub(crate) fn spawn_wave(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    map_id: u32,
    entries: &[u32],
    x: f32,
    y: f32,
    z: f32,
    orientation: f32,
) -> Vec<u64> {
    let mut guids = Vec::with_capacity(entries.len());
    for (i, &entry) in entries.iter().enumerate() {
        let Some(tmpl) = ctx.db.game_creature_template().entry().find(entry) else {
            spacetimedb::log::warn!("spawn_wave: no creature template {entry} — skipped");
            continue;
        };
        let next_low = ctx
            .db
            .game_creature_spawn()
            .iter()
            .filter(|s| s.entry == entry)
            .map(|s| s.guid & 0x00FF_FFFF)
            .filter(|low| low & crate::instance::INSTANCE_POP_LOW_BAND == 0)
            .max()
            .unwrap_or(0)
            + 1;
        let guid = wave_guid(entry, next_low);
        let (dx, dy) = wave_offset(i);
        let spawn = crate::creatures::CreatureSpawn {
            guid,
            entry,
            map_id,
            x: x + dx,
            y: y + dy,
            z,
            orientation,

            respawn_at: ctx
                .timestamp
                .checked_add(spacetimedb::TimeDuration::from_micros(
                    u32::MAX as i64 * 1_000_000,
                ))
                .unwrap_or(ctx.timestamp),
            despawn_at: crate::creatures::timer_never(ctx),
            movement_type: crate::creatures::MOVEMENT_IDLE,
            respawn_secs: u32::MAX,
            life_seq: 0,
            source_guid: 0,
            instance_id,
        };
        let entity =
            crate::creatures::build_creature_entity(ctx, &spawn, &tmpl, ctx.random(), instance_id);
        ctx.db.game_creature_spawn().insert(spawn);
        crate::creatures::insert_creature_entity(ctx, entity);
        ctx.db.game_encounter_spawn().insert(EncounterSpawn {
            id: 0,
            instance_id,
            encounter_id,
            guid,
        });
        guids.push(guid);
    }
    guids
}

pub(crate) fn equip_swap(
    ctx: &ReducerContext,
    creature_guid: u64,
    main_hand: u32,
    off_hand: u32,
    ranged: u32,
) -> Result<(), String> {
    let e = crate::helpers::live_entity(ctx, creature_guid)
        .map_err(|_| format!("no live entity {creature_guid}"))?;
    if e.is_player() {
        return Err("equip_swap targets creatures only".to_string());
    }
    let display = |entry: u32| -> Result<u32, String> {
        if entry == 0 {
            return Ok(0);
        }
        ctx.db
            .game_item_template()
            .entry()
            .find(entry)
            .map(|item| item.display_id)
            .ok_or_else(|| format!("virtual equipment item {entry} is missing"))
    };
    let (main_hand, off_hand, ranged) = (display(main_hand)?, display(off_hand)?, display(ranged)?);
    EncounterEquip {
        creature_guid,
        instance_id: e.instance_id,
        main_hand,
        off_hand,
        ranged,
    }
    .replace(ctx);
    Ok(())
}

pub(crate) fn move_to_point(
    ctx: &ReducerContext,
    creature_guid: u64,
    x: f32,
    y: f32,
    z: f32,
    run: bool,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let Some(mut e) = entities.guid().find(creature_guid) else {
        return Err(format!("no live entity {creature_guid}"));
    };
    if e.is_player() {
        return Err("move_to_point targets creatures only".to_string());
    }
    if e.dead {
        return Err("mover is dead".to_string());
    }
    let base = if run {
        constants::speeds::RUN
    } else {
        constants::speeds::WALK
    };
    let speed = crate::combat::effective_move_speed(ctx, creature_guid, base);
    if speed <= 0.0 {
        return Ok(());
    }
    let spline_id = crate::creatures::tick::begin_leg(ctx, &mut e);
    let (dx, dy) = (x - e.x, y - e.y);
    let dist = (dx * dx + dy * dy).sqrt();
    if dist <= f32::EPSILON {
        crate::creatures::tick::stop_where_rendered(ctx, &mut e);
        e.update(ctx);
        return Ok(());
    }
    crate::creatures::tick::emit_move_spline(
        ctx,
        creature_guid,
        (e.x, e.y, e.z),
        (x, y, z),
        ((dist / speed) * 1000.0) as u32,
        run,
        spline_id,
        e.map_id,
        e.instance_id,
        (e.grid_x, e.grid_y),
        None,
    );
    e.last_move_ms = crate::creatures::tick::now_ms(ctx);
    e.update(ctx);
    Ok(())
}

#[allow(dead_code, reason = "retained Package API operation")]
pub(crate) fn encounter_reset(ctx: &ReducerContext, instance_id: u64, encounter_id: u32) {
    upsert_state_row(ctx, instance_id, encounter_id, ENCOUNTER_NOT_STARTED, 0);
    let tracked: Vec<EncounterSpawn> = ctx
        .db
        .game_encounter_spawn()
        .by_instance()
        .filter(&instance_id)
        .filter(|s| s.encounter_id == encounter_id)
        .collect();
    despawn_tracked(ctx, &tracked);
}

#[allow(
    dead_code,
    reason = "Packages select the encounter operations they need"
)]
pub(crate) fn encounter_reset_full(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    boss_entries: &[u32],
) {
    encounter_reset(ctx, instance_id, encounter_id);
    for &entry in boss_entries {
        reset_hp_fired(ctx, instance_id, entry);
    }
}

fn despawn_tracked(ctx: &ReducerContext, tracked: &[EncounterSpawn]) {
    for t in tracked {
        crate::creatures::despawn_creature_entity(ctx, t.guid);
        ctx.db.game_creature_spawn().guid().delete(t.guid);
        EncounterEquip::delete(ctx, t.guid);
        ctx.db.game_encounter_spawn().id().delete(t.id);
    }
}

// Delete tracked spawn rows too so Instance teardown cannot respawn them in the open world.
pub(crate) fn sweep_encounter_state(ctx: &ReducerContext, instance_id: u64) {
    let tracked: Vec<EncounterSpawn> = ctx
        .db
        .game_encounter_spawn()
        .by_instance()
        .filter(&instance_id)
        .collect();
    despawn_tracked(ctx, &tracked);
    let states = ctx.db.game_encounter_state();

    let stale: Vec<u64> = states
        .by_pair()
        .filter(&instance_id)
        .map(|r| r.id)
        .collect();
    for id in stale {
        states.id().delete(id);
    }
    let equips = ctx.db.game_encounter_equip();
    let stale: Vec<u64> = equips
        .by_instance()
        .filter(&instance_id)
        .map(|r| r.creature_guid)
        .collect();
    for guid in stale {
        EncounterEquip::delete(ctx, guid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_lookup_skips_unclaimed_and_refuses_duplicate_authority() {
        fn handler(
            _ctx: &ReducerContext,
            _instance_id: u64,
            _signal: EncounterSignal,
        ) -> Result<(), String> {
            Ok(())
        }

        let binding = EncounterBinding::BlackfathomDeepsKelris;
        assert!(matches!(package_handler_for(binding, &[]), Ok(None)));

        let claimed = [(binding, handler as EncounterPackageHandler)];
        assert!(matches!(
            package_handler_for(binding, &claimed),
            Ok(Some(_))
        ));

        let duplicate = [(binding, handler as EncounterPackageHandler); 2];
        let duplicate =
            package_handler_for(binding, &duplicate).expect_err("duplicate authority must refuse");
        assert!(duplicate.contains("more than one installed package authority"));
    }

    #[test]
    fn crossing_fires_each_watch_once_high_to_low_and_never_refires_after_heal_redrop() {
        let watches = [66u8, 33u8];
        assert_eq!(crossed_thresholds(&watches, HP_FIRED_NONE, 60), vec![66]);
        assert_eq!(crossed_thresholds(&watches, 66, 60), Vec::<u8>::new());
        assert_eq!(crossed_thresholds(&watches, 66, 30), vec![33]);
        assert_eq!(crossed_thresholds(&watches, 33, 5), Vec::<u8>::new());
    }

    #[test]
    fn one_giant_hit_crossing_several_watches_fires_all_desc_and_dedups_duplicates() {
        let watches = [33u8, 66u8, 66u8, 90u8];
        assert_eq!(
            crossed_thresholds(&watches, HP_FIRED_NONE, 10),
            vec![90, 66, 33]
        );
    }

    #[test]
    fn crossing_boundary_is_at_or_below_and_higher_watch_never_fires_after_lower_ratchet() {
        let watches = [66u8];
        assert_eq!(crossed_thresholds(&watches, HP_FIRED_NONE, 66), vec![66]);
        assert_eq!(
            crossed_thresholds(&watches, HP_FIRED_NONE, 67),
            Vec::<u8>::new()
        );
        assert_eq!(crossed_thresholds(&[66u8, 33u8], 33, 20), Vec::<u8>::new());
    }

    #[test]
    fn hp_pct_floors_uses_u64_math_and_treats_zero_max_as_full() {
        assert_eq!(hp_pct(664, 1000), 66);
        assert_eq!(hp_pct(0, 1000), 0);
        assert_eq!(hp_pct(1000, 1000), 100);
        assert_eq!(hp_pct(u32::MAX, u32::MAX), 100);
        assert_eq!(hp_pct(5, 0), 100);
    }

    #[test]
    fn reserved_hp_key_namespace_is_disjoint_from_package_encounter_ids() {
        assert!(is_reserved_encounter_id(hp_fired_key(1)));
        assert!(is_reserved_encounter_id(hp_fired_key(646)));
        assert_eq!(hp_fired_key(646), 0x8000_0000 | 646);
        assert!(!is_reserved_encounter_id(0));
        assert!(!is_reserved_encounter_id(1));
        assert!(!is_reserved_encounter_id(0x7FFF_FFFF));
        assert_ne!(hp_fired_key(639), hp_fired_key(646));
    }

    #[test]
    fn state_constants_are_the_mangos_four_and_hp_fired_none_sits_above_every_pct() {
        assert_eq!(ENCOUNTER_NOT_STARTED, 0);
        assert_eq!(ENCOUNTER_IN_PROGRESS, 1);
        assert_eq!(ENCOUNTER_DONE, 2);
        assert_eq!(ENCOUNTER_FAILED, 3);
        const { assert!(HP_FIRED_NONE > 100) };
    }

    #[test]
    fn named_source_states_translate_to_the_internal_encounter_contract() {
        assert_eq!(encounter_state_for_signal(EncounterSignal::Fail), Some(3));
        assert_eq!(
            encounter_state_for_signal(EncounterSignal::Complete),
            Some(2)
        );
    }

    #[test]
    fn wave_guid_carries_highguid_unit_entry_bits_and_a_bounded_low() {
        let g = wave_guid(636, 7);
        assert_eq!(
            g >> 48,
            0xF130,
            "HIGHGUID_UNIT so the 5875 client classifies it as a Unit"
        );
        assert_eq!((g >> 24) & 0xFF_FFFF, 636);
        assert_eq!(g & 0xFF_FFFF, 7);
        assert_eq!((wave_guid(636, 0x0100_0000 + 5) >> 24) & 0xFF_FFFF, 636);
        assert_eq!(wave_guid(636, 0x0100_0000 + 5) & 0xFF_FFFF, 5);
    }

    #[test]
    fn wave_offsets_spread_adds_on_a_three_wide_two_yard_grid() {
        assert_eq!(wave_offset(0), (-2.0, 0.0));
        assert_eq!(wave_offset(1), (0.0, 0.0));
        assert_eq!(wave_offset(2), (2.0, 0.0));
        assert_eq!(wave_offset(3), (-2.0, 2.0));
        let pts: Vec<(i32, i32)> = (0..9)
            .map(|i| {
                let (x, y) = wave_offset(i);
                (x as i32, y as i32)
            })
            .collect();
        let mut dedup = pts.clone();
        dedup.sort_unstable();
        dedup.dedup();
        assert_eq!(dedup.len(), pts.len());
    }
}
