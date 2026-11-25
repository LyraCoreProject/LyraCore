use std::collections::HashSet;

use lyracore_shared::constants::go_type;
use spacetimedb::{reducer, table, ReducerContext, Table, TimeDuration, Timestamp};

use crate::game_corpse_loot;
use crate::game_corpse_loot_eligible;
use crate::game_creature_spawn_map;
use crate::game_gameobject_loot;
use crate::game_map;
use crate::game_world_entity;
use crate::native::game_native_player;
use crate::nav::game_nav_chunk;
use crate::spell::{CastGateRefusal, CastRefusalReason};
use crate::terrain::game_terrain_chunk;

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
    // Copper a chest rolls between when it opens, as vmangos mingold..maxgold.
    #[default(0u32)]
    pub money_min: u32,
    #[default(0u32)]
    pub money_max: u32,
}

#[table(accessor = game_gameobject_trap)]
pub struct GameObjectTrap {
    #[primary_key]
    pub entry: u32,
    pub spell_id: u32,
    pub cooldown_secs: u32,
}

// A type-18 template's vmangos summoningRitual data: data0, data1, data2, data3 and data6.
#[table(accessor = game_gameobject_ritual)]
pub struct GameObjectRitual {
    #[primary_key]
    pub entry: u32,
    pub required_participants: u32,
    pub spell_id: u32,
    pub anim_spell_id: u32,
    pub persistent: bool,
    pub casters_grouped: bool,
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
    // SpacetimeDB 2.14.0 serves AOI subscriptions with three equality-indexed columns.
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
    // The source spawn's respawn range in seconds; 0 and 0 take the template's `respawn_secs`.
    #[default(0u32)]
    pub respawn_min_secs: u32,
    #[default(0u32)]
    pub respawn_max_secs: u32,
    // A looted chest or gathered node is hidden from every client until it respawns.
    #[default(false)]
    pub despawned: bool,
}

impl GameObject {
    fn stamp_cell(&mut self) {
        let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(self.x, self.y);
        self.grid_x = grid_x;
        self.grid_y = grid_y;
        self.cell = lyracore_shared::spatial::grid_cell_id(grid_x, grid_y);
    }

    /// Inserts the object with its grid address stamped from its position and registers its
    /// collider; `Err` on a guid already in use.
    pub(crate) fn try_insert(mut self, ctx: &ReducerContext) -> Result<Self, String> {
        self.stamp_cell();
        let object = ctx.db.game_gameobject().try_insert(self)?;
        #[cfg(feature = "conformance_test_fixture")]
        crate::native::debug::record_birth(
            ctx,
            object.guid,
            object.map_id,
            object.instance_id,
            object.cell,
        );
        crate::go_collider::register(ctx, &object);
        crate::aoi::gameobject_changed(ctx, &object, true);
        Ok(object)
    }

    pub(crate) fn insert(self, ctx: &ReducerContext) -> Self {
        let guid = self.guid;
        self.try_insert(ctx)
            .unwrap_or_else(|e| panic!("gameobject {guid} did not insert: {e}"))
    }

    pub(crate) fn update(mut self, ctx: &ReducerContext) -> Self {
        self.stamp_cell();
        let old = ctx.db.game_gameobject().guid().find(self.guid);
        let object = ctx.db.game_gameobject().guid().update(self);
        if old.as_ref().is_none_or(|old| {
            (
                old.template_entry,
                old.map_id,
                old.instance_id,
                old.x,
                old.y,
                old.z,
            ) != (
                object.template_entry,
                object.map_id,
                object.instance_id,
                object.x,
                object.y,
                object.z,
            )
        }) {
            crate::go_collider::register(ctx, &object);
        }
        if old.as_ref().is_none_or(|old| {
            (old.map_id, old.instance_id, old.cell, old.despawned)
                != (
                    object.map_id,
                    object.instance_id,
                    object.cell,
                    object.despawned,
                )
        }) {
            crate::aoi::gameobject_changed(ctx, &object, old.is_none());
        }
        if let Some(old) = old {
            crate::native::object::changed(ctx, &old, &object);
        }
        object
    }

    pub(crate) fn delete(ctx: &ReducerContext, guid: impl std::borrow::Borrow<u64>) -> bool {
        let guid = *guid.borrow();
        let removed = ctx.db.game_gameobject().guid().delete(guid);
        crate::go_collider::remove(ctx, guid);
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
        .filter(|object| !object.despawned)
        .take(limit.min(MAX_RESULTS))
        .collect()
}

#[table(accessor = game_gameobject_pool, public)]
pub struct GameObjectPool {
    #[primary_key]
    pub pool_id: u32,
    pub max_active: u32,
    // Unused: every pool rolls a used member's replacement and shows it after its respawn delay.
    #[default(false)]
    pub in_place: bool,
}

// `weight` is a member's share of a roll; docs/schema.md#gameobject-pools gives the rule.
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
    #[default(0.0f32)]
    pub rotation_0: f32,
    #[default(0.0f32)]
    pub rotation_1: f32,
    #[default(0.0f32)]
    pub rotation_2: f32,
    #[default(0.0f32)]
    pub rotation_3: f32,
    // The source spawn's respawn range in seconds; 0 and 0 take the template's `respawn_secs`.
    #[default(0u32)]
    pub respawn_min_secs: u32,
    #[default(0u32)]
    pub respawn_max_secs: u32,
}

/// A pool's parent, as vmangos `pool_pool`, and its map; docs/schema.md#gameobject-pools.
#[table(
    accessor = game_gameobject_pool_link,
    index(accessor = by_parent, btree(columns = [parent_pool_id])),
    index(accessor = by_map, btree(columns = [map_id]))
)]
pub struct GameObjectPoolLink {
    #[primary_key]
    pub pool_id: u32,
    // 0 for a top-level pool.
    pub parent_pool_id: u32,
    // The pool's share of a roll among its parent's child pools.
    pub weight: u32,
    pub map_id: u32,
}

/// A pool member's object in one Instance; its guid is `pool_copy_guid(copy_id)`.
#[table(
    accessor = game_gameobject_pool_copy,
    index(accessor = by_instance_point, btree(columns = [instance_id, point_id]))
)]
pub struct GameObjectPoolCopy {
    #[primary_key]
    #[auto_inc]
    pub copy_id: u64,
    pub instance_id: u64,
    pub point_id: u64,
}

// Bit 47 keeps pool-point guids disjoint from imported and standalone gameobjects.
pub const POOL_TAG: u64 = 1 << 47;
// Bit 46 reserves gameobject Instance copies below pool bit 47; a pool member's copy sets both.
pub(crate) const GO_COPY_BAND: u64 = 1 << 46;
// Bit 45 reserves Duel flags below the Instance copies.
pub(crate) const DUEL_FLAG_BAND: u64 = 1 << 45;
// Bit 44 reserves Summoning Ritual objects below the Duel flags.
pub(crate) const RITUAL_BAND: u64 = 1 << 44;

pub(crate) fn gameobject_guid(low: u64) -> u64 {
    lyracore_shared::guid::compose(lyracore_shared::guid::GAMEOBJECT, low)
}

pub(crate) use lyracore_shared::guid::is_gameobject as is_gameobject_guid;

/// The guid of a Shard-local object `local_id` in `band`; `None` once the band is exhausted.
pub(crate) fn banded_object_guid(band: u64, character_guid: u64, local_id: u64) -> Option<u64> {
    // Shard-local ids need the realm-wide GUID slot of their Character to stay unique.
    let range_size = crate::auth::GUID_RANGE_SIZE;
    if local_id >= range_size {
        return None;
    }
    let shard_slot = character_guid / range_size;
    let payload = shard_slot.checked_mul(range_size)?.checked_add(local_id)?;
    (payload < band).then_some(gameobject_guid(band | payload))
}

/// Replaces the template's standalone spawn at its packed guid; keeps an existing template row.
#[cfg(feature = "debug_reducers")]
pub(crate) fn spawn_standalone(
    ctx: &ReducerContext,
    template: GameObjectTemplate,
    map_id: u32,
    [x, y, z]: [f32; 3],
) -> GameObject {
    let template_entry = template.entry;
    if ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(template_entry)
        .is_none()
    {
        ctx.db.game_gameobject_template().insert(template);
    }
    let guid = lyracore_package_delta::packed_gameobject_spawn_guid(template_entry);
    GameObject::delete(ctx, guid);
    ctx.db.game_gameobject_unlocked().go_guid().delete(guid);
    GameObject {
        guid,
        template_entry,
        map_id,
        x,
        y,
        z,
        orientation: 0.0,
        state: 0,
        created_at: crate::time::now(ctx),
        respawn_at_micros: 0,
        instance_id: 0,
        grid_x: 0,
        grid_y: 0,
        cell: 0,
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0,
        respawn_min_secs: 0,
        respawn_max_secs: 0,
        despawned: false,
    }
    .insert(ctx)
}

pub(crate) fn pool_point_guid(point_id: u64) -> u64 {
    gameobject_guid(POOL_TAG | (point_id & (GO_COPY_BAND - 1)))
}

/// The pool point of an open-world pool object's guid.
pub(crate) fn pool_point_id_of(guid: u64) -> Option<u64> {
    (is_gameobject_guid(guid) && guid & (POOL_TAG | GO_COPY_BAND) == POOL_TAG)
        .then_some(guid & (GO_COPY_BAND - 1))
}

fn pool_copy_guid(copy_id: u64) -> u64 {
    gameobject_guid(POOL_TAG | GO_COPY_BAND | (copy_id & (GO_COPY_BAND - 1)))
}

fn pool_copy_id_of(guid: u64) -> Option<u64> {
    (is_gameobject_guid(guid) && guid & (POOL_TAG | GO_COPY_BAND) == POOL_TAG | GO_COPY_BAND)
        .then_some(guid & (GO_COPY_BAND - 1))
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

fn pick_weighted<'a, T>(
    ctx: &ReducerContext,
    candidates: &'a [T],
    weight: impl Fn(&T) -> u32,
) -> Option<&'a T> {
    let weights: Vec<u32> = candidates.iter().map(weight).collect();
    let total = weights.iter().fold(0u32, |sum, w| sum.saturating_add(*w));
    if total == 0 {
        return None;
    }
    weighted_pick(&weights, ctx.random::<u32>() % total).map(|index| &candidates[index])
}

// Imported data holds three levels; the cap stops a malformed link cycle.
const MAX_POOL_DEPTH: u32 = 8;

/// Where a pool rolls: the open world, or one Instance of the pool's map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PoolScope {
    World,
    Instance { instance_id: u64, map_id: u32 },
}

impl PoolScope {
    fn instance_id(self) -> u64 {
        match self {
            Self::World => 0,
            Self::Instance { instance_id, .. } => instance_id,
        }
    }

    fn admits_map(self, map_id: u32) -> bool {
        match self {
            Self::World => true,
            Self::Instance { map_id: own, .. } => own == map_id,
        }
    }

    /// The guid of the point's object in this scope, while the point is up here.
    fn active_guid(self, ctx: &ReducerContext, point_id: u64) -> Option<u64> {
        match self {
            Self::World => {
                let guid = pool_point_guid(point_id);
                ctx.db.game_gameobject().guid().find(guid).map(|_| guid)
            }
            Self::Instance { instance_id, .. } => ctx
                .db
                .game_gameobject_pool_copy()
                .by_instance_point()
                .filter((instance_id, point_id))
                .next()
                .map(|copy| pool_copy_guid(copy.copy_id)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PoolTiming {
    Now,
    AfterRespawnDelay,
}

/// The maps an open-world pool arms on: an imported map that no Instance copies.
struct MapFence {
    imported: HashSet<u32>,
    instanceable: HashSet<u32>,
}

impl MapFence {
    fn load(ctx: &ReducerContext) -> Self {
        Self {
            imported: imported_spatial_maps(ctx),
            ..Self::instanceable(ctx)
        }
    }

    /// Fences only the maps an Instance copies, without reading terrain or navigation.
    fn instanceable(ctx: &ReducerContext) -> Self {
        Self {
            imported: HashSet::new(),
            instanceable: ctx
                .db
                .game_map()
                .iter()
                .map(|map| map.id)
                .filter(|&map_id| crate::instance::is_instanceable(ctx, map_id))
                .collect(),
        }
    }

    fn admits(&self, map_id: u32) -> bool {
        pool_member_map_is_live(map_id, &self.imported) && !self.instanceable.contains(&map_id)
    }
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

pub(crate) fn eligible_pool_candidates<'a>(
    members: &'a [GameObjectPoolMember],
    active_points: &HashSet<u64>,
    admits_map: impl Fn(u32) -> bool,
) -> Vec<&'a GameObjectPoolMember> {
    members
        .iter()
        .filter(|m| m.weight > 0 && admits_map(m.map_id) && !active_points.contains(&m.point_id))
        .collect()
}

fn pool_limit(ctx: &ReducerContext, pool_id: u32) -> u32 {
    ctx.db
        .game_gameobject_pool()
        .pool_id()
        .find(pool_id)
        .map_or(0, |pool| pool.max_active)
}

fn pool_members(ctx: &ReducerContext, pool_id: u32) -> Vec<GameObjectPoolMember> {
    ctx.db
        .game_gameobject_pool_member()
        .by_pool()
        .filter(&pool_id)
        .collect()
}

fn child_pools(ctx: &ReducerContext, pool_id: u32) -> Vec<GameObjectPoolLink> {
    if pool_id == 0 {
        return Vec::new();
    }
    ctx.db
        .game_gameobject_pool_link()
        .by_parent()
        .filter(&pool_id)
        .collect()
}

/// The pool member an object stands for, and the scope its pool rolls in.
fn pool_point_of(ctx: &ReducerContext, guid: u64) -> Option<(GameObjectPoolMember, PoolScope)> {
    let members = ctx.db.game_gameobject_pool_member();
    if let Some(point_id) = pool_point_id_of(guid) {
        return members
            .point_id()
            .find(point_id)
            .map(|member| (member, PoolScope::World));
    }
    let copy = ctx
        .db
        .game_gameobject_pool_copy()
        .copy_id()
        .find(pool_copy_id_of(guid)?)?;
    let member = members.point_id().find(copy.point_id)?;
    let scope = PoolScope::Instance {
        instance_id: copy.instance_id,
        map_id: member.map_id,
    };
    Some((member, scope))
}

/// The open-world pool member whose object `guid` is.
pub(crate) fn world_pool_member(ctx: &ReducerContext, guid: u64) -> Option<GameObjectPoolMember> {
    ctx.db
        .game_gameobject_pool_member()
        .point_id()
        .find(pool_point_id_of(guid)?)
}

fn pool_is_active(ctx: &ReducerContext, scope: PoolScope, pool_id: u32, depth: u32) -> bool {
    depth <= MAX_POOL_DEPTH
        && (pool_members(ctx, pool_id)
            .iter()
            .any(|m| scope.active_guid(ctx, m.point_id).is_some())
            || child_pools(ctx, pool_id)
                .iter()
                .any(|child| pool_is_active(ctx, scope, child.pool_id, depth + 1)))
}

fn despawn_point(ctx: &ReducerContext, scope: PoolScope, point_id: u64) {
    let Some(guid) = scope.active_guid(ctx, point_id) else {
        return;
    };
    crate::loot::reap_corpse_loot_family(ctx, guid);
    close_loot_windows(ctx, guid);
    ctx.db.game_gameobject_unlocked().go_guid().delete(guid);
    ctx.db
        .game_gameobject_trap_cooldown()
        .go_guid()
        .delete(guid);
    GameObject::delete(ctx, guid);
    if let Some(copy_id) = pool_copy_id_of(guid) {
        ctx.db.game_gameobject_pool_copy().copy_id().delete(copy_id);
    }
}

fn despawn_pool(ctx: &ReducerContext, scope: PoolScope, pool_id: u32, depth: u32) {
    if depth > MAX_POOL_DEPTH {
        return;
    }
    for member in pool_members(ctx, pool_id) {
        despawn_point(ctx, scope, member.point_id);
    }
    for child in child_pools(ctx, pool_id) {
        despawn_pool(ctx, scope, child.pool_id, depth + 1);
    }
}

/// When a rolled member shows; 0, never, for a chest without a respawn time.
fn replacement_respawn_at(
    ctx: &ReducerContext,
    member: &GameObjectPoolMember,
    instance_id: u64,
) -> u64 {
    let template = ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(member.template_entry);
    let delay = spawn_respawn_secs(
        member.respawn_min_secs,
        member.respawn_max_secs,
        template
            .as_ref()
            .map_or(0, |template| template.respawn_secs),
        ctx.random(),
    );
    let now = now_micros(ctx);
    if template.is_some_and(|template| template.type_id == go_type::CHEST) {
        chest_respawn_at(now, delay, instance_id).unwrap_or(0)
    } else {
        now + respawn_window_micros(delay)
    }
}

fn activate_point(
    ctx: &ReducerContext,
    scope: PoolScope,
    member: &GameObjectPoolMember,
    timing: PoolTiming,
) {
    let instance_id = scope.instance_id();
    let guid = match scope {
        PoolScope::World => pool_point_guid(member.point_id),
        PoolScope::Instance { instance_id, .. } => pool_copy_guid(
            ctx.db
                .game_gameobject_pool_copy()
                .insert(GameObjectPoolCopy {
                    copy_id: 0,
                    instance_id,
                    point_id: member.point_id,
                })
                .copy_id,
        ),
    };
    // A replacement waits hidden and used, as a despawned object does.
    let hidden_until = match timing {
        PoolTiming::Now => None,
        PoolTiming::AfterRespawnDelay => Some(replacement_respawn_at(ctx, member, instance_id)),
    };
    GameObject {
        guid,
        template_entry: member.template_entry,
        map_id: member.map_id,
        x: member.x,
        y: member.y,
        z: member.z,
        orientation: member.orientation,
        state: u8::from(hidden_until.is_some()),
        created_at: crate::time::now(ctx),
        respawn_at_micros: hidden_until.unwrap_or(0),
        instance_id,
        grid_x: 0,
        grid_y: 0,
        cell: 0,
        rotation_0: member.rotation_0,
        rotation_1: member.rotation_1,
        rotation_2: member.rotation_2,
        rotation_3: member.rotation_3,
        respawn_min_secs: member.respawn_min_secs,
        respawn_max_secs: member.respawn_max_secs,
        despawned: hidden_until.is_some(),
    }
    .insert(ctx);
}

fn active_points(
    ctx: &ReducerContext,
    scope: PoolScope,
    members: &[GameObjectPoolMember],
) -> HashSet<u64> {
    members
        .iter()
        .filter(|m| scope.active_guid(ctx, m.point_id).is_some())
        .map(|m| m.point_id)
        .collect()
}

fn active_child_pools(
    ctx: &ReducerContext,
    scope: PoolScope,
    children: &[GameObjectPoolLink],
    left_out: Option<u32>,
    depth: u32,
) -> HashSet<u32> {
    children
        .iter()
        .filter(|child| {
            Some(child.pool_id) != left_out && pool_is_active(ctx, scope, child.pool_id, depth + 1)
        })
        .map(|child| child.pool_id)
        .collect()
}

/// Rolls the pool's members up to its limit (vmangos `PoolGroup::SpawnObject`); returns how many.
fn fill_members(
    ctx: &ReducerContext,
    scope: PoolScope,
    pool_id: u32,
    timing: PoolTiming,
    fence: Option<&MapFence>,
    depth: u32,
) -> u32 {
    let limit = pool_limit(ctx, pool_id) as usize;
    let members = pool_members(ctx, pool_id);
    let mut active = active_points(ctx, scope, &members);
    // Child pools and members share the limit, as vmangos `mSpawnedPools` counts both.
    let children_up = active_child_pools(ctx, scope, &child_pools(ctx, pool_id), None, depth).len();
    let mut brought_up = 0;
    while active.len() + children_up < limit {
        let candidates = eligible_pool_candidates(&members, &active, |map_id| {
            scope.admits_map(map_id) && fence.is_none_or(|fence| fence.admits(map_id))
        });
        let Some(chosen) = pick_weighted(ctx, &candidates, |m| m.weight) else {
            break;
        };
        activate_point(ctx, scope, chosen, timing);
        active.insert(chosen.point_id);
        brought_up += 1;
    }
    brought_up
}

/// Rolls child pools, never `left_out`, up to the pool's limit; returns whether any came up.
fn fill_children(
    ctx: &ReducerContext,
    scope: PoolScope,
    pool_id: u32,
    timing: PoolTiming,
    fence: Option<&MapFence>,
    left_out: Option<u32>,
    depth: u32,
) -> bool {
    let children = child_pools(ctx, pool_id);
    if children.is_empty() {
        return false;
    }
    let limit = pool_limit(ctx, pool_id) as usize;
    let members_up = active_points(ctx, scope, &pool_members(ctx, pool_id)).len();
    let mut active = active_child_pools(ctx, scope, &children, left_out, depth);
    let mut tried: HashSet<u32> = left_out.into_iter().collect();
    let mut brought_up = false;
    while active.len() + members_up < limit {
        let candidates: Vec<&GameObjectPoolLink> = children
            .iter()
            .filter(|child| {
                child.weight > 0
                    && !active.contains(&child.pool_id)
                    && !tried.contains(&child.pool_id)
            })
            .collect();
        let Some(chosen) = pick_weighted(ctx, &candidates, |child| child.weight) else {
            break;
        };
        let chosen = chosen.pool_id;
        tried.insert(chosen);
        if spawn_pool(ctx, scope, chosen, timing, fence, depth + 1) {
            active.insert(chosen);
            brought_up = true;
        }
    }
    brought_up
}

/// Brings up child pools, then members (vmangos `PoolManager::SpawnPool`); whether any came up.
fn spawn_pool(
    ctx: &ReducerContext,
    scope: PoolScope,
    pool_id: u32,
    timing: PoolTiming,
    fence: Option<&MapFence>,
    depth: u32,
) -> bool {
    if depth > MAX_POOL_DEPTH {
        return false;
    }
    let children = fill_children(ctx, scope, pool_id, timing, fence, None, depth);
    let members = fill_members(ctx, scope, pool_id, timing, fence, depth) > 0;
    children || members
}

/// Takes a used object out of its pool and rolls a hidden replacement; `false` outside a pool.
/// docs/schema.md#gameobject-pools gives the rule.
pub(crate) fn pool_member_used_up(ctx: &ReducerContext, guid: u64) -> bool {
    let Some((member, scope)) = pool_point_of(ctx, guid) else {
        return false;
    };
    despawn_point(ctx, scope, member.point_id);
    let parent = ctx
        .db
        .game_gameobject_pool_link()
        .pool_id()
        .find(member.pool_id)
        .map(|link| link.parent_pool_id)
        .filter(|&parent| parent != 0);
    // vmangos UpdatePool: a nested pool hands its place to a free sibling.
    let moved = parent.is_some_and(|parent| {
        fill_children(
            ctx,
            scope,
            parent,
            PoolTiming::AfterRespawnDelay,
            None,
            Some(member.pool_id),
            0,
        )
    });
    if moved {
        despawn_pool(ctx, scope, member.pool_id, 0);
    } else {
        // vmangos leaves a nested place empty with no sibling free: a Deliberate Deviation.
        fill_members(
            ctx,
            scope,
            member.pool_id,
            PoolTiming::AfterRespawnDelay,
            None,
            0,
        );
    }
    true
}

/// Deletes `gathered_guid` and rolls the pool's members up again at once.
#[cfg(feature = "debug_reducers")]
pub(crate) fn reroll_pool(ctx: &ReducerContext, pool_id: u32, gathered_guid: u64) {
    crate::GameObject::delete(ctx, gathered_guid);
    fill_members(ctx, PoolScope::World, pool_id, PoolTiming::Now, None, 0);
}

fn arm_world_root(ctx: &ReducerContext, pool_id: u32, fence: &MapFence) {
    despawn_pool(ctx, PoolScope::World, pool_id, 0);
    spawn_pool(
        ctx,
        PoolScope::World,
        pool_id,
        PoolTiming::Now,
        Some(fence),
        0,
    );
}

pub(crate) fn arm_pool(ctx: &ReducerContext, pool_id: u32) {
    arm_world_root(ctx, pool_id, &MapFence::load(ctx));
}

fn is_world_root(ctx: &ReducerContext, pool_id: u32) -> bool {
    ctx.db
        .game_gameobject_pool_link()
        .pool_id()
        .find(pool_id)
        .is_none_or(|link| link.parent_pool_id == 0)
}

// One roll call takes at most 250 pools; the importer groups them to bring up at most 250 objects.
const MAX_POOLS_PER_ROLL: usize = 250;

/// Rolls the listed top-level open-world pools afresh, as `;`-separated pool ids.
#[reducer]
pub fn arm_pools(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let pool_ids = packed
        .split(';')
        .filter(|id| !id.is_empty())
        .map(|id| id.parse::<u32>().map_err(|_| format!("bad pool id: {id}")))
        .collect::<Result<Vec<u32>, String>>()?;
    if pool_ids.len() > MAX_POOLS_PER_ROLL {
        return Err(format!(
            "{} pools are more than one call rolls ({MAX_POOLS_PER_ROLL})",
            pool_ids.len()
        ));
    }
    if let Some(pool_id) = pool_ids.iter().find(|&&id| !is_world_root(ctx, id)) {
        return Err(format!("pool {pool_id} is not a top-level pool"));
    }
    let fence = MapFence::instanceable(ctx);
    for pool_id in pool_ids {
        arm_world_root(ctx, pool_id, &fence);
    }
    Ok(())
}

/// Rolls every top-level open-world pool afresh; a Realm with more than one call's worth refuses.
#[reducer]
pub fn arm_all_pools(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let roots: Vec<u32> = ctx
        .db
        .game_gameobject_pool()
        .iter()
        .map(|pool| pool.pool_id)
        .filter(|&pool_id| is_world_root(ctx, pool_id))
        .collect();
    if roots.len() > MAX_POOLS_PER_ROLL {
        return Err(format!(
            "{} top-level pools are more than one call rolls; roll them with arm_pools in batches",
            roots.len()
        ));
    }
    let fence = MapFence::load(ctx);
    for pool_id in roots {
        arm_world_root(ctx, pool_id, &fence);
    }
    Ok(())
}

/// Rolls the top-level pools of `map_id` for a new Instance of that map.
pub(crate) fn arm_instance_pools(ctx: &ReducerContext, map_id: u32, instance_id: u64) {
    let scope = PoolScope::Instance {
        instance_id,
        map_id,
    };
    let roots: Vec<u32> = ctx
        .db
        .game_gameobject_pool_link()
        .by_map()
        .filter(&map_id)
        .filter(|link| link.parent_pool_id == 0)
        .map(|link| link.pool_id)
        .collect();
    for pool_id in roots {
        spawn_pool(ctx, scope, pool_id, PoolTiming::Now, None, 0);
    }
}

/// Forgets an Instance's pool objects; the caller deletes the objects themselves.
pub(crate) fn forget_instance_pools(ctx: &ReducerContext, instance_id: u64) {
    let copies = ctx.db.game_gameobject_pool_copy();
    let copy_ids: Vec<u64> = copies
        .by_instance_point()
        .filter(instance_id)
        .map(|copy| copy.copy_id)
        .collect();
    for copy_id in copy_ids {
        copies.copy_id().delete(copy_id);
    }
}

const RESPAWN_WINDOW_MICROS: u64 = 180_000_000;

pub(crate) fn respawn_window_micros(respawn_secs: u32) -> u64 {
    if respawn_secs == 0 {
        RESPAWN_WINDOW_MICROS
    } else {
        respawn_secs as u64 * 1_000_000
    }
}

// A rolled chest despawns 5 minutes after it opens or after a Loot Release that leaves loot, as
// vmangos DoLootRelease.
const PARTLY_LOOTED_CHEST_MICROS: u64 = 300_000_000;

fn now_micros(ctx: &ReducerContext) -> u64 {
    crate::time::now(ctx).to_micros_since_unix_epoch() as u64
}

/// A spawn's respawn delay in seconds: a `draw` from its own range, else `template_secs`.
/// vmangos GetRandomRespawnTime draws between `spawntimesecsmin` and `spawntimesecsmax`.
pub(crate) fn spawn_respawn_secs(
    min_secs: u32,
    max_secs: u32,
    template_secs: u32,
    draw: u32,
) -> u32 {
    if max_secs == 0 {
        return template_secs;
    }
    let low = min_secs.clamp(1, max_secs);
    let span = u64::from(max_secs - low) + 1;
    low + u32::try_from(u64::from(draw) % span).expect("a remainder below the span fits in u32")
}

/// Where a chest stands in its use, read from its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChestPhase {
    /// Closed, with no loot rolled.
    Ready,
    /// Loot rolled: open, or closed again by a Loot Release that left loot.
    Opened,
    /// Looted and hidden until it respawns.
    Despawned,
}

pub(crate) fn chest_phase(state: u8, respawn_at_micros: u64, despawned: bool) -> ChestPhase {
    if despawned {
        ChestPhase::Despawned
    } else if state == 0 && respawn_at_micros == 0 {
        ChestPhase::Ready
    } else {
        ChestPhase::Opened
    }
}

/// When a chest that despawns at `now` respawns. `None` for a chest that never respawns: one in an
/// Instance, or one whose respawn delay is 0 (vmangos `spawntimesecsmin` of 0 or less).
pub(crate) fn chest_respawn_at(now: u64, respawn_secs: u32, instance_id: u64) -> Option<u64> {
    (instance_id == 0 && respawn_secs != 0).then(|| now + u64::from(respawn_secs) * 1_000_000)
}

/// Records a Character the chest showed its loot to; the quest rows it needs keep the chest.
fn record_chest_opener(ctx: &ReducerContext, chest_guid: u64, opener_guid: u64) {
    let openers = ctx.db.game_corpse_loot_eligible();
    if !openers
        .by_corpse()
        .filter(&chest_guid)
        .any(|opener| opener.eligible_guid == opener_guid)
    {
        openers.insert(crate::CorpseLootEligible {
            id: 0,
            corpse_guid: chest_guid,
            eligible_guid: opener_guid,
        });
    }
}

/// vmangos `Loot::isLooted` is false while money or a row is left for the looter or for any earlier
/// opener, as `FillQuestLoot` counts each opener's quest items.
fn chest_holds_loot(ctx: &ReducerContext, chest_guid: u64, looter_guid: u64) -> bool {
    if crate::loot::purse(ctx, chest_guid) != 0 {
        return true;
    }
    let takers: Vec<u64> = ctx
        .db
        .game_corpse_loot_eligible()
        .by_corpse()
        .filter(&chest_guid)
        .map(|opener| opener.eligible_guid)
        .chain(std::iter::once(looter_guid))
        .collect();
    ctx.db
        .game_corpse_loot()
        .by_corpse()
        .filter(&chest_guid)
        .any(|row| takers.iter().any(|&taker| row.takeable_by(ctx, taker)))
}

fn other_loot_window_open(ctx: &ReducerContext, go_guid: u64, character_guid: u64) -> bool {
    ctx.db
        .game_native_player()
        .by_open_loot()
        .filter(go_guid)
        .any(|player| player.character_guid != character_guid)
}

// A despawned chest leaves no Loot Window open on it.
fn close_loot_windows(ctx: &ReducerContext, go_guid: u64) {
    let players = ctx.db.game_native_player();
    let open: Vec<_> = players.by_open_loot().filter(go_guid).collect();
    for mut player in open {
        player.open_loot = 0;
        players.connection_id().update(player);
    }
}

/// Drops a looted chest's loot and hides it until its respawn, as vmangos GO_JUST_DEACTIVATED.
fn despawn_chest(ctx: &ReducerContext, mut chest: GameObject, template: &GameObjectTemplate) {
    crate::loot::reap_corpse_loot_family(ctx, chest.guid);
    close_loot_windows(ctx, chest.guid);
    let delay = spawn_respawn_secs(
        chest.respawn_min_secs,
        chest.respawn_max_secs,
        template.respawn_secs,
        ctx.random(),
    );
    let respawn_at = chest_respawn_at(now_micros(ctx), delay, chest.instance_id);
    if respawn_at.is_some() && pool_member_used_up(ctx, chest.guid) {
        return;
    }
    chest.despawned = true;
    chest.respawn_at_micros = respawn_at.unwrap_or(0);
    chest.update(ctx);
}

/// A Loot Release on an opened chest: an emptied chest despawns, and one with loot left closes and
/// despawns 5 minutes later.
pub(crate) fn chest_loot_released(ctx: &ReducerContext, looter_guid: u64, go_guid: u64) {
    let Some(mut chest) = ctx.db.game_gameobject().guid().find(go_guid) else {
        return;
    };
    let Some(template) = ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(chest.template_entry)
    else {
        return;
    };
    if template.type_id != go_type::CHEST
        || chest_phase(chest.state, chest.respawn_at_micros, chest.despawned) != ChestPhase::Opened
    {
        return;
    }
    if chest_holds_loot(ctx, go_guid, looter_guid) {
        chest.state = 0;
        chest.respawn_at_micros = now_micros(ctx) + PARTLY_LOOTED_CHEST_MICROS;
        chest.update(ctx);
    } else {
        despawn_chest(ctx, chest, &template);
    }
}

pub(crate) fn loot_window_open(ctx: &ReducerContext, go_guid: u64) -> bool {
    ctx.db
        .game_native_player()
        .by_open_loot()
        .filter(go_guid)
        .next()
        .is_some()
}

/// Runs an object's elapsed respawn timer: an opened chest despawns once no Loot Window is open on
/// it, and a despawned or used object respawns.
pub(crate) fn respawn_timer_elapsed(ctx: &ReducerContext, go_guid: u64) {
    let Some(object) = ctx.db.game_gameobject().guid().find(go_guid) else {
        return;
    };
    if let Some(template) = ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(object.template_entry)
        .filter(|template| template.type_id == go_type::CHEST)
    {
        if chest_phase(object.state, object.respawn_at_micros, object.despawned)
            == ChestPhase::Opened
        {
            if !loot_window_open(ctx, go_guid) {
                despawn_chest(ctx, object, &template);
            }
            return;
        }
    }
    if object.despawned || object.state == 1 {
        respawn(ctx, object);
    }
}

/// Shows the object again closed and unused.
pub(crate) fn respawn(ctx: &ReducerContext, mut object: GameObject) {
    crate::loot::reap_corpse_loot_family(ctx, object.guid);
    ctx.db
        .game_gameobject_unlocked()
        .go_guid()
        .delete(object.guid);
    object.state = 0;
    object.despawned = false;
    object.respawn_at_micros = 0;
    object.update(ctx);
}

/// Stamps an opened or despawned world chest that has no respawn time, so the respawn pass can
/// reach it. Returns the number stamped. Idempotent: a stamped chest no longer matches.
pub(crate) fn stamp_opened_chests(ctx: &ReducerContext) -> u64 {
    let chest_entries: Vec<u32> = ctx
        .db
        .game_gameobject_template()
        .iter()
        .filter(|template| template.type_id == go_type::CHEST && template.respawn_secs != 0)
        .map(|template| template.entry)
        .collect();
    let now = now_micros(ctx);
    let mut stamped = 0;
    for entry in chest_entries {
        let opened: Vec<GameObject> = ctx
            .db
            .game_gameobject()
            .by_template_scope()
            .filter(entry)
            .filter(|chest| {
                chest.respawn_at_micros == 0
                    && chest.instance_id == 0
                    && chest_phase(chest.state, chest.respawn_at_micros, chest.despawned)
                        != ChestPhase::Ready
            })
            .collect();
        for mut chest in opened {
            chest.respawn_at_micros = now;
            chest.update(ctx);
            stamped += 1;
        }
    }
    stamped
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
        .filter(|go| !go.despawned)
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
    if dx * dx + dy * dy + dz * dz > crate::npc_interaction::INTERACTION_RANGE_SQ {
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
            let current = crate::skill::skill_value(ctx, caster_guid, row.property, false);
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
        go_type::CHEST => use_chest(ctx, caster_guid, go, &tmpl)?,
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
            let learned = crate::skill::skill_value(ctx, caster_guid, line, false);
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
            if !pool_member_used_up(ctx, go.guid) {
                let delay = spawn_respawn_secs(
                    go.respawn_min_secs,
                    go.respawn_max_secs,
                    tmpl.respawn_secs,
                    ctx.random(),
                );
                go.state = 1;
                go.despawned = true;
                go.respawn_at_micros = now_micros(ctx) + respawn_window_micros(delay);
                go.update(ctx);
            }
        }
        go_type::DOOR | go_type::BUTTON => {
            if locked_shut(ctx, go.guid, tmpl.lock_id) {
                return Err("it is locked".to_string());
            }
            go.state = toggle_state(go.state);
            go.update(ctx);
        }
        go_type::SUMMONING_RITUAL => crate::summoning_ritual::assist(ctx, caster_guid, go)?,
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

fn use_chest(
    ctx: &ReducerContext,
    caster_guid: u64,
    mut chest: GameObject,
    template: &GameObjectTemplate,
) -> Result<(), String> {
    if locked_shut(ctx, chest.guid, template.lock_id) {
        return Err("it is locked".to_string());
    }
    match chest_phase(chest.state, chest.respawn_at_micros, chest.despawned) {
        ChestPhase::Despawned => return Err("chest already looted".to_string()),
        ChestPhase::Ready => {
            roll_chest_loot(ctx, chest.guid, template);
            chest.respawn_at_micros = now_micros(ctx) + PARTLY_LOOTED_CHEST_MICROS;
        }
        // vmangos SendLoot shows an activated chest's remaining loot to any opener.
        ChestPhase::Opened => {}
    }
    record_chest_opener(ctx, chest.guid, caster_guid);
    chest.state = 1;
    if chest_holds_loot(ctx, chest.guid, caster_guid)
        || other_loot_window_open(ctx, chest.guid, caster_guid)
    {
        chest.update(ctx);
    } else {
        despawn_chest(ctx, chest, template);
    }
    Ok(())
}

fn roll_chest_loot(ctx: &ReducerContext, chest_guid: u64, template: &GameObjectTemplate) {
    crate::loot::reap_corpse_loot_family(ctx, chest_guid);
    let rows: Vec<crate::loot::LootRow> = if template.data1 != 0 {
        ctx.db
            .game_gameobject_loot()
            .by_loot()
            .filter(&template.data1)
            .map(crate::loot::LootRow::from)
            .collect()
    } else {
        Vec::new()
    };
    let mut winners = crate::loot::roll_loot_rows(ctx, chest_guid, rows);
    if winners.is_empty() && template.data0 != 0 {
        winners.push(crate::loot::LootDrop {
            item_entry: template.data0,
            count: 1,
            quest_only: false,
        });
    }
    crate::loot::insert_corpse_rows(
        ctx,
        chest_guid,
        winners,
        &[],
        crate::loot::UnneededQuestDrop::KeepShared,
    );
    crate::loot::stock_purse(ctx, chest_guid, 0, (template.money_min, template.money_max));
}

pub(crate) fn despawn_from_relay(ctx: &ReducerContext, go_guid: u64) -> Result<(), String> {
    if ctx.db.game_gameobject().guid().find(go_guid).is_none() {
        return Err(format!("relay gameobject {go_guid} is missing"));
    }
    crate::loot::reap_corpse_loot_family(ctx, go_guid);
    close_loot_windows(ctx, go_guid);
    ctx.db.game_gameobject_unlocked().go_guid().delete(go_guid);
    ctx.db
        .game_gameobject_trap_cooldown()
        .go_guid()
        .delete(go_guid);
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
        crate::GameObject::delete(ctx, g);
    }
    let unlocked = ctx.db.game_gameobject_unlocked();
    let picked: Vec<u64> = unlocked.iter().map(|row| row.go_guid).collect();
    for go_guid in picked {
        unlocked.go_guid().delete(go_guid);
    }
    let pool_copies = ctx.db.game_gameobject_pool_copy();
    let copy_ids: Vec<u64> = pool_copies.iter().map(|copy| copy.copy_id).collect();
    for copy_id in copy_ids {
        pool_copies.copy_id().delete(copy_id);
    }
    // A slice whose every placement is pooled sends an empty first batch to clear the old objects.
    if load_go_batch(ctx, &packed)? == 0 && ctx.db.game_gameobject_pool_member().count() == 0 {
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
        if f.len() != 12 && f.len() != 14 {
            return Err(format!(
                "gameobject import row needs 12 or 14 fields, got {}: {row}",
                f.len()
            ));
        }
        let pu64 = |s: &str| s.parse::<u64>().map_err(|_| format!("bad u64: {s}"));
        let pu32 = |s: &str| s.parse::<u32>().map_err(|_| format!("bad u32: {s}"));
        let pf32 = |s: &str| s.parse::<f32>().map_err(|_| format!("bad f32: {s}"));
        let pu8 = |s: &str| s.parse::<u8>().map_err(|_| format!("bad u8 state: {s}"));
        let (respawn_min_secs, respawn_max_secs) = match f.get(12..14) {
            Some([min, max]) => (pu32(min)?, pu32(max)?),
            _ => (0, 0),
        };

        GameObject {
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
            grid_x: 0,
            grid_y: 0,
            cell: 0,
            rotation_0: pf32(f[8])?,
            rotation_1: pf32(f[9])?,
            rotation_2: pf32(f[10])?,
            rotation_3: pf32(f[11])?,
            respawn_min_secs,
            respawn_max_secs,
            despawned: false,
        }
        .try_insert(ctx)
        .map_err(|e| format!("gameobject insert failed (dup guid?): {e}"))?;
        loaded += 1;
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_derives_the_grid_cell_from_the_position() {
        let mut object = GameObject {
            guid: 1,
            template_entry: 0,
            map_id: 1,
            x: 2_133.0,
            y: -4_477.0,
            z: 0.0,
            orientation: 0.0,
            state: 0,
            created_at: Timestamp::UNIX_EPOCH,
            respawn_at_micros: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            cell: 0,
            rotation_0: 0.0,
            rotation_1: 0.0,
            rotation_2: 0.0,
            rotation_3: 0.0,
            respawn_min_secs: 0,
            respawn_max_secs: 0,
            despawned: false,
        };
        object.stamp_cell();
        let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(2_133.0, -4_477.0);
        assert_eq!((object.grid_x, object.grid_y), (grid_x, grid_y));
        assert_eq!(
            object.cell,
            lyracore_shared::spatial::cell_id_at(2_133.0, -4_477.0)
        );
        assert_ne!(object.cell, 0, "a real position is not cell (0, 0)");
    }

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
    fn a_despawned_chest_respawns_its_delay_later() {
        assert_eq!(chest_respawn_at(1_000, 600, 0), Some(600_001_000));
    }

    #[test]
    fn a_chest_without_a_respawn_time_never_respawns() {
        assert_eq!(chest_respawn_at(1_000, 0, 0), None);
    }

    #[test]
    fn a_chest_in_an_instance_never_respawns() {
        assert_eq!(chest_respawn_at(1_000, 600, 7), None);
    }

    #[test]
    fn a_closed_chest_without_a_timer_is_ready_and_a_rolled_one_is_opened_until_it_despawns() {
        assert_eq!(chest_phase(0, 0, false), ChestPhase::Ready);
        assert_eq!(chest_phase(1, 900_001_000, false), ChestPhase::Opened);
        assert_eq!(chest_phase(0, 300_001_000, false), ChestPhase::Opened);
        assert_eq!(chest_phase(1, 0, false), ChestPhase::Opened);
        assert_eq!(chest_phase(1, 600_001_000, true), ChestPhase::Despawned);
        assert_eq!(chest_phase(1, 0, true), ChestPhase::Despawned);
    }

    #[test]
    fn a_spawn_without_its_own_range_takes_its_template_respawn_time() {
        assert_eq!(spawn_respawn_secs(0, 0, 600, 12_345), 600);
        assert_eq!(spawn_respawn_secs(0, 0, 0, 12_345), 0);
    }

    #[test]
    fn a_spawn_draws_its_respawn_time_from_its_own_range_inclusive() {
        assert_eq!(spawn_respawn_secs(300, 600, 7_200, 0), 300);
        assert_eq!(spawn_respawn_secs(300, 600, 7_200, 300), 600);
        assert_eq!(spawn_respawn_secs(300, 600, 7_200, 301), 300);
        assert_eq!(spawn_respawn_secs(120, 120, 7_200, u32::MAX), 120);
    }

    #[test]
    fn a_spawn_range_never_draws_zero_or_beyond_its_maximum() {
        assert_eq!(spawn_respawn_secs(0, 60, 7_200, 0), 1);
        assert_eq!(spawn_respawn_secs(900, 60, 7_200, 5), 60);
        assert_eq!(spawn_respawn_secs(1, u32::MAX, 0, u32::MAX - 1), u32::MAX);
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
                rotation_0: 0.0,
                rotation_1: 0.0,
                rotation_2: 0.0,
                rotation_3: 0.0,
                respawn_min_secs: 0,
                respawn_max_secs: 0,
            }
        }
        let live_map: HashSet<u32> = [1].into_iter().collect();
        let members = vec![
            member(1, 1, 5),
            member(2, 1, 0),
            member(3, 0, 5),
            member(4, 1, 5),
        ];
        let active_points: HashSet<u64> = [4].into_iter().collect();
        let out = eligible_pool_candidates(&members, &active_points, |map_id| {
            pool_member_map_is_live(map_id, &live_map)
        });
        let ids: Vec<u64> = out.iter().map(|m| m.point_id).collect();
        assert_eq!(
            ids,
            vec![1],
            "only the live-map, weighted, inactive member survives all three gates"
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
        for point_id in [1u64, 2, 3, 4, 42, 9999] {
            let guid = pool_point_guid(point_id);
            assert!(is_gameobject_guid(guid));
            assert_ne!(guid & POOL_TAG, 0, "tagged as a pool point");
            assert_eq!(
                pool_point_id_of(guid),
                Some(point_id),
                "round-trips to point_id"
            );
        }
        for standalone in [3, 4, 50102, 1234].map(gameobject_guid) {
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
    fn gather_deplete_routes_pooled_to_its_pool_and_standalone_to_in_place() {
        let pooled = pool_point_guid(1_000_000);
        assert_eq!(
            pool_point_id_of(pooled),
            Some(1_000_000),
            "pooled point → its pool rolls the replacement"
        );
        let standalone = gameobject_guid(50102);
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

    #[test]
    fn go_copy_band_is_disjoint_from_pool_static_and_debug_namespaces() {
        let copy_guid = gameobject_guid(GO_COPY_BAND | 1);
        const { assert!(DUEL_FLAG_BAND < GO_COPY_BAND && GO_COPY_BAND < POOL_TAG) };
        assert_eq!(pool_point_id_of(copy_guid), None);
        const { assert!(GO_COPY_BAND > 0xFF_FFFF) };
        assert_ne!(copy_guid, pool_point_guid(1));
    }

    #[test]
    fn a_pool_members_instance_copy_guid_is_disjoint_from_points_and_plain_copies() {
        let pool_copy = pool_copy_guid(7);
        assert_eq!(pool_copy_id_of(pool_copy), Some(7));
        assert_eq!(pool_point_id_of(pool_copy), None);
        assert_eq!(pool_copy_id_of(pool_point_guid(7)), None);
        assert_eq!(pool_copy_id_of(gameobject_guid(GO_COPY_BAND | 7)), None);
        assert_ne!(pool_copy, gameobject_guid(GO_COPY_BAND | 7));
    }

    #[test]
    fn an_open_world_point_never_reaches_the_instance_copy_band() {
        let point = (1 << 40) | 3_998_642;
        assert_eq!(pool_point_id_of(pool_point_guid(point)), Some(point));
        assert_eq!(pool_point_guid(point) & GO_COPY_BAND, 0);
    }
}
