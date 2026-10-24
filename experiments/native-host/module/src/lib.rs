use spacetimedb::{
    reducer, table, view, ConnectionId, Identity, Query, ReducerContext, Table, ViewContext,
};

#[table(accessor = experiment)]
pub struct Experiment {
    #[primary_key]
    id: u8,
    operator: Identity,
    count: u64,
    edges: bool,
}

#[table(accessor = motion)]
#[derive(Clone)]
pub struct Motion {
    #[primary_key]
    guid: u64,
    #[index(btree)]
    cell: i64,
    x: f32,
    y: f32,
    z: f32,
    tick: u32,
    sequence: u64,
    hidden: bool,
    ghost_only: bool,
}

#[table(accessor = viewer)]
pub struct Viewer {
    #[primary_key]
    identity: Identity,
    connection: Option<ConnectionId>,
    #[unique]
    guid: u64,
    cell: i64,
    ghost: bool,
}

#[table(accessor = viewer_cell, index(accessor = by_viewer_cell, btree(columns = [viewer, cell])))]
pub struct ViewerCell {
    #[primary_key]
    #[auto_inc]
    id: u64,
    #[index(btree)]
    viewer: Identity,
    ghost: bool,
    #[index(btree)]
    cell: i64,
}

#[table(accessor = visible_entity, index(accessor = by_viewer_guid, btree(columns = [viewer, guid])))]
pub struct VisibleEntity {
    #[primary_key]
    #[auto_inc]
    id: u64,
    #[index(btree)]
    viewer: Identity,
    #[index(btree)]
    guid: u64,
}

#[reducer(init)]
pub fn init(ctx: &ReducerContext) {
    ctx.db.experiment().insert(Experiment {
        id: 0,
        operator: ctx.sender(),
        count: 0,
        edges: false,
    });
}

fn operator(ctx: &ReducerContext) -> Result<Experiment, String> {
    ctx.db
        .experiment()
        .id()
        .find(0)
        .filter(|e| e.operator == ctx.sender())
        .ok_or_else(|| "operator required".into())
}

fn cell(x: f32, y: f32, space: u32) -> i64 {
    let gx = (x / 50.0).floor() as i16;
    let gy = (y / 50.0).floor() as i16;
    ((space as i64) << 32) | ((gx as u16 as i64) << 16) | (gy as u16 as i64)
}

fn neighbors(key: i64) -> impl Iterator<Item = i64> {
    let space = key & !0xffff_ffff;
    let gx = (key >> 16) as i16;
    let gy = key as i16;
    (-2..=2).flat_map(move |dx| {
        (-2..=2).map(move |dy| {
            space | (i64::from((gx + dx) as u16) << 16) | i64::from((gy + dy) as u16)
        })
    })
}

#[reducer]
pub fn seed(ctx: &ReducerContext, count: u64, dense: bool, edges: bool) -> Result<(), String> {
    let mut config = operator(ctx)?;
    if count > 5000 || config.count != 0 {
        return Err("seed requires a fresh experiment with at most 5000 Characters".into());
    }
    config.count = count;
    config.edges = edges;
    ctx.db.experiment().id().update(config);
    for guid in 1..=count {
        let group = if dense { 0 } else { (guid - 1) / 32 };
        let x = group as f32 * 400.0 + (guid % 8) as f32 + 20.0;
        let y = (guid % 4) as f32 + 20.0;
        ctx.db.motion().insert(Motion {
            guid,
            cell: cell(x, y, 0),
            x,
            y,
            z: 0.0,
            tick: 0,
            sequence: 0,
            hidden: false,
            ghost_only: false,
        });
    }
    Ok(())
}

fn grant_cells(ctx: &ReducerContext, viewer: Identity, anchor: i64) {
    ctx.db.viewer_cell().viewer().delete(viewer);
    let ghost = ctx
        .db
        .viewer()
        .identity()
        .find(viewer)
        .is_some_and(|v| v.ghost);
    for key in neighbors(anchor) {
        ctx.db.viewer_cell().insert(ViewerCell {
            id: 0,
            viewer,
            ghost,
            cell: key,
        });
    }
}

fn grant_entities(ctx: &ReducerContext, identity: Identity, anchor: i64) {
    ctx.db.visible_entity().viewer().delete(identity);
    let ghost = ctx
        .db
        .viewer()
        .identity()
        .find(identity)
        .is_some_and(|v| v.ghost);
    for key in neighbors(anchor) {
        for entity in ctx
            .db
            .motion()
            .cell()
            .filter(key)
            .filter(|m| !m.hidden && (!m.ghost_only || ghost))
        {
            ctx.db.visible_entity().insert(VisibleEntity {
                id: 0,
                viewer: identity,
                guid: entity.guid,
            });
        }
    }
}

#[reducer]
pub fn enter(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    if ctx.sender() != Identity::from_claims("native-host-experiment", &format!("E{guid}")) {
        return Err("native Account does not own this Character".into());
    }
    if ctx.db.viewer().identity().find(ctx.sender()).is_some() {
        return Err("connection already owns a Character".into());
    }
    let entity = ctx
        .db
        .motion()
        .guid()
        .find(guid)
        .ok_or("unknown Character")?;
    if ctx.db.viewer().guid().find(guid).is_some() {
        return Err("Character already connected".into());
    }
    ctx.db.viewer().insert(Viewer {
        identity: ctx.sender(),
        connection: ctx.connection_id(),
        guid,
        cell: entity.cell,
        ghost: false,
    });
    grant_cells(ctx, ctx.sender(), entity.cell);
    if ctx.db.experiment().id().find(0).is_some_and(|e| e.edges) {
        grant_entities(ctx, ctx.sender(), entity.cell);
    }
    Ok(())
}

#[reducer(client_disconnected)]
pub fn leave(ctx: &ReducerContext) {
    if !ctx
        .db
        .viewer()
        .identity()
        .find(ctx.sender())
        .is_some_and(|v| v.connection == ctx.connection_id())
    {
        return;
    }
    ctx.db.visible_entity().viewer().delete(ctx.sender());
    ctx.db.viewer_cell().viewer().delete(ctx.sender());
    ctx.db.viewer().identity().delete(ctx.sender());
}

fn replace_motion(ctx: &ReducerContext, next: Motion) {
    let old = ctx.db.motion().guid().find(next.guid).unwrap();
    let new_cell = next.cell;
    let guid = next.guid;
    let hidden = next.hidden;
    let ghost_only = next.ghost_only;
    ctx.db.motion().guid().update(next);
    if old.cell == new_cell && old.hidden == hidden && old.ghost_only == ghost_only {
        return;
    }
    let edges = ctx.db.experiment().id().find(0).is_some_and(|e| e.edges);
    if edges {
        ctx.db.visible_entity().guid().delete(guid);
        for grant in ctx
            .db
            .viewer_cell()
            .cell()
            .filter(new_cell)
            .filter(|g| !hidden && (!ghost_only || g.ghost))
        {
            ctx.db.visible_entity().insert(VisibleEntity {
                id: 0,
                viewer: grant.viewer,
                guid,
            });
        }
    }
    if let Some(mut v) = ctx.db.viewer().guid().find(guid) {
        v.cell = new_cell;
        grant_cells(ctx, v.identity, new_cell);
        if edges {
            grant_entities(ctx, v.identity, new_cell);
        }
        ctx.db.viewer().identity().update(v);
    }
}

#[reducer]
pub fn move_character(
    ctx: &ReducerContext,
    x: f32,
    y: f32,
    z: f32,
    tick: u32,
) -> Result<(), String> {
    if !x.is_finite()
        || !y.is_finite()
        || !z.is_finite()
        || x.abs() > 100000.0
        || y.abs() > 100000.0
    {
        return Err("invalid position".into());
    }
    let v = ctx
        .db
        .viewer()
        .identity()
        .find(ctx.sender())
        .ok_or("enter first")?;
    let mut row = ctx
        .db
        .motion()
        .guid()
        .find(v.guid)
        .ok_or("unknown Character")?;
    row.x = x;
    row.y = y;
    row.z = z;
    row.cell = cell(x, y, (row.cell >> 32) as u32);
    row.tick = tick;
    row.sequence += 1;
    replace_motion(ctx, row);
    Ok(())
}

#[reducer]
pub fn step(
    ctx: &ReducerContext,
    first: u64,
    count: u64,
    tick: u32,
    crossing: bool,
) -> Result<(), String> {
    let config = operator(ctx)?;
    if first == 0 || count > 128 || first.saturating_add(count) > config.count + 1 {
        return Err("invalid movement batch".into());
    }
    for guid in first..first + count {
        let mut row = ctx
            .db
            .motion()
            .guid()
            .find(guid)
            .ok_or("unknown Character")?;
        row.sequence += 1;
        row.tick = tick;
        if crossing {
            row.x += if row.sequence % 2 == 0 { -160.0 } else { 160.0 };
            row.cell = cell(row.x, row.y, (row.cell >> 32) as u32);
        } else {
            row.z = (row.sequence % 2) as f32;
        }
        replace_motion(ctx, row);
    }
    Ok(())
}

#[reducer]
pub fn set_hidden(
    ctx: &ReducerContext,
    guid: u64,
    hidden: bool,
    ghost_only: bool,
) -> Result<(), String> {
    operator(ctx)?;
    let mut row = ctx
        .db
        .motion()
        .guid()
        .find(guid)
        .ok_or("unknown Character")?;
    row.hidden = hidden;
    row.ghost_only = ghost_only;
    replace_motion(ctx, row);
    Ok(())
}

#[reducer]
pub fn set_ghost(ctx: &ReducerContext, ghost: bool) -> Result<(), String> {
    let mut v = ctx
        .db
        .viewer()
        .identity()
        .find(ctx.sender())
        .ok_or("enter first")?;
    v.ghost = ghost;
    let (identity, anchor) = (v.identity, v.cell);
    ctx.db.viewer().identity().update(v);
    grant_cells(ctx, identity, anchor);
    if ctx.db.experiment().id().find(0).is_some_and(|e| e.edges) {
        grant_entities(ctx, identity, anchor);
    }
    Ok(())
}

#[reducer]
pub fn set_viewer_ghost(ctx: &ReducerContext, guid: u64, ghost: bool) -> Result<(), String> {
    operator(ctx)?;
    let mut v = ctx.db.viewer().guid().find(guid).ok_or("enter first")?;
    v.ghost = ghost;
    let (identity, anchor) = (v.identity, v.cell);
    ctx.db.viewer().identity().update(v);
    grant_cells(ctx, identity, anchor);
    if ctx.db.experiment().id().find(0).is_some_and(|e| e.edges) {
        grant_entities(ctx, identity, anchor);
    }
    Ok(())
}

#[view(accessor = cell_motion, public)]
pub fn cell_motion(ctx: &ViewContext) -> impl Query<Motion> {
    let ghost = ctx
        .db
        .viewer()
        .identity()
        .find(ctx.sender())
        .is_some_and(|v| v.ghost);
    ctx.from
        .viewer_cell()
        .r#where(|v| v.viewer.eq(ctx.sender()))
        .right_semijoin(ctx.from.motion(), |v, m| v.cell.eq(m.cell))
        .r#where(|m| {
            m.hidden
                .eq(false)
                .and(m.ghost_only.eq(false).or(m.ghost_only.eq(ghost)))
        })
}

#[view(accessor = edge_motion, public)]
pub fn edge_motion(ctx: &ViewContext) -> impl Query<Motion> {
    let ghost = ctx
        .db
        .viewer()
        .identity()
        .find(ctx.sender())
        .is_some_and(|v| v.ghost);
    ctx.from
        .visible_entity()
        .r#where(|v| v.viewer.eq(ctx.sender()))
        .right_semijoin(ctx.from.motion(), |v, m| v.guid.eq(m.guid))
        .r#where(|m| {
            m.hidden
                .eq(false)
                .and(m.ghost_only.eq(false).or(m.ghost_only.eq(ghost)))
        })
}

#[reducer]
pub fn set_space(ctx: &ReducerContext, guid: u64, space: u32) -> Result<(), String> {
    operator(ctx)?;
    let mut row = ctx
        .db
        .motion()
        .guid()
        .find(guid)
        .ok_or("unknown Character")?;
    row.cell = cell(row.x, row.y, space);
    replace_motion(ctx, row);
    Ok(())
}
