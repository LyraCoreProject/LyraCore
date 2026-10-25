use std::collections::{BTreeMap, BTreeSet};

use lyracore_shared::constants::{npc_flags::SPIRITHEALER, player_flags::GHOST};
use lyracore_shared::spatial::{grid_cell_id, grid_cell_of_id, BOX_HALF_SPAN};
use spacetimedb::{table, ReducerContext, Table};

use crate::corpse::{game_corpse, Corpse};
use crate::gameobject::{game_gameobject, GameObject};
use crate::spell::{game_dynamic_object, DynamicObject};
use crate::world::{game_world_entity, WorldEntity};

pub(crate) const WORLD_ENTITY: u8 = 0;
pub(crate) const GAME_OBJECT: u8 = 1;
pub(crate) const CORPSE: u8 = 2;
pub(crate) const DYNAMIC_OBJECT: u8 = 3;
pub(crate) const TRANSPORT: u8 = 4;

#[table(
    accessor = game_aoi_grant,
    index(accessor = by_viewer, btree(columns = [viewer_guid])),
    index(accessor = by_subject, btree(columns = [layer, entity_guid]))
)]
pub struct AoiGrant {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub viewer_guid: u64,
    pub layer: u8,
    pub entity_guid: u64,
}

#[derive(Clone, PartialEq, Eq)]
#[table(
    accessor = game_aoi_viewer,
    index(accessor = by_cell, btree(columns = [map_id, instance_id, cell]))
)]
pub struct AoiViewer {
    #[primary_key]
    pub guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub cell: i64,
    pub is_ghost: bool,
}

crate::character_owned!(delete, fn sweep_delete_game_aoi_grant(ctx, character_guid) {
    remove_viewer_grants(ctx, character_guid);
    remove_subject(ctx, WORLD_ENTITY, character_guid);
});

crate::character_owned!(delete, fn sweep_delete_game_aoi_viewer(ctx, character_guid) {
    ctx.db.game_aoi_viewer().guid().delete(character_guid);
});

// AOI Grants are derived from the live entities in their Shard.
crate::character_owned!(not_transported, fn sweep_transfer_game_aoi_grant());
crate::character_owned!(not_transported, fn sweep_transfer_game_aoi_viewer());

impl AoiViewer {
    fn of(entity: &WorldEntity) -> Self {
        Self {
            guid: entity.guid,
            map_id: entity.map_id,
            instance_id: entity.instance_id,
            cell: entity.cell,
            is_ghost: entity.player_flags & GHOST != 0,
        }
    }

    fn sees_entity(&self, entity: &WorldEntity, stealthed: bool) -> bool {
        self.contains(entity.map_id, entity.instance_id, entity.cell)
            && (self.guid == entity.guid
                || (!stealthed && (self.is_ghost || entity.npc_flags & SPIRITHEALER == 0)))
    }

    fn contains(&self, map_id: u32, instance_id: u64, cell: i64) -> bool {
        let (x, y) = grid_cell_of_id(self.cell);
        let (other_x, other_y) = grid_cell_of_id(cell);
        self.map_id == map_id
            && self.instance_id == instance_id
            && x.abs_diff(other_x) <= BOX_HALF_SPAN as u32
            && y.abs_diff(other_y) <= BOX_HALF_SPAN as u32
    }
}

fn cells(cell: i64) -> impl Iterator<Item = i64> {
    let (x, y) = grid_cell_of_id(cell);
    (x.saturating_sub(BOX_HALF_SPAN)..=x.saturating_add(BOX_HALF_SPAN)).flat_map(move |x| {
        (y.saturating_sub(BOX_HALF_SPAN)..=y.saturating_add(BOX_HALF_SPAN))
            .map(move |y| grid_cell_id(x, y))
    })
}

pub(crate) fn entity_changed(
    ctx: &ReducerContext,
    old: Option<&WorldEntity>,
    entity: &WorldEntity,
) {
    let viewers = ctx.db.game_aoi_viewer();
    if let Some(previous) = viewers.guid().find(entity.guid) {
        if entity.is_player() {
            let viewer = AoiViewer::of(entity);
            if previous != viewer {
                viewers.guid().update(viewer.clone());
                refresh_viewer(ctx, &viewer);
            }
        } else {
            viewers.guid().delete(entity.guid);
            remove_viewer_grants(ctx, entity.guid);
        }
    }
    if old.is_some_and(|old| {
        old.map_id == entity.map_id
            && old.instance_id == entity.instance_id
            && old.cell == entity.cell
            && old.npc_flags == entity.npc_flags
    }) {
        return;
    }
    refresh_entity(ctx, entity.guid);
}

pub(crate) fn enter_viewer(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    let entity = ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .filter(WorldEntity::is_player)
        .ok_or("AOI viewer requires a live Character")?;
    let viewer = AoiViewer::of(&entity);
    let viewers = ctx.db.game_aoi_viewer();
    if viewers.guid().find(guid).is_some() {
        viewers.guid().update(viewer.clone());
    } else {
        viewers.insert(viewer.clone());
    }
    refresh_viewer(ctx, &viewer);
    Ok(())
}

pub(crate) fn refresh_entity(ctx: &ReducerContext, guid: u64) {
    if ctx.db.game_aoi_viewer().count() == 0 {
        return;
    }
    let Some(entity) = ctx.db.game_world_entity().guid().find(guid) else {
        remove_subject(ctx, WORLD_ENTITY, guid);
        return;
    };
    let stealthed = crate::spell::is_stealthed(ctx, guid);
    let wanted = nearby_viewers(ctx, entity.map_id, entity.instance_id, entity.cell)
        .filter(|viewer| viewer.sees_entity(&entity, stealthed))
        .map(|viewer| viewer.guid)
        .collect();
    replace_subject(ctx, WORLD_ENTITY, guid, wanted);
}

pub(crate) fn gameobject_changed(ctx: &ReducerContext, object: &GameObject) {
    if ctx.db.game_aoi_viewer().count() == 0 {
        return;
    }
    let wanted = nearby_viewers(ctx, object.map_id, object.instance_id, object.cell)
        .map(|viewer| viewer.guid)
        .collect();
    replace_subject(ctx, GAME_OBJECT, object.guid, wanted);
}

pub(crate) fn corpse_changed(ctx: &ReducerContext, corpse: &Corpse) {
    let wanted = nearby_viewers(ctx, corpse.map_id, corpse.instance_id, corpse.cell)
        .map(|viewer| viewer.guid)
        .collect();
    replace_subject(ctx, CORPSE, corpse.guid, wanted);
}

pub(crate) fn dynamic_object_changed(ctx: &ReducerContext, object: &DynamicObject) {
    let wanted = nearby_viewers(ctx, object.map_id, object.instance_id, object.cell)
        .map(|viewer| viewer.guid)
        .collect();
    replace_subject(ctx, DYNAMIC_OBJECT, object.guid, wanted);
}

pub(crate) fn nearby_viewers(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    cell: i64,
) -> impl Iterator<Item = AoiViewer> + '_ {
    cells(cell).flat_map(move |cell| {
        ctx.db
            .game_aoi_viewer()
            .by_cell()
            .filter((map_id, instance_id, cell))
    })
}

fn refresh_viewer(ctx: &ReducerContext, viewer: &AoiViewer) {
    let mut wanted = BTreeSet::new();
    for cell in cells(viewer.cell) {
        for entity in
            ctx.db
                .game_world_entity()
                .by_cell()
                .filter((viewer.map_id, viewer.instance_id, cell))
        {
            if viewer.sees_entity(&entity, crate::spell::is_stealthed(ctx, entity.guid)) {
                wanted.insert((WORLD_ENTITY, entity.guid));
            }
        }
        for object in
            ctx.db
                .game_gameobject()
                .by_cell()
                .filter((viewer.map_id, viewer.instance_id, cell))
        {
            wanted.insert((GAME_OBJECT, object.guid));
        }
        for corpse in
            ctx.db
                .game_corpse()
                .by_cell()
                .filter((viewer.map_id, viewer.instance_id, cell))
        {
            wanted.insert((CORPSE, corpse.guid));
        }
        for object in
            ctx.db
                .game_dynamic_object()
                .by_cell()
                .filter((viewer.map_id, viewer.instance_id, cell))
        {
            wanted.insert((DYNAMIC_OBJECT, object.guid));
        }
    }
    for transport in crate::transport::visible_on(ctx, viewer.map_id, viewer.instance_id) {
        wanted.insert((TRANSPORT, transport));
    }
    let grants = ctx.db.game_aoi_grant();
    let existing: BTreeMap<_, _> = grants
        .by_viewer()
        .filter(viewer.guid)
        .map(|grant| ((grant.layer, grant.entity_guid), grant))
        .collect();
    for (subject, grant) in existing {
        if !wanted.remove(&subject) {
            grants.id().delete(grant.id);
            crate::native::grant_removed(ctx, &grant);
        }
    }
    // A Passenger's CREATE names its Transport, so the client must already have that Transport.
    let (transports, others): (Vec<_>, Vec<_>) = wanted
        .into_iter()
        .partition(|(layer, _)| *layer == TRANSPORT);
    for (layer, entity_guid) in transports.into_iter().chain(others) {
        let grant = grants.insert(AoiGrant {
            id: 0,
            viewer_guid: viewer.guid,
            layer,
            entity_guid,
        });
        crate::native::grant_added(ctx, &grant);
    }
}

fn replace_subject(ctx: &ReducerContext, layer: u8, entity_guid: u64, mut wanted: BTreeSet<u64>) {
    let grants = ctx.db.game_aoi_grant();
    let existing: Vec<_> = grants.by_subject().filter((layer, entity_guid)).collect();
    for grant in existing {
        if !wanted.remove(&grant.viewer_guid) {
            grants.id().delete(grant.id);
            crate::native::grant_removed(ctx, &grant);
        }
    }
    for viewer_guid in wanted {
        let grant = grants.insert(AoiGrant {
            id: 0,
            viewer_guid,
            layer,
            entity_guid,
        });
        crate::native::grant_added(ctx, &grant);
    }
}

pub(crate) fn entity_removed(ctx: &ReducerContext, guid: u64) {
    ctx.db.game_aoi_viewer().guid().delete(guid);
    remove_viewer_grants(ctx, guid);
    remove_subject(ctx, WORLD_ENTITY, guid);
}

pub(crate) fn remove_subject(ctx: &ReducerContext, layer: u8, guid: u64) {
    let grants = ctx.db.game_aoi_grant();
    let rows: Vec<_> = grants.by_subject().filter((layer, guid)).collect();
    for grant in rows {
        grants.id().delete(grant.id);
        crate::native::grant_removed(ctx, &grant);
    }
}

fn remove_viewer_grants(ctx: &ReducerContext, guid: u64) {
    let grants = ctx.db.game_aoi_grant();
    let rows: Vec<_> = grants.by_viewer().filter(guid).collect();
    for grant in rows {
        grants.id().delete(grant.id);
        crate::native::grant_removed(ctx, &grant);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer() -> AoiViewer {
        AoiViewer {
            guid: 1,
            map_id: 0,
            instance_id: 7,
            cell: grid_cell_id(0, 0),
            is_ghost: false,
        }
    }

    #[test]
    fn aoi_includes_the_boundary_and_excludes_other_maps_and_instances() {
        let viewer = viewer();
        assert!(viewer.contains(0, 7, grid_cell_id(-2, 2)));
        assert!(!viewer.contains(0, 7, grid_cell_id(3, 0)));
        assert!(!viewer.contains(1, 7, grid_cell_id(0, 0)));
        assert!(!viewer.contains(0, 8, grid_cell_id(0, 0)));
    }

    #[test]
    fn spirit_healers_require_a_ghost_and_stealth_hides_peers() {
        let mut viewer = viewer();
        let mut entity = crate::helpers::tests::entity(2, 0, 0, 0, 0);
        entity.instance_id = 7;
        entity.npc_flags = SPIRITHEALER;
        assert!(!viewer.sees_entity(&entity, false));
        viewer.is_ghost = true;
        assert!(viewer.sees_entity(&entity, false));
        assert!(!viewer.sees_entity(&entity, true));
        entity.guid = viewer.guid;
        assert!(viewer.sees_entity(&entity, true));
    }

    #[test]
    fn extreme_cells_do_not_wrap_into_the_aoi() {
        let mut viewer = viewer();
        viewer.cell = grid_cell_id(i32::MIN, i32::MIN);
        assert!(!viewer.contains(0, 7, grid_cell_id(i32::MAX, i32::MAX)));
        assert_eq!(cells(viewer.cell).count(), 9);
    }
}
