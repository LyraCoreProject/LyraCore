//! Closed DOOR and BUTTON collision, scoped to one map and instance.

use crate::gameobject::{game_gameobject, game_gameobject_template, GameObject};
use crate::go_model::game_go_model;
use lyracore_shared::vmap::{decode, segment_tri_hit};
use spacetimedb::{reducer, table, ReducerContext, Table};

#[table(accessor = game_go_collider, index(accessor = by_partition, btree(columns = [map_id, instance_id])))]
pub struct GoCollider {
    #[primary_key]
    pub go_guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub entry: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub radius_world: f32,
}

/// Replace derived registration from the current spawn and model. Open doors remain registered.
pub(crate) fn register(ctx: &ReducerContext, go: &GameObject) {
    remove(ctx, go.guid);
    let Some(template) = ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(go.template_entry)
    else {
        return;
    };
    if !matches!(template.type_id, 0 | 1) {
        return;
    }
    let Some(model) = ctx.db.game_go_model().entry().find(go.template_entry) else {
        return;
    };
    // Older imports stored an AABB-centered radius without its center. Derive the origin
    // bound from vertices so off-center meshes cannot miss the broad phase.
    let Ok(tris) = decode(&model.blob) else {
        return;
    };
    let radius_world = tris
        .iter()
        .flat_map(|t| t.verts)
        .map(|v| dot(v, v).sqrt())
        .fold(0.0_f32, f32::max)
        * model.scale;
    if !radius_world.is_finite()
        || model.scale <= 0.0
        || radius_world < 0.0
        || ![go.x, go.y, go.z].iter().all(|v| v.is_finite())
    {
        return;
    }
    ctx.db.game_go_collider().insert(GoCollider {
        go_guid: go.guid,
        map_id: go.map_id,
        instance_id: go.instance_id,
        entry: go.template_entry,
        x: go.x,
        y: go.y,
        z: go.z,
        radius_world,
    });
}

pub(crate) fn remove(ctx: &ReducerContext, guid: u64) {
    ctx.db.game_go_collider().go_guid().delete(guid);
}

/// Operator reconciliation covers every partition hosted by this Shard.
#[reducer]
pub fn rebuild_go_colliders(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let guids: Vec<_> = ctx
        .db
        .game_go_collider()
        .iter()
        .map(|c| c.go_guid)
        .collect();
    for guid in guids {
        remove(ctx, guid);
    }
    for go in ctx.db.game_gameobject().iter() {
        register(ctx, &go);
    }
    Ok(())
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Conservative broad phase, including tangency and a point segment inside the sphere.
fn overlaps_sphere(a: [f32; 3], b: [f32; 3], center: [f32; 3], radius: f32) -> bool {
    let dir = sub(b, a);
    let len_sq = dot(dir, dir);
    let t = if len_sq > 0.0 {
        (dot(sub(center, a), dir) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let nearest = std::array::from_fn(|i| a[i] + t * dir[i]);
    let offset = sub(nearest, center);
    dot(offset, offset) <= radius * radius + 0.0001
}

/// Inverse of scale, normalized spawn quaternion and translation. Zero quaternion means yaw.
fn local_point(
    point: [f32; 3],
    position: [f32; 3],
    rotation: [f32; 4],
    yaw: f32,
    scale: f32,
) -> Option<[f32; 3]> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let q = if rotation == [0.0; 4] {
        [0.0, 0.0, (yaw * 0.5).sin(), (yaw * 0.5).cos()]
    } else {
        rotation
    };
    let norm = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return None;
    }
    let v = sub(point, position);
    let axis = [-q[0] / norm, -q[1] / norm, -q[2] / norm];
    let uv = cross(axis, v);
    let uuv = cross(axis, uv);
    let local = std::array::from_fn(|i| (v[i] + 2.0 * (q[3] / norm * uv[i] + uuv[i])) / scale);
    local.iter().all(|v| v.is_finite()).then_some(local)
}

/// Nearest world hit. Triangle class is irrelevant for state-controlled geometry.
pub(crate) fn ray(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    a: [f32; 3],
    b: [f32; 3],
) -> Option<[f32; 3]> {
    let mut nearest: Option<f32> = None;
    for collider in ctx
        .db
        .game_go_collider()
        .by_partition()
        .filter((map_id, instance_id))
    {
        if !overlaps_sphere(
            a,
            b,
            [collider.x, collider.y, collider.z],
            collider.radius_world,
        ) {
            continue;
        }
        let Some(go) = ctx.db.game_gameobject().guid().find(collider.go_guid) else {
            continue;
        };
        if go.state != 0
            || go.map_id != map_id
            || go.instance_id != instance_id
            || go.template_entry != collider.entry
        {
            continue;
        }
        let Some(model) = ctx.db.game_go_model().entry().find(collider.entry) else {
            continue;
        };
        let pos = [go.x, go.y, go.z];
        let rotation = [go.rotation_0, go.rotation_1, go.rotation_2, go.rotation_3];
        let Some(local_a) = local_point(a, pos, rotation, go.orientation, model.scale) else {
            continue;
        };
        let Some(local_b) = local_point(b, pos, rotation, go.orientation, model.scale) else {
            continue;
        };
        let Ok(tris) = decode(&model.blob) else {
            continue;
        };
        for tri in tris {
            if let Some(t) = segment_tri_hit(local_a, local_b, tri.verts) {
                nearest = Some(nearest.map_or(t, |old| old.min(t)));
            }
        }
    }
    nearest.map(|t| std::array::from_fn(|i| a[i] + t * (b[i] - a[i])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_rejects_only_segments_outside_the_bound() {
        assert!(overlaps_sphere(
            [-2.0, 1.0, 0.0],
            [2.0, 1.0, 0.0],
            [0.0; 3],
            1.0
        ));
        assert!(overlaps_sphere([0.0; 3], [0.0; 3], [0.0; 3], 1.0));
        assert!(!overlaps_sphere([2.0; 3], [2.0; 3], [0.0; 3], 1.0));
        assert!(!overlaps_sphere(
            [-2.0, 2.0, 0.0],
            [2.0, 2.0, 0.0],
            [0.0; 3],
            1.0
        ));
        assert!(!overlaps_sphere(
            [2.0, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            [0.0; 3],
            1.0
        ));
    }

    fn near(actual: [f32; 3], expected: [f32; 3]) {
        for i in 0..3 {
            assert!(
                (actual[i] - expected[i]).abs() < 0.0001,
                "{actual:?} != {expected:?}"
            );
        }
    }

    #[test]
    fn local_transform_undoes_translation_yaw_and_scale() {
        near(
            local_point(
                [10.0, 24.0, 36.0],
                [10.0, 20.0, 30.0],
                [0.0; 4],
                std::f32::consts::FRAC_PI_2,
                2.0,
            )
            .unwrap(),
            [2.0, 0.0, 3.0],
        );
    }

    #[test]
    fn stored_quaternion_overrides_yaw_and_rotates_out_of_the_horizontal_plane() {
        let q = [1.0, 0.0, 0.0, 1.0]; // Normalizes to a quarter turn about X.
        near(
            local_point([2.0, -6.0, 4.0], [0.0; 3], q, 2.0, 2.0).unwrap(),
            [1.0, 2.0, 3.0],
        );
    }

    #[test]
    fn transformed_segment_preserves_the_world_hit_parameter() {
        let pos = [10.0, 20.0, 30.0];
        let a = local_point(
            [10.0, 16.0, 30.0],
            pos,
            [0.0; 4],
            std::f32::consts::FRAC_PI_2,
            2.0,
        )
        .unwrap();
        let b = local_point(
            [10.0, 24.0, 30.0],
            pos,
            [0.0; 4],
            std::f32::consts::FRAC_PI_2,
            2.0,
        )
        .unwrap();
        let tri = [[0.0, -1.0, -1.0], [0.0, 1.0, -1.0], [0.0, 0.0, 1.0]];
        assert!((segment_tri_hit(a, b, tri).unwrap() - 0.5).abs() < 0.0001);
    }

    #[test]
    fn invalid_transforms_do_not_produce_hits() {
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(local_point([0.0; 3], [0.0; 3], [0.0; 4], 0.0, scale).is_none());
        }
        assert!(local_point([0.0; 3], [0.0; 3], [f32::NAN; 4], 0.0, 1.0).is_none());
    }

    #[test]
    fn ray_merge_chooses_the_nearest_source_in_both_directions() {
        use crate::vmap::nearest_hit;
        let near = Some([2.0, 0.0, 0.0]);
        let far = Some([8.0, 0.0, 0.0]);
        assert_eq!(nearest_hit([0.0; 3], near, far), near);
        assert_eq!(nearest_hit([0.0; 3], far, near), near);
        assert_eq!(nearest_hit([10.0, 0.0, 0.0], near, far), far);
        assert_eq!(nearest_hit([0.0; 3], None, near), near);
    }
}
