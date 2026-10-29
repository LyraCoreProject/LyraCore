use std::collections::HashSet;

use spacetimedb::{reducer, table, ReducerContext, Table, TimeDuration, Timestamp};

use crate::creatures::game_creature_spawn_map;
use crate::game_gameobject_loot;
use crate::game_player_skill;
use crate::game_world_entity;
use crate::nav::game_nav_chunk;
use crate::spell::{CastGateRefusal, CastRefusalReason};
use crate::terrain::game_terrain_chunk;

pub mod go_type {

    pub const DOOR: u8 = 0;
    pub const BUTTON: u8 = 1;
    #[cfg(any(test, feature = "debug_reducers"))]
    #[cfg_attr(not(has_packages), allow(dead_code))]
    pub const QUESTGIVER: u8 = lyracore_shared::constants::go_type::QUESTGIVER;
    pub const CHEST: u8 = lyracore_shared::constants::go_type::CHEST;
    pub const TRAP: u8 = 6;
    pub const GOOBER: u8 = 10;
    pub const GATHER: u8 = 25;
    // Source type 25 is FISHINGHOLE; import drops it before using GATHER.
}

pub(crate) fn can_gather(learned: u32, required: u32) -> Result<(), String> {
    if learned == 0 {
        return Err("you have not learned this gathering skill".to_string());
    }
    if learned < required {
        return Err("your skill is too low to gather this node".to_string());
    }
    Ok(())
}

#[table(accessor = game_gameobject_template, public)]
pub struct GameObjectTemplate {
    #[primary_key]
    pub entry: u32,
    pub type_id: u8,
    pub display_id: u32,
    pub name: String,
    pub data0: u32,
    pub data1: u32,
    #[default(0u32)]
    pub gather_skill_line: u32,
    #[default(0u32)]
    pub respawn_secs: u32,
    #[default(0u32)]
    pub gather_gray: u32,
    #[default(0u32)]
    pub lock_id: u32,
    #[default(0f32)]
    pub size: f32,
}

#[table(accessor = game_gameobject_trap)]
pub struct GameObjectTrap {
    #[primary_key]
    pub entry: u32,
    pub spell_id: u32,
    pub cooldown_secs: u32,
}

#[table(
    accessor = game_gameobject_trap_cooldown,
    index(accessor = by_instance, btree(columns = [instance_id]))
)]
pub struct GameObjectTrapCooldown {
    #[primary_key]
    pub go_guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub ready_at: Timestamp,
}

#[table(accessor = game_lock, public, index(accessor = by_lock, btree(columns = [lock_id])))]
pub struct GameLock {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub lock_id: u32,
    pub index: u8,
    pub kind: u8,
    pub property: u32,
    pub required_skill: u32,
}

pub(crate) const LOCK_KIND_ITEM: u8 = 1;
pub(crate) const LOCK_KIND_SKILL: u8 = 2;

pub(crate) fn is_real_lock_opener(kind: u8, property: u32) -> bool {
    (kind == LOCK_KIND_SKILL && property != 0) || kind == LOCK_KIND_ITEM
}

#[table(accessor = game_gameobject_unlocked, public)]
pub struct GameObjectUnlocked {
    #[primary_key]
    pub go_guid: u64,
}

#[table(
    accessor = game_gameobject,
    public,
    index(accessor = by_map, btree(columns = [map_id])),
    index(accessor = by_grid, btree(columns = [map_id, grid_x, grid_y])),

    index(accessor = by_respawn_at, btree(columns = [respawn_at_micros])),
    index(
        accessor = by_template_scope,
        btree(columns = [template_entry, map_id, instance_id])
    ),
    // SpacetimeDB 2.7.1 serves AOI subscriptions with three equality-indexed columns.
    index(accessor = by_cell, btree(columns = [map_id, instance_id, cell]))
)]
pub struct GameObject {
    #[primary_key]
    pub guid: u64,
    pub template_entry: u32,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub state: u8,
    pub created_at: Timestamp,
    #[default(0u64)]
    pub respawn_at_micros: u64,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
    #[default(0i64)]
    pub cell: i64,
    // SpacetimeDB normalizes rotation0 to rotation_0; keep field and stored column names identical.
    #[default(0.0f32)]
    pub rotation_0: f32,
    #[default(0.0f32)]
    pub rotation_1: f32,
    #[default(0.0f32)]
    pub rotation_2: f32,
    #[default(0.0f32)]
    pub rotation_3: f32,
}

impl GameObject {
    pub(crate) fn try_insert(self, ctx: &ReducerContext) -> Result<Self, String> {
        let object = ctx.db.game_gameobject().try_insert(self)?;
        crate::aoi::gameobject_changed(ctx, &object, true);
        Ok(object)
    }

    pub(crate) fn insert(self, ctx: &ReducerContext) -> Self {
        let object = ctx.db.game_gameobject().insert(self);
        crate::aoi::gameobject_changed(ctx, &object, true);
        object
    }

    pub(crate) fn update(self, ctx: &ReducerContext) -> Self {
        let old = ctx.db.game_gameobject().guid().find(self.guid);
        let object = ctx.db.game_gameobject().guid().update(self);
        if old.as_ref().is_none_or(|old| {
            (old.map_id, old.instance_id, old.cell)
                != (object.map_id, object.instance_id, object.cell)
        }) {
            crate::aoi::gameobject_changed(ctx, &object, old.is_none());
        }
        if let Some(old) = old {
            crate::native::gameobject_changed(ctx, &old, &object);
        }
        object
    }

    pub(crate) fn delete(ctx: &ReducerContext, guid: impl std::borrow::Borrow<u64>) -> bool {
        let guid = *guid.borrow();
        let removed = ctx.db.game_gameobject().guid().delete(guid);
        crate::aoi::remove_subject(ctx, crate::aoi::GAME_OBJECT, guid);
        removed
    }
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn gameobject_destination_evidence(
    ctx: &ReducerContext,
    template_entry: u32,
    map_id: u32,
    instance_id: u64,
    limit: usize,
) -> Vec<GameObject> {
    const MAX_RESULTS: usize = 128;
    ctx.db
        .game_gameobject()
        .by_template_scope()
        .filter((template_entry, map_id, instance_id))
        .take(limit.min(MAX_RESULTS))
        .collect()
}

#[table(accessor = game_gameobject_pool, public)]
pub struct GameObjectPool {
    #[primary_key]
    pub pool_id: u32,
    pub max_active: u32,
    #[default(false)]
    pub in_place: bool,
}

#[table(accessor = game_gameobject_pool_member, public,
        index(accessor = by_pool, btree(columns = [pool_id])))]
pub struct GameObjectPoolMember {
    #[primary_key]
    #[auto_inc]
    pub point_id: u64,
    pub pool_id: u32,
    pub template_entry: u32,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub weight: u32,
}

// Bit 47 keeps pool-point guids disjoint from imported and standalone gameobjects.
pub const POOL_TAG: u64 = 1 << 47;

pub(crate) fn pool_point_guid(point_id: u64) -> u64 {
    (0xF110u64 << 48) | POOL_TAG | (point_id & 0x7FFF_FFFF_FFFF)
}

pub(crate) fn pool_point_id_of(guid: u64) -> Option<u64> {
    if (guid >> 48) == 0xF110 && (guid & POOL_TAG) != 0 {
        Some(guid & 0x7FFF_FFFF_FFFF)
    } else {
        None
    }
}

pub(crate) fn weighted_pick(weights: &[u32], roll: u32) -> Option<usize> {
    let mut cum = 0u32;
    for (i, &w) in weights.iter().enumerate() {
        cum = cum.saturating_add(w);
        if roll < cum {
            return Some(i);
        }
    }
    None
}

pub(crate) fn reroll_pool(ctx: &ReducerContext, pool_id: u32, gathered_guid: u64) {
    let gos = ctx.db.game_gameobject();
    let members = ctx.db.game_gameobject_pool_member();
    crate::go_collider::remove(ctx, gathered_guid);
    crate::GameObject::delete(ctx, gathered_guid);
    let candidates: Vec<GameObjectPoolMember> = members
        .by_pool()
        .filter(&pool_id)
        .filter(|m| m.weight > 0 && gos.guid().find(pool_point_guid(m.point_id)).is_none())
        .collect();
    if candidates.is_empty() {
        return;
    }
    let weights: Vec<u32> = candidates.iter().map(|m| m.weight).collect();
    let total: u32 = weights.iter().sum();
    let roll = ctx.random::<u32>() % total;
    let chosen =
        &candidates[weighted_pick(&weights, roll).expect("roll < total ⇒ a band always hits")];
    activate_point(ctx, chosen);
}

fn activate_point(ctx: &ReducerContext, m: &GameObjectPoolMember) {
    let go = GameObject {
        guid: pool_point_guid(m.point_id),
        template_entry: m.template_entry,
        map_id: m.map_id,
        x: m.x,
        y: m.y,
        z: m.z,
        orientation: m.orientation,
        state: 0,
        created_at: crate::time::now(ctx),
        respawn_at_micros: 0,
        instance_id: 0,
        grid_x: lyracore_shared::spatial::grid_cell(m.x, m.y).0,
        grid_y: lyracore_shared::spatial::grid_cell(m.x, m.y).1,
        cell: lyracore_shared::spatial::cell_id_at(m.x, m.y),
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0,
    }
    .insert(ctx);
    crate::go_collider::register(ctx, &go);
}

fn imported_spatial_maps(ctx: &ReducerContext) -> HashSet<u32> {
    ctx.db
        .game_terrain_chunk()
        .iter()
        .map(|c| c.map_id)
        .chain(ctx.db.game_nav_chunk().iter().map(|c| c.map_id))
        .chain(
            ctx.db
                .game_creature_spawn_map()
                .iter()
                .map(|map| map.map_id),
        )
        .collect()
}

pub(crate) fn pool_member_map_is_live(member_map: u32, imported_maps: &HashSet<u32>) -> bool {
    imported_maps.is_empty() || imported_maps.contains(&member_map)
}

pub(crate) fn eligible_pool_candidates(
    members: Vec<GameObjectPoolMember>,
    active_point_guids: &HashSet<u64>,
    imported_maps: &HashSet<u32>,
) -> Vec<GameObjectPoolMember> {
    members
        .into_iter()
        .filter(|m| {
            m.weight > 0
                && pool_member_map_is_live(m.map_id, imported_maps)
                && !active_point_guids.contains(&pool_point_guid(m.point_id))
        })
        .collect()
}

pub(crate) fn arm_pool(ctx: &ReducerContext, pool_id: u32) {
    let gos = ctx.db.game_gameobject();
    let members = ctx.db.game_gameobject_pool_member();
    let imported_maps = imported_spatial_maps(ctx);
    let max_active = ctx
        .db
        .game_gameobject_pool()
        .pool_id()
        .find(pool_id)
        .map(|p| p.max_active)
        .unwrap_or(0);
    for m in members.by_pool().filter(&pool_id) {
        crate::go_collider::remove(ctx, pool_point_guid(m.point_id));
        crate::GameObject::delete(ctx, pool_point_guid(m.point_id));
    }
    for _ in 0..max_active {
        let pool_members: Vec<GameObjectPoolMember> = members.by_pool().filter(&pool_id).collect();
        let active_point_guids: HashSet<u64> = pool_members
            .iter()
            .map(|m| pool_point_guid(m.point_id))
            .filter(|guid| gos.guid().find(guid).is_some())
            .collect();
        let candidates =
            eligible_pool_candidates(pool_members, &active_point_guids, &imported_maps);
        if candidates.is_empty() {
            break;
        }
        let weights: Vec<u32> = candidates.iter().map(|m| m.weight).collect();
        let total: u32 = weights.iter().sum();
        let roll = ctx.random::<u32>() % total;
        let chosen =
            &candidates[weighted_pick(&weights, roll).expect("roll < total ⇒ a band always hits")];
        activate_point(ctx, chosen);
    }
}

#[reducer]
pub fn arm_all_pools(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let pool_ids: Vec<u32> = ctx
        .db
        .game_gameobject_pool()
        .iter()
        .map(|p| p.pool_id)
        .collect();
    for pid in pool_ids {
        arm_pool(ctx, pid);
    }
    Ok(())
}

const USE_RANGE_SQ: f32 = crate::loot::LOOT_RANGE_SQ;

const RESPAWN_WINDOW_MICROS: u64 = 180_000_000;

pub(crate) fn respawn_window_micros(respawn_secs: u32) -> u64 {
    if respawn_secs == 0 {
        RESPAWN_WINDOW_MICROS
    } else {
        respawn_secs as u64 * 1_000_000
    }
}

pub(crate) fn respawn_due(state: u8, respawn_at_micros: u64, now: u64) -> bool {
    state == 1 && respawn_at_micros != 0 && now >= respawn_at_micros
}

fn lock_requires_opening(ctx: &ReducerContext, lock_id: u32) -> bool {
    ctx.db
        .game_lock()
        .by_lock()
        .filter(&lock_id)
        .any(|r| is_real_lock_opener(r.kind, r.property))
}

fn locked_shut(ctx: &ReducerContext, go_guid: u64, lock_id: u32) -> bool {
    lock_id != 0
        && lock_requires_opening(ctx, lock_id)
        && ctx
            .db
            .game_gameobject_unlocked()
            .go_guid()
            .find(go_guid)
            .is_none()
}

pub(crate) fn usable_go(
    ctx: &ReducerContext,
    caster_guid: u64,
    go_guid: u64,
) -> Result<(GameObject, GameObjectTemplate), crate::actor::ActionRefusal> {
    reachable_go(ctx, caster_guid, go_guid).map_err(Into::into)
}

enum ReachRefusal {
    OtherMap,
    OtherInstance,
    OutOfRange,
    Refused(crate::actor::ActionRefusal),
}

impl From<ReachRefusal> for crate::actor::ActionRefusal {
    fn from(refusal: ReachRefusal) -> Self {
        use crate::actor::{ActionRefusal, ActionRefusalKind};
        match refusal {
            ReachRefusal::OtherMap => ActionRefusal::new(
                ActionRefusalKind::OtherPartition,
                "gameobject on another map",
            ),
            ReachRefusal::OtherInstance => ActionRefusal::new(
                ActionRefusalKind::OtherPartition,
                "gameobject in another instance",
            ),
            ReachRefusal::OutOfRange => {
                ActionRefusal::new(ActionRefusalKind::OutOfRange, "gameobject out of range")
            }
            ReachRefusal::Refused(refusal) => refusal,
        }
    }
}

fn reachable_go(
    ctx: &ReducerContext,
    caster_guid: u64,
    go_guid: u64,
) -> Result<(GameObject, GameObjectTemplate), ReachRefusal> {
    use crate::actor::{ActionRefusal, ActionRefusalKind};

    let go = ctx
        .db
        .game_gameobject()
        .guid()
        .find(go_guid)
        .ok_or_else(|| {
            ReachRefusal::Refused(ActionRefusal::new(
                ActionRefusalKind::MissingTarget,
                "no such gameobject",
            ))
        })?;
    let player = crate::helpers::live_entity(ctx, caster_guid).map_err(|_| {
        ReachRefusal::Refused(ActionRefusal::new(
            ActionRefusalKind::MissingActor,
            "user not in world",
        ))
    })?;
    if player.map_id != go.map_id {
        return Err(ReachRefusal::OtherMap);
    }
    if player.instance_id != go.instance_id {
        return Err(ReachRefusal::OtherInstance);
    }
    let (dx, dy, dz) = (go.x - player.x, go.y - player.y, go.z - player.z);
    if dx * dx + dy * dy + dz * dz > USE_RANGE_SQ {
        return Err(ReachRefusal::OutOfRange);
    }
    let tmpl = ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(go.template_entry)
        .ok_or_else(|| {
            ReachRefusal::Refused(ActionRefusal::new(
                ActionRefusalKind::MissingTarget,
                format!("no gameobject template {}", go.template_entry),
            ))
        })?;
    Ok((go, tmpl))
}

pub(crate) fn apply_pick_lock(
    ctx: &ReducerContext,
    caster_guid: u64,
    go_guid: u64,
) -> Result<(), CastGateRefusal> {
    let (_go, tmpl) = reachable_go(ctx, caster_guid, go_guid).map_err(|refusal| {
        let reason = match refusal {
            ReachRefusal::OtherMap | ReachRefusal::OutOfRange => CastRefusalReason::OutOfRange,
            ReachRefusal::OtherInstance | ReachRefusal::Refused(_) => CastRefusalReason::Unreported,
        };
        CastGateRefusal::other(reason, crate::actor::ActionRefusal::from(refusal).detail)
    })?;
    if tmpl.lock_id == 0 {
        return Err("it is not locked".to_string().into());
    }
    if ctx
        .db
        .game_gameobject_unlocked()
        .go_guid()
        .find(go_guid)
        .is_some()
    {
        return Ok(());
    }
    let mut satisfied = false;
    let mut climb: Option<(u32, u32)> = None;
    for row in ctx.db.game_lock().by_lock().filter(&tmpl.lock_id) {
        if !is_real_lock_opener(row.kind, row.property) {
            continue;
        }
        if row.kind == LOCK_KIND_SKILL {
            let current = ctx
                .db
                .game_player_skill()
                .by_character()
                .filter(&caster_guid)
                .find(|s| s.skill_line == row.property)
                .map(|s| s.current as u32)
                .unwrap_or(0);
            if current >= row.required_skill {
                satisfied = true;
                climb = Some((row.property, row.required_skill));
                break;
            }
        } else if row.kind == LOCK_KIND_ITEM
            && crate::items::item_count(ctx, caster_guid, row.property) > 0
        {
            satisfied = true;
            break;
        }
    }
    if !satisfied {
        return Err("your Lockpicking is too low".to_string().into());
    }
    ctx.db
        .game_gameobject_unlocked()
        .insert(GameObjectUnlocked { go_guid });
    if let Some((line, required)) = climb {
        crate::skill::gain_profession_skill(ctx, caster_guid, line, required, 0);
    }
    Ok(())
}

pub(crate) fn request_use_gameobject(
    ctx: &ReducerContext,
    caster_guid: u64,
    go_guid: u64,
) -> Result<(), crate::actor::ActionRefusal> {
    let (go, tmpl) = usable_go(ctx, caster_guid, go_guid)?;
    use_resolved_gameobject(ctx, caster_guid, go, tmpl).map_err(Into::into)
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn apply_use_gameobject(
    ctx: &ReducerContext,
    caster_guid: u64,
    go_guid: u64,
) -> Result<(), String> {
    let (go, tmpl) = usable_go(ctx, caster_guid, go_guid).map_err(String::from)?;
    use_resolved_gameobject(ctx, caster_guid, go, tmpl)
}

fn use_resolved_gameobject(
    ctx: &ReducerContext,
    caster_guid: u64,
    mut go: GameObject,
    tmpl: GameObjectTemplate,
) -> Result<(), String> {
    let (used_go_guid, used_go_entry, used_go_instance) =
        (go.guid, go.template_entry, go.instance_id);
    match tmpl.type_id {
        go_type::CHEST => {
            if locked_shut(ctx, go.guid, tmpl.lock_id) {
                return Err("it is locked".to_string());
            }
            if go.state != 0 {
                return Err("chest already looted".to_string());
            }

            let raw: Vec<(u32, u32, u32, u32, bool)> = if tmpl.data1 != 0 {
                ctx.db
                    .game_gameobject_loot()
                    .by_loot()
                    .filter(&tmpl.data1)
                    .map(|r| (r.item_entry, r.chance_bp, r.count, r.group_id, r.quest_only))
                    .collect()
            } else {
                Vec::new()
            };
            let mut winners = crate::loot::roll_loot_rows_quest_aware(ctx, raw);
            if winners.is_empty() && tmpl.data0 != 0 {
                winners.push((tmpl.data0, 1, false));
            }
            crate::loot::insert_corpse_rows(ctx, go.guid, winners);
            go.state = 1;
            go.update(ctx);
        }
        go_type::GOOBER => {
            if locked_shut(ctx, go.guid, tmpl.lock_id) {
                return Err("it is locked".to_string());
            }
            crate::quest::on_gameobject_used(ctx, caster_guid, go.template_entry);
        }
        go_type::GATHER => {
            if go.state != 0 {
                return Err("node already gathered".to_string());
            }
            let line = tmpl.gather_skill_line;
            let learned = ctx
                .db
                .game_player_skill()
                .by_character()
                .filter(&caster_guid)
                .find(|s| s.skill_line == line)
                .map(|s| s.current as u32)
                .unwrap_or(0);
            can_gather(learned, tmpl.data1)?;
            // Grant before depletion so a full bag leaves the node gatherable.
            crate::items::grant_item(ctx, caster_guid, tmpl.data0, 1)?;
            crate::skill::gain_profession_skill(
                ctx,
                caster_guid,
                line,
                tmpl.data1,
                tmpl.gather_gray,
            );
            match pool_point_id_of(go.guid)
                .and_then(|pid| ctx.db.game_gameobject_pool_member().point_id().find(pid))
            {
                Some(member)
                    if !ctx
                        .db
                        .game_gameobject_pool()
                        .pool_id()
                        .find(member.pool_id)
                        .map(|p| p.in_place)
                        .unwrap_or(false) =>
                {
                    reroll_pool(ctx, member.pool_id, go.guid);
                }
                _ => {
                    go.state = 1;
                    go.respawn_at_micros = (crate::time::now(ctx).to_micros_since_unix_epoch()
                        as u64)
                        + respawn_window_micros(tmpl.respawn_secs);
                    go.update(ctx);
                }
            }
        }
        go_type::DOOR | go_type::BUTTON => {
            if locked_shut(ctx, go.guid, tmpl.lock_id) {
                return Err("it is locked".to_string());
            }
            go.state = toggle_state(go.state);
            go.update(ctx);
        }
        _ => {}
    }
    crate::hooks::fire_on_go_used(
        ctx,
        &crate::hooks::GoUsedPayload {
            go_guid: used_go_guid,
            go_entry: used_go_entry,
            user_guid: caster_guid,
            instance_id: used_go_instance,
        },
    );
    Ok(())
}

pub(crate) fn despawn_from_relay(ctx: &ReducerContext, go_guid: u64) -> Result<(), String> {
    if ctx.db.game_gameobject().guid().find(go_guid).is_none() {
        return Err(format!("relay gameobject {go_guid} is missing"));
    }
    crate::loot::reap_corpse_loot_family(ctx, go_guid);
    ctx.db.game_gameobject_unlocked().go_guid().delete(go_guid);
    ctx.db
        .game_gameobject_trap_cooldown()
        .go_guid()
        .delete(go_guid);
    crate::go_collider::remove(ctx, go_guid);
    GameObject::delete(ctx, go_guid);
    Ok(())
}

pub(crate) fn activate_trap_from_relay(
    ctx: &ReducerContext,
    source_guid: u64,
    go_guid: u64,
) -> Result<(), String> {
    let source = ctx
        .db
        .game_world_entity()
        .guid()
        .find(source_guid)
        .filter(|entity| !entity.dead)
        .ok_or_else(|| format!("relay trap user {source_guid} is unavailable"))?;
    let object = ctx
        .db
        .game_gameobject()
        .guid()
        .find(go_guid)
        .filter(|object| object.map_id == source.map_id && object.instance_id == source.instance_id)
        .ok_or_else(|| format!("relay trap {go_guid} is unavailable"))?;
    let template = ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(object.template_entry)
        .filter(|template| template.type_id == go_type::TRAP)
        .ok_or_else(|| format!("relay object {go_guid} is not a trap"))?;
    let trap = ctx
        .db
        .game_gameobject_trap()
        .entry()
        .find(template.entry)
        .filter(|trap| trap.spell_id != 0)
        .ok_or_else(|| format!("trap metadata {} is missing", template.entry))?;
    if ctx
        .db
        .game_gameobject_trap_cooldown()
        .go_guid()
        .find(go_guid)
        .is_some_and(|cooldown| cooldown.ready_at > crate::time::now(ctx))
    {
        return Ok(());
    }
    crate::spell::start_creature_spell(
        ctx,
        crate::spell::CreatureSpellStart {
            caster_guid: source_guid,
            caster_level: source.level as u8,
            spell_id: trap.spell_id,
            mode: crate::spell::CreatureSpellStartMode::Triggered,
            target: crate::spell::CreatureSpellTarget::Unit(source_guid),
            interrupt_previous: false,
            admission: crate::spell::CreatureSpellCasterAdmission::Living,
        },
    )?;
    let cooldown_secs = trap_cooldown_secs(trap.cooldown_secs);
    let ready_at = crate::time::now(ctx)
        .checked_add(TimeDuration::from_micros(
            i64::from(cooldown_secs) * 1_000_000,
        ))
        .unwrap_or(crate::time::now(ctx));
    let cooldowns = ctx.db.game_gameobject_trap_cooldown();
    let row = GameObjectTrapCooldown {
        go_guid,
        map_id: object.map_id,
        instance_id: object.instance_id,
        ready_at,
    };
    if cooldowns.go_guid().find(go_guid).is_some() {
        cooldowns.go_guid().update(row);
    } else {
        cooldowns.insert(row);
    }
    Ok(())
}

fn trap_cooldown_secs(source_cooldown_secs: u32) -> u32 {
    if source_cooldown_secs == 0 {
        4
    } else {
        source_cooldown_secs
    }
}

pub(crate) fn clear_relay_trap_cooldowns_for_instance(ctx: &ReducerContext, instance_id: u64) {
    let cooldowns = ctx.db.game_gameobject_trap_cooldown();
    for row in cooldowns
        .by_instance()
        .filter(&instance_id)
        .collect::<Vec<_>>()
    {
        cooldowns.go_guid().delete(row.go_guid);
    }
}

pub(crate) fn toggle_state(state: u8) -> u8 {
    if state == 0 {
        1
    } else {
        0
    }
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_pick_lock_entry(
    ctx: &ReducerContext,
    character_guid: u64,
    go_entry: u32,
) -> Result<(), String> {
    let go = ctx
        .db
        .game_gameobject()
        .iter()
        .find(|g| g.template_entry == go_entry)
        .ok_or_else(|| format!("no spawned gameobject of entry {go_entry}"))?;
    Ok(apply_pick_lock(ctx, character_guid, go.guid)?)
}

#[reducer]
pub fn import_gameobjects(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let gos = ctx.db.game_gameobject();
    let guids: Vec<u64> = gos.iter().map(|g| g.guid).collect();
    for g in guids {
        crate::go_collider::remove(ctx, g);
        crate::GameObject::delete(ctx, g);
    }
    if load_go_batch(ctx, &packed)? == 0 {
        return Err("import payload was empty".to_string());
    }
    Ok(())
}

#[reducer]
pub fn import_gameobjects_append(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    load_go_batch(ctx, &packed)?;
    Ok(())
}

fn load_go_batch(ctx: &ReducerContext, packed: &str) -> Result<u32, String> {
    let now = crate::time::now(ctx);
    let mut loaded = 0u32;
    for row in packed.split(';').filter(|r| !r.is_empty()) {
        let f: Vec<&str> = row.split(',').collect();
        if f.len() != 12 {
            return Err(format!(
                "gameobject import row needs 12 fields, got {}: {row}",
                f.len()
            ));
        }
        let pu64 = |s: &str| s.parse::<u64>().map_err(|_| format!("bad u64: {s}"));
        let pu32 = |s: &str| s.parse::<u32>().map_err(|_| format!("bad u32: {s}"));
        let pf32 = |s: &str| s.parse::<f32>().map_err(|_| format!("bad f32: {s}"));
        let pu8 = |s: &str| s.parse::<u8>().map_err(|_| format!("bad u8 state: {s}"));

        let (gx_src, gy_src) = (pf32(f[3])?, pf32(f[4])?);
        let go = GameObject {
            guid: pu64(f[0])?,
            template_entry: pu32(f[1])?,
            map_id: pu32(f[2])?,
            x: pf32(f[3])?,
            y: pf32(f[4])?,
            z: pf32(f[5])?,
            orientation: pf32(f[6])?,
            state: pu8(f[7])?,
            created_at: now,
            respawn_at_micros: 0,
            instance_id: 0,

            grid_x: lyracore_shared::spatial::grid_cell(gx_src, gy_src).0,
            grid_y: lyracore_shared::spatial::grid_cell(gx_src, gy_src).1,
            cell: lyracore_shared::spatial::cell_id_at(gx_src, gy_src),
            rotation_0: pf32(f[8])?,
            rotation_1: pf32(f[9])?,
            rotation_2: pf32(f[10])?,
            rotation_3: pf32(f[11])?,
        }
        .try_insert(ctx)
        .map_err(|e| format!("gameobject insert failed (dup guid?): {e}"))?;
        crate::go_collider::register(ctx, &go);
        loaded += 1;
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_trap_uses_the_source_default_only_for_zero_cooldown() {
        assert_eq!(trap_cooldown_secs(0), 4);
        assert_eq!(trap_cooldown_secs(1), 1);
        assert_eq!(trap_cooldown_secs(30), 30);
    }

    #[test]
    fn go_type_ids_are_the_client_gameobject_types() {
        assert_eq!(go_type::DOOR, 0);
        assert_eq!(go_type::BUTTON, 1);
        assert_eq!(go_type::CHEST, 3);
        assert_eq!(go_type::TRAP, 6);
        assert_eq!(go_type::GOOBER, 10);
    }

    #[test]
    fn questgiver_type_id_matches_the_shared_constant_and_is_cmangos_type_2() {
        assert_eq!(go_type::QUESTGIVER, 2);
        assert_eq!(
            go_type::QUESTGIVER,
            lyracore_shared::constants::go_type::QUESTGIVER
        );
    }

    #[test]
    fn toggle_state_flips_zero_and_one_and_is_total() {
        assert_eq!(toggle_state(0), 1, "closed -> open");
        assert_eq!(toggle_state(1), 0, "open -> closed");
        assert_eq!(toggle_state(2), 0);
        assert_eq!(toggle_state(255), 0);
    }

    #[test]
    fn is_real_lock_opener_ignores_property_zero_skill_slots() {
        assert!(
            is_real_lock_opener(LOCK_KIND_SKILL, 633),
            "Lockpicking 633 skill row is a real opener"
        );
        assert!(
            is_real_lock_opener(LOCK_KIND_SKILL, 186),
            "any real SkillLine (Mining) is a real opener"
        );
        assert!(
            !is_real_lock_opener(LOCK_KIND_SKILL, 0),
            "property==0 skill slot (unmapped LockType) is NOT an opener"
        );
        assert!(
            is_real_lock_opener(LOCK_KIND_ITEM, 5397),
            "an item-key row is a real opener"
        );
        assert!(
            !is_real_lock_opener(0, 633),
            "an unmodeled lock kind is never an opener"
        );
        assert!(
            !is_real_lock_opener(9, 633),
            "an unmodeled lock kind is never an opener"
        );
        assert!(
            is_real_lock_opener(LOCK_KIND_SKILL, 633) && !is_real_lock_opener(LOCK_KIND_SKILL, 0)
        );
    }

    #[test]
    fn can_gather_admits_learned_and_rejects_unlearned_or_too_low() {
        assert!(can_gather(1, 1).is_ok());
        assert!(can_gather(75, 1).is_ok());
        assert_eq!(
            can_gather(0, 1).unwrap_err(),
            "you have not learned this gathering skill"
        );
        assert_eq!(
            can_gather(5, 25).unwrap_err(),
            "your skill is too low to gather this node"
        );
    }

    #[test]
    fn respawn_window_micros_defaults_to_the_const_else_scales_seconds() {
        assert_eq!(respawn_window_micros(0), RESPAWN_WINDOW_MICROS);
        assert_eq!(respawn_window_micros(0), 180_000_000);
        assert_eq!(respawn_window_micros(30), 30_000_000);
        assert_eq!(respawn_window_micros(1), 1_000_000);
        assert_eq!(respawn_window_micros(180), RESPAWN_WINDOW_MICROS);
    }

    #[test]
    fn pool_member_map_is_live_admits_everything_before_any_real_import() {
        let none_imported: HashSet<u32> = HashSet::new();
        assert!(
            pool_member_map_is_live(0, &none_imported),
            "map 0 admitted pre-import"
        );
        assert!(
            pool_member_map_is_live(1, &none_imported),
            "map 1 admitted pre-import"
        );
        assert!(
            pool_member_map_is_live(36, &none_imported),
            "any map admitted pre-import"
        );
    }

    #[test]
    fn pool_member_map_is_live_gates_by_map_once_something_is_really_imported() {
        let map_1_imported: HashSet<u32> = [1].into_iter().collect();
        assert!(
            pool_member_map_is_live(1, &map_1_imported),
            "the imported map itself stays live"
        );
        assert!(
            !pool_member_map_is_live(0, &map_1_imported),
            "init's map-0 fixture is foreign now"
        );
        assert!(
            !pool_member_map_is_live(36, &map_1_imported),
            "any other foreign map is refused too"
        );
        let multi_map: HashSet<u32> = [0, 36].into_iter().collect();
        assert!(pool_member_map_is_live(0, &multi_map));
        assert!(pool_member_map_is_live(36, &multi_map));
        assert!(!pool_member_map_is_live(1, &multi_map));
    }

    #[test]
    fn eligible_pool_candidates_applies_weight_map_and_already_active_together() {
        fn member(point_id: u64, map_id: u32, weight: u32) -> GameObjectPoolMember {
            GameObjectPoolMember {
                point_id,
                pool_id: 2,
                template_entry: 1731,
                map_id,
                x: 0.0,
                y: 0.0,
                z: 0.0,
                orientation: 0.0,
                weight,
            }
        }
        let live_map: HashSet<u32> = [1].into_iter().collect();
        let members = vec![
            member(1, 1, 5),
            member(2, 1, 0),
            member(3, 0, 5),
            member(4, 1, 5),
        ];
        let active_point_guids: HashSet<u64> = [pool_point_guid(4)].into_iter().collect();
        let out = eligible_pool_candidates(members, &active_point_guids, &live_map);
        let ids: Vec<u64> = out.iter().map(|m| m.point_id).collect();
        assert_eq!(
            ids,
            vec![1],
            "only the live-map, weighted, inactive member survives all three gates"
        );
    }

    #[test]
    fn respawn_due_flips_only_a_depleted_armed_and_elapsed_node() {
        let armed_at = 1_000u64;
        assert!(
            respawn_due(1, armed_at, armed_at),
            "due exactly at the armed time"
        );
        assert!(
            respawn_due(1, armed_at, armed_at + 1),
            "due once past the armed time"
        );
        assert!(
            !respawn_due(1, armed_at, armed_at - 1),
            "before the armed time → not due"
        );
        assert!(
            !respawn_due(1, 0, u64::MAX),
            "never-armed depleted node stays depleted"
        );
        assert!(
            !respawn_due(0, armed_at, armed_at + 1),
            "a ready node is never a respawn candidate"
        );
    }

    #[test]
    fn weighted_pick_lands_each_roll_in_its_cumulative_band() {
        let w = [20u32, 20, 20, 20, 20, 2];
        assert_eq!(weighted_pick(&w, 0), Some(0));
        assert_eq!(weighted_pick(&w, 19), Some(0));
        assert_eq!(weighted_pick(&w, 20), Some(1));
        assert_eq!(weighted_pick(&w, 99), Some(4));
        assert_eq!(weighted_pick(&w, 100), Some(5));
        assert_eq!(weighted_pick(&w, 101), Some(5));
        let w0 = [10u32, 0, 10];
        assert_eq!(weighted_pick(&w0, 9), Some(0));
        assert_eq!(weighted_pick(&w0, 10), Some(2));
        assert_eq!(weighted_pick(&[5u32, 5], 10), None);
    }

    #[test]
    fn pool_point_guid_tagged_namespace_is_disjoint_from_standalone() {
        const GO_HIGH: u64 = 0xF110u64 << 48;
        for point_id in [1u64, 2, 3, 4, 42, 9999] {
            let guid = pool_point_guid(point_id);
            assert_eq!(
                guid >> 48,
                0xF110,
                "carries HIGHGUID_GAMEOBJECT in bits 48..63"
            );
            assert_ne!(guid & POOL_TAG, 0, "tagged as a pool point");
            assert_eq!(
                pool_point_id_of(guid),
                Some(point_id),
                "round-trips to point_id"
            );
        }
        for standalone in [GO_HIGH | 3, GO_HIGH | 4, GO_HIGH | 50102, GO_HIGH | 1234] {
            assert_eq!(
                pool_point_id_of(standalone),
                None,
                "standalone GO is not a pool point"
            );
            assert_ne!(
                standalone,
                pool_point_guid(standalone & 0x7FFF_FFFF_FFFF),
                "disjoint from pooled"
            );
        }
    }

    #[test]
    fn gather_deplete_routes_pooled_to_reroll_and_standalone_to_in_place() {
        let pooled = pool_point_guid(1_000_000);
        assert_eq!(
            pool_point_id_of(pooled),
            Some(1_000_000),
            "pooled point → consult the pool's in_place"
        );
        let standalone = (0xF110u64 << 48) | 50102;
        assert_eq!(
            pool_point_id_of(standalone),
            None,
            "standalone gather node → in-place respawn timer"
        );
        assert_eq!(
            respawn_window_micros(300),
            300_000_000,
            "standalone vein arms its 300s window"
        );
        assert_eq!(
            respawn_window_micros(0),
            RESPAWN_WINDOW_MICROS,
            "default node keeps the 3-min fallback"
        );
    }

    #[test]
    fn tier_variety_weighted_reroll_is_eightyfive_fifteen_and_repeats_the_common_tier() {
        let w = [85u32, 15];
        assert_eq!(weighted_pick(&w, 0), Some(0), "low roll → Copper");
        assert_eq!(
            weighted_pick(&w, 84),
            Some(0),
            "band end (half-open) → still Copper"
        );
        assert_eq!(weighted_pick(&w, 85), Some(1), "the Tin band starts at 85");
        assert_eq!(
            weighted_pick(&w, 99),
            Some(1),
            "…and ends at 99 → 15/100 rolls hit Tin"
        );
        let copper = (0..100u32)
            .filter(|&r| weighted_pick(&w, r) == Some(0))
            .count();
        let tin = (0..100u32)
            .filter(|&r| weighted_pick(&w, r) == Some(1))
            .count();
        assert_eq!(
            (copper, tin),
            (85, 15),
            "the band split IS the weight split (no flip, repeats allowed)"
        );
        let coords = [(-9620.11f32, -46.3336f32, 47.3641f32); 2];
        for roll in [0u32, 50, 84, 85, 99] {
            let idx = weighted_pick(&w, roll).unwrap();
            assert_eq!(
                coords[idx], coords[0],
                "a rolled tier spawns at the SAME co-located coord (no wander)"
            );
        }
    }

    #[test]
    fn tier_variety_skill_gate_blocks_a_rolled_tin_for_a_low_char() {
        let (copper_req, tin_req) = (1u32, 65u32);
        assert!(
            can_gather(50, copper_req).is_ok(),
            "a rolled Copper is always tappable"
        );
        assert_eq!(
            can_gather(50, tin_req).unwrap_err(),
            "your skill is too low to gather this node",
            "a rolled Tin teases a low char until Mining 65"
        );
        assert!(
            can_gather(65, tin_req).is_ok(),
            "Mining 65 unlocks the rolled Tin"
        );
        assert_eq!(
            can_gather(0, copper_req).unwrap_err(),
            "you have not learned this gathering skill"
        );
    }
}
