use spacetimedb::{reducer, table, ReducerContext, Table, Timestamp};

use crate::helpers::require_operator;

const WMO_MAPS: [u32; 1] = [36];

#[table(accessor = game_import_meta)]
pub struct ImportMeta {
    #[primary_key]
    pub family: String,
    pub source_sha: String,
    pub file_hash: String,
    pub row_count: u64,
    pub imported_at: Timestamp,
}

pub(crate) fn stamp(
    ctx: &ReducerContext,
    family: &str,
    source_sha: &str,
    file_hash: &str,
    row_count: u64,
) {
    let table = ctx.db.game_import_meta();
    table.family().delete(family.to_string());
    table.insert(ImportMeta {
        family: family.to_string(),
        source_sha: source_sha.to_string(),
        file_hash: file_hash.to_string(),
        row_count,
        imported_at: crate::time::now(ctx),
    });
}

#[reducer]
pub fn stamp_import_meta(
    ctx: &ReducerContext,
    family: String,
    source_sha: String,
    file_hash: String,
    row_count: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    stamp(ctx, &family, &source_sha, &file_hash, row_count);
    Ok(())
}

#[table(accessor = game_world_import)]
pub struct WorldImport {
    #[primary_key]
    pub map_id: u32,
    pub completed_at: Timestamp,
}

#[reducer]
pub fn begin_world_import(ctx: &ReducerContext) -> Result<(), String> {
    require_operator(ctx)?;
    let table = ctx.db.game_world_import();
    let maps: Vec<u32> = table.iter().map(|row| row.map_id).collect();
    for map_id in maps {
        table.map_id().delete(map_id);
    }
    Ok(())
}

#[reducer]
pub fn complete_world_import(ctx: &ReducerContext, map_ids: Vec<u32>) -> Result<(), String> {
    require_operator(ctx)?;
    content_problem(
        &map_ids,
        |_| true,
        |map| crate::vmap::active_generation_id(ctx, map).is_some(),
    )?;
    let table = ctx.db.game_world_import();
    for map_id in map_ids {
        table.map_id().delete(map_id);
        table.insert(WorldImport {
            map_id,
            completed_at: crate::time::now(ctx),
        });
    }
    Ok(())
}

#[reducer]
pub fn check_world_content(ctx: &ReducerContext, map_ids: Vec<u32>) -> Result<(), String> {
    require_operator(ctx)?;
    let imports = ctx.db.game_world_import();
    content_problem(
        &map_ids,
        |map| imports.map_id().find(map).is_some(),
        |map| crate::vmap::active_generation_id(ctx, map).is_some(),
    )
}

fn content_problem(
    map_ids: &[u32],
    completed: impl Fn(u32) -> bool,
    wmo_active: impl Fn(u32) -> bool,
) -> Result<(), String> {
    if map_ids.is_empty() {
        return Err("no maps named for the content check".to_string());
    }
    if let Some(map) = map_ids.iter().copied().find(|&map| !completed(map)) {
        return Err(format!("no completed world import for map {map}"));
    }
    if let Some(map) = map_ids
        .iter()
        .copied()
        .find(|map| WMO_MAPS.contains(map) && !wmo_active(*map))
    {
        return Err(format!("missing imported WMO geometry for map {map}"));
    }
    Ok(())
}

#[cfg(test)]
mod world_import_tests {
    use super::content_problem;

    #[test]
    fn complete_content_passes() {
        assert_eq!(
            content_problem(&[0, 1, 36, 369], |_| true, |_| true),
            Ok(())
        );
    }

    #[test]
    fn each_missing_part_names_itself() {
        let refused = |result: Result<(), String>| result.unwrap_err();
        assert!(refused(content_problem(&[], |_| true, |_| true)).contains("no maps"));
        assert_eq!(
            refused(content_problem(&[0, 1], |map| map == 0, |_| true)),
            "no completed world import for map 1"
        );
        assert_eq!(
            refused(content_problem(&[0, 36], |_| true, |_| false)),
            "missing imported WMO geometry for map 36"
        );
    }

    #[test]
    fn maps_without_an_importer_vmap_slice_need_no_wmo_geometry() {
        assert_eq!(content_problem(&[0, 1, 369], |_| true, |_| false), Ok(()));
    }
}
