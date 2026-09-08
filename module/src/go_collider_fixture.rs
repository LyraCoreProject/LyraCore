//! Synthetic durable verification. Run only on an isolated fixture Shard.

use crate::game_world_entity;
use crate::gameobject::game_gameobject;
use crate::go_collider::game_go_collider;
use crate::go_model::game_go_model;
use lyracore_shared::vmap::{encode, TriClass, VmapTri};
use spacetimedb::{reducer, ReducerContext, Table};

const MAP: u32 = 36;
const ENTRY: u32 = 509_113;
// The ray misses the spawn origin, so an incorrectly centered tiny radius would discard it.
const A: [f32; 3] = [-4.0, 0.5, 0.0];
const B: [f32; 3] = [4.0, 0.5, 0.0];

fn require(condition: bool, reason: &str) -> Result<(), String> {
    condition.then_some(()).ok_or_else(|| reason.to_string())
}

fn rays(ctx: &ReducerContext, instance: u64, expected_x: Option<f32>) -> Result<(), String> {
    for hit in [
        crate::vmap::los_ray(ctx, MAP, instance, A, B),
        crate::vmap::collision_ray(ctx, MAP, instance, A, B),
    ] {
        require(
            match (hit, expected_x) {
                (None, None) => true,
                (Some(hit), Some(x)) => (hit[0] - x).abs() < 0.001,
                _ => false,
            },
            &format!("instance {instance}: expected {expected_x:?}, got {hit:?}"),
        )?;
    }
    Ok(())
}

fn door(ctx: &ReducerContext, instance: u64) -> Result<crate::gameobject::GameObject, String> {
    let row = ctx
        .db
        .game_go_collider()
        .by_partition()
        .filter((MAP, instance))
        .find(|r| r.entry == ENTRY)
        .ok_or_else(|| format!("missing collider in {instance}"))?;
    ctx.db
        .game_gameobject()
        .guid()
        .find(row.go_guid)
        .ok_or_else(|| "missing door".to_string())
}

fn blink(ctx: &ReducerContext, instance: u64, expected_x: f32) -> Result<(), String> {
    crate::debug::debug_spawn_player_entity(ctx, 1)?;
    let mut caster = ctx
        .db
        .game_world_entity()
        .guid()
        .find(1)
        .ok_or("missing fixture Character")?;
    caster.map_id = MAP;
    caster.instance_id = instance;
    caster.x = A[0];
    caster.y = A[1];
    caster.z = A[2];
    caster.orientation = 0.0;
    (caster.grid_x, caster.grid_y) = lyracore_shared::spatial::grid_cell(caster.x, caster.y);
    caster.cell = lyracore_shared::spatial::cell_id_at(caster.x, caster.y);
    ctx.db.game_world_entity().guid().update(caster);
    crate::spell::blink_forward(ctx, 1, 8.0);
    let landed = ctx
        .db
        .game_world_entity()
        .guid()
        .find(1)
        .ok_or("Blink removed the Character")?;
    require(
        landed.instance_id == instance && (landed.x - expected_x).abs() < 0.001,
        "Blink crossed the closed door or lost instance identity",
    )?;
    ctx.db.game_world_entity().guid().delete(1);
    Ok(())
}

fn package_claim(ctx: &ReducerContext, claim: &str) -> Result<(), String> {
    let packed = format!(
        r#"{{"version":1,"package":"fixture.collision","source_hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","claims":[{claim}]}}"#
    );
    crate::package_import::apply_package_deltas(ctx, "gameobjects".to_string(), packed)
}

fn package_lifecycle(ctx: &ReducerContext) -> Result<(), String> {
    let spawn_key = format!(
        r#""table":"game_gameobject","key":{{"map_id":{MAP},"spawn_id":{ENTRY}}},"operation":"update""#
    );
    package_claim(
        ctx,
        &format!(r#"{{{spawn_key},"fields":{{"x":{{"type":"f32","value":40.0}}}}}}"#),
    )?;
    rays(ctx, 0, None)?;
    for hit in [
        crate::vmap::los_ray(ctx, MAP, 0, [36.0, 0.5, 0.0], [44.0, 0.5, 0.0]),
        crate::vmap::collision_ray(ctx, MAP, 0, [36.0, 0.5, 0.0], [44.0, 0.5, 0.0]),
    ] {
        require(
            hit.is_some_and(|p| (p[0] - 41.0).abs() < 0.001),
            "Package move left the collider at its old position",
        )?;
    }
    package_claim(
        ctx,
        &format!(r#"{{{spawn_key},"fields":{{"x":{{"type":"f32","value":0.0}}}}}}"#),
    )?;
    for (type_id, expected) in [(3, None), (0, Some(1.0))] {
        package_claim(
            ctx,
            &format!(
                r#"{{"table":"game_gameobject_template","key":{{"entry":{ENTRY}}},"operation":"update","fields":{{"type_id":{{"type":"u8","value":{type_id}}}}}}}"#
            ),
        )?;
        rays(ctx, 0, expected)?;
    }
    let inserted = format!(
        r#"{{"table":"game_gameobject","key":{{"map_id":{MAP},"spawn_id":16000113}},"operation":"insert","fields":{{"template_entry":{{"type":"u32","value":{ENTRY}}},"x":{{"type":"f32","value":-2.0}},"y":{{"type":"f32","value":0.0}},"z":{{"type":"f32","value":0.0}},"orientation":{{"type":"f32","value":0.0}},"state":{{"type":"u8","value":0}},"rotation_0":{{"type":"f32","value":0.0}},"rotation_1":{{"type":"f32","value":0.0}},"rotation_2":{{"type":"f32","value":0.0}},"rotation_3":{{"type":"f32","value":0.0}}}}}}"#
    );
    for _ in 0..2 {
        package_claim(ctx, &inserted)?;
        rays(ctx, 0, Some(-1.0))?;
        require(
            ctx.db
                .game_go_collider()
                .by_partition()
                .filter((MAP, 0u64))
                .count()
                == 2,
            "Package replay duplicated a collider",
        )?;
    }
    crate::package_import::apply_package_deltas(ctx, "gameobjects".to_string(), String::new())?;
    rays(ctx, 0, Some(1.0))?;
    require(
        ctx.db
            .game_go_collider()
            .by_partition()
            .filter((MAP, 0u64))
            .count()
            == 1,
        "Package clear retained a collider",
    )
}

fn packed_model(entry: u32, scale: f32) -> String {
    // Off-center local geometry exposes an incorrect spawn-centered use of an AABB radius.
    let tris = [VmapTri {
        verts: [[1.0, -2.0, -2.0], [1.0, 2.0, -2.0], [1.0, 0.0, 2.0]],
        class: TriClass::M2,
    }];
    let blob = encode(&tris);
    let hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();
    format!("{entry},{scale},0.01,{hex}")
}

#[reducer]
pub fn debug_assert_go_collision(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    require(
        !crate::vmap::vmap_enabled(ctx, MAP),
        "fixture requires no active static generation on map 36",
    )?;
    crate::debug::debug_set_vmap_enabled(ctx, true)?;
    crate::go_model::import_go_models(ctx, packed_model(ENTRY, 1.0))?;
    crate::debug::debug_spawn_gameobject(ctx, ENTRY, 0, 0, 0, MAP, 0.0, 0.0, 0.0, 0, 0, 0, 0)?;
    rays(ctx, 0, Some(1.0))?;
    crate::go_collider::rebuild_go_colliders(ctx)?;
    crate::go_collider::rebuild_go_colliders(ctx)?;
    require(
        ctx.db
            .game_go_collider()
            .by_partition()
            .filter((MAP, 0u64))
            .count()
            == 1,
        "rebuild duplicated the collider",
    )?;
    require(
        !crate::nav::has_los(ctx, MAP, 0, (-4.0, 0.0, 0.0), (4.0, 0.0, 0.0)),
        "nav sight bypassed dynamic geometry",
    )?;
    let step = crate::nav::nav_step(ctx, MAP, 0, (-4.0, 0.0), (4.0, 0.0), 8.0, 0.0, 0.0);
    require(
        step.0.abs() < 0.001,
        "movement did not stop one yard before the door",
    )?;
    crate::debug::debug_set_vmap_enabled(ctx, false)?;
    rays(ctx, 0, None)?;
    crate::debug::debug_set_vmap_enabled(ctx, true)?;

    let nearer_guid = (0xF110u64 << 48) | u64::from(ENTRY + 99);
    crate::gameobject::import_gameobjects_append(
        ctx,
        format!("{nearer_guid},{ENTRY},{MAP},-2,0,0,0,0,0,0,0,0"),
    )?;
    rays(ctx, 0, Some(-1.0))?;
    for hit in [
        crate::vmap::los_ray(ctx, MAP, 0, B, A),
        crate::vmap::collision_ray(ctx, MAP, 0, B, A),
    ] {
        require(
            hit.is_some_and(|p| (p[0] - 1.0).abs() < 0.001),
            "dynamic rows did not select their nearest hit",
        )?;
    }
    crate::gameobject::despawn_from_relay(ctx, nearer_guid)?;

    package_lifecycle(ctx)?;
    let first = crate::instance::create_instance(ctx, MAP, 0)?;
    let sibling = crate::instance::create_instance(ctx, MAP, 0)?;
    rays(ctx, first, Some(1.0))?;
    rays(ctx, sibling, Some(1.0))?;
    require(
        crate::vmap::los_ray(ctx, 1, first, A, B).is_none(),
        "map isolation failed",
    )?;
    blink(ctx, first, 0.0)?;
    crate::encounter::open_door(ctx, ENTRY, first)?;
    blink(ctx, first, 4.0)?;
    rays(ctx, first, None)?;
    rays(ctx, sibling, Some(1.0))?;
    rays(ctx, 0, Some(1.0))?;
    let mut closed = door(ctx, first)?;
    closed.state = 0;
    ctx.db.game_gameobject().guid().update(closed);
    rays(ctx, first, Some(1.0))?;

    // Static and dynamic hits compete by distance, independent of insertion order.
    {
        use crate::vmap::{
            game_vmap_generation, game_vmap_generation_chunk, VmapGeneration, VmapGenerationChunk,
        };
        let generation_id = 509_113;
        ctx.db.game_vmap_generation().insert(VmapGeneration {
            id: generation_id,
            map_id: MAP,
            state: 2,
            expected_chunks: 1,
            accepted_chunks: 1,
            expected_bytes: 0,
            manifest_digest: vec![],
            source_identity: "synthetic".to_string(),
            selection_identity: "synthetic".to_string(),
        });
        let cx = lyracore_shared::terrain::cell_index(-1.0).ok_or("missing cell")?;
        let cy = lyracore_shared::terrain::cell_index(A[1]).ok_or("missing cell")?;
        let chunk = ctx
            .db
            .game_vmap_generation_chunk()
            .insert(VmapGenerationChunk {
                id: 0,
                generation_id,
                shard_ordinal: 0,
                key: lyracore_shared::terrain::cell_key(MAP, cx, cy),
                map_id: MAP,
                cell_x: cx,
                cell_y: cy,
                blob: encode(&[VmapTri {
                    verts: [[-1.0, -2.0, -2.0], [-1.0, 2.0, -2.0], [-1.0, 0.0, 2.0]],
                    class: TriClass::Wmo {
                        group_id: 0,
                        mogp_flags: 0,
                    },
                }]),
            });
        rays(ctx, first, Some(-1.0))?;
        for hit in [
            crate::vmap::los_ray(ctx, MAP, first, B, A),
            crate::vmap::collision_ray(ctx, MAP, first, B, A),
        ] {
            require(
                hit.is_some_and(|p| (p[0] - 1.0).abs() < 0.001),
                "reverse ray failed to choose dynamic hit before static wall",
            )?;
        }
        ctx.db.game_vmap_generation_chunk().id().delete(chunk.id);
        ctx.db.game_vmap_generation().id().delete(generation_id);
    }

    // Model replacement refreshes every registered instance and its broad-phase bound.
    crate::go_model::import_go_models_append(ctx, packed_model(ENTRY, 2.0))?;
    rays(ctx, first, Some(2.0))?;
    rays(ctx, sibling, Some(2.0))?;
    let removed = door(ctx, first)?.guid;
    crate::gameobject::despawn_from_relay(ctx, removed)?;
    rays(ctx, first, None)?;
    require(
        ctx.db.game_go_collider().go_guid().find(removed).is_none(),
        "despawn retained collider",
    )?;
    let sibling_guid = door(ctx, sibling)?.guid;
    crate::instance::teardown_instance(ctx, sibling);
    rays(ctx, sibling, None)?;
    require(
        ctx.db
            .game_go_collider()
            .go_guid()
            .find(sibling_guid)
            .is_none(),
        "instance reap retained collider",
    )?;
    crate::instance::teardown_instance(ctx, first);

    // The durable ray uses the spawn's current yaw/quaternion, translation and model scale.
    let mut transformed = door(ctx, 0)?;
    transformed.x = 10.0;
    transformed.y = 20.0;
    transformed.z = 30.0;
    transformed.orientation = std::f32::consts::FRAC_PI_2;
    crate::go_collider::register(ctx, &transformed);
    ctx.db.game_gameobject().guid().update(transformed);
    for hit in [
        crate::vmap::los_ray(ctx, MAP, 0, [10.0, 16.0, 30.0], [10.0, 24.0, 30.0]),
        crate::vmap::collision_ray(ctx, MAP, 0, [10.0, 16.0, 30.0], [10.0, 24.0, 30.0]),
    ] {
        require(
            hit.is_some_and(|p| (p[1] - 22.0).abs() < 0.001),
            "translated yaw and scale did not place the door at y=22",
        )?;
    }
    let mut transformed = door(ctx, 0)?;
    transformed.rotation_1 = 1.0;
    transformed.rotation_3 = 1.0;
    ctx.db.game_gameobject().guid().update(transformed);
    require(
        crate::vmap::floor_z(ctx, MAP, 0, 10.0, 20.0, 30.0)
            .is_some_and(|z| (z - 28.0).abs() < 0.001),
        "quaternion floor probe missed scaled door",
    )?;

    // A stale registry row cannot resurrect a deleted GameObject.
    let source = door(ctx, 0)?.guid;
    ctx.db.game_gameobject().guid().delete(source);
    for hit in [
        crate::vmap::los_ray(ctx, MAP, 0, [10.0, 20.0, 32.0], [10.0, 20.0, 24.0]),
        crate::vmap::collision_ray(ctx, MAP, 0, [10.0, 20.0, 32.0], [10.0, 20.0, 24.0]),
    ] {
        require(hit.is_none(), "an orphan collider still blocked the ray")?;
    }
    crate::go_collider::rebuild_go_colliders(ctx)?;
    require(
        ctx.db.game_go_collider().go_guid().find(source).is_none(),
        "rebuild retained orphan",
    )?;

    // Both supported types register; an unrelated prop with a model does not.
    for (entry, type_id, expected) in [(ENTRY + 1, 1, true), (ENTRY + 2, 3, false)] {
        crate::debug::debug_spawn_gameobject(
            ctx, entry, type_id, 0, 0, MAP, 0.0, 0.0, 0.0, 0, 0, 0, 0,
        )?;
        crate::go_model::import_go_models_append(ctx, packed_model(entry, 1.0))?;
        let guid = (0xF110u64 << 48) | u64::from(entry);
        require(
            ctx.db.game_go_collider().go_guid().find(guid).is_some() == expected,
            "template type gate failed",
        )?;
        crate::gameobject::despawn_from_relay(ctx, guid)?;
        ctx.db.game_go_model().entry().delete(entry);
    }
    let imported_guid = (0xF110u64 << 48) | u64::from(ENTRY);
    crate::gameobject::import_gameobjects(
        ctx,
        format!("{imported_guid},{ENTRY},{MAP},0,0,0,0,0,0,0,0,0"),
    )?;
    rays(ctx, 0, Some(2.0))?;
    crate::gameobject::despawn_from_relay(ctx, imported_guid)?;
    ctx.db.game_go_model().entry().delete(ENTRY);
    Ok(())
}
