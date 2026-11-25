// vmangos `PoolManager::LoadFromDB` for GameObjects; read docs/schema.md#gameobject-pools first.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::{bail, Result};

use super::GoPlacement;
use crate::creature_population::{chance_bp, pool_limits, pool_parents};
use crate::dump::{parse_table, push_insert, TableRows};

// Imported points sit far above the ids the member table's own sequence hands out.
const IMPORTED_POINT_BAND: u64 = 1 << 40;

pub(super) struct GameobjectPools {
    owned: HashSet<u32>,
    pool_rows: Vec<String>,
    member_rows: Vec<String>,
    link_rows: Vec<String>,
}

impl GameobjectPools {
    /// Whether a pool owns the placement, so it is no static spawn.
    pub(super) fn owns(&self, db_guid: u32) -> bool {
        self.owned.contains(&db_guid)
    }

    pub(super) fn push_statements(&self, stmts: &mut Vec<String>) {
        stmts.push("DELETE FROM game_gameobject_pool_link WHERE pool_id >= 0".into());
        stmts.push("DELETE FROM game_gameobject_pool_member WHERE point_id >= 0".into());
        stmts.push("DELETE FROM game_gameobject_pool WHERE pool_id >= 0".into());
        push_insert(
            stmts,
            "game_gameobject_pool",
            "pool_id,max_active,in_place",
            &self.pool_rows,
        );
        push_insert(
            stmts,
            "game_gameobject_pool_member",
            "point_id,pool_id,template_entry,map_id,x,y,z,orientation,weight,rotation_0,rotation_1,rotation_2,rotation_3,respawn_min_secs,respawn_max_secs",
            &self.member_rows,
        );
        push_insert(
            stmts,
            "game_gameobject_pool_link",
            "pool_id,parent_pool_id,weight,map_id",
            &self.link_rows,
        );
    }
}

/// Roll weights for one pool's members or child pools, in source order, as vmangos
/// `PoolGroup::RollOne` reads their chances; docs/schema.md#gameobject-pools gives the rule.
pub(super) fn pool_weights(limit: u32, chances_bp: &[u32]) -> Vec<u32> {
    const WHOLE_BP: u32 = 10_000;
    if limit != 1 || chances_bp.iter().all(|&chance| chance == 0) {
        return vec![1; chances_bp.len()];
    }
    let equal = chances_bp.iter().filter(|&&chance| chance == 0).count();
    let scale = u32::try_from(equal.max(1)).unwrap_or(u32::MAX);
    let mut reached = 0u32;
    let shares: Vec<Option<u32>> = chances_bp
        .iter()
        .map(|&chance| {
            (chance != 0).then(|| {
                let next = reached.saturating_add(chance).min(WHOLE_BP);
                let share = next - reached;
                reached = next;
                share
            })
        })
        .collect();
    let rest = WHOLE_BP - reached;
    shares
        .into_iter()
        .map(|share| share.map_or(rest, |share| share.saturating_mul(scale)))
        .collect()
}

/// The source pools that own placements; one under a pool without a limit never spawns.
pub(super) fn collect(
    dump: &(impl TableRows + ?Sized),
    placements: &[GoPlacement],
) -> Result<GameobjectPools> {
    let limits = pool_limits(dump)?;
    let parents = pool_parents(dump)?;
    let spawnable = |pool: u64| limits.get(&pool).is_some_and(|&limit| limit != 0);
    let by_guid: BTreeMap<u32, &GoPlacement> = placements
        .iter()
        .map(|placement| (placement.db_guid, placement))
        .collect();
    let mut by_entry: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
    for placement in placements {
        by_entry
            .entry(placement.entry)
            .or_default()
            .push(placement.db_guid);
    }

    // A guid in both tables keeps its `pool_gameobject` pool; vmangos skips the second listing.
    let mut membership: BTreeMap<u32, (u64, u32)> = BTreeMap::new();
    for row in parse_table(dump, "pool_gameobject")?.iter() {
        let guid: u32 = row.number("guid")?;
        let pool: u64 = row.number("pool_entry")?;
        let chance = chance_bp(row, "chance", "pool_gameobject")?;
        if by_guid.contains_key(&guid) && spawnable(pool) {
            membership.entry(guid).or_insert((pool, chance));
        }
    }
    for row in parse_table(dump, "pool_gameobject_template")?.iter() {
        let entry: u64 = row.number("id")?;
        let pool: u64 = row.number("pool_entry")?;
        let chance = chance_bp(row, "chance", "pool_gameobject_template")?;
        if !spawnable(pool) {
            continue;
        }
        for &guid in by_entry.get(&entry).into_iter().flatten() {
            membership.entry(guid).or_insert((pool, chance));
        }
    }

    let mut root_of: BTreeMap<u64, u64> = BTreeMap::new();
    let mut root_maps: BTreeMap<u64, i64> = BTreeMap::new();
    let mut pool_members: BTreeMap<u64, Vec<(u32, u32)>> = BTreeMap::new();
    let mut never_spawn = 0usize;
    for (&guid, &(pool, chance)) in &membership {
        let mut chain = vec![pool];
        let mut id = pool;
        while let Some(&(parent, _)) = parents.get(&id) {
            if chain.contains(&parent) {
                bail!("cyclic GameObject pool {parent}");
            }
            chain.push(parent);
            id = parent;
        }
        if !chain.iter().all(|&link| spawnable(link)) {
            never_spawn += 1;
            continue;
        }
        let map = by_guid[&guid].map;
        root_maps
            .entry(id)
            .and_modify(|root_map| *root_map = (*root_map).min(map))
            .or_insert(map);
        for link in chain {
            root_of.insert(link, id);
        }
        pool_members.entry(pool).or_default().push((guid, chance));
    }

    let mut children: BTreeMap<u64, Vec<(u64, u32)>> = BTreeMap::new();
    for (&child, &(parent, chance)) in &parents {
        if root_of.contains_key(&child) {
            children.entry(parent).or_default().push((child, chance));
        }
    }
    let mut pool_rows = Vec::new();
    let mut link_rows = Vec::new();
    for (&pool, &root) in &root_of {
        pool_rows.push(format!("({pool},{},false)", limits[&pool]));
        if pool == root {
            link_rows.push(format!("({pool},0,0,{})", root_maps[&root]));
        }
    }
    for (&parent, kids) in &children {
        let chances: Vec<u32> = kids.iter().map(|&(_, chance)| chance).collect();
        for (&(child, _), weight) in kids.iter().zip(pool_weights(limits[&parent], &chances)) {
            link_rows.push(format!(
                "({child},{parent},{weight},{})",
                root_maps[&root_of[&child]]
            ));
        }
    }
    let mut member_rows = Vec::new();
    for (&pool, members) in &pool_members {
        let chances: Vec<u32> = members.iter().map(|&(_, chance)| chance).collect();
        for (&(guid, _), weight) in members.iter().zip(pool_weights(limits[&pool], &chances)) {
            let placement = by_guid[&guid];
            let [x, y, z, o] = placement.position;
            let [rot0, rot1, rot2, rot3] = placement.rotation;
            let (respawn_min, respawn_max) = placement.respawn;
            member_rows.push(format!(
                "({point},{pool},{entry},{map},{x},{y},{z},{o},{weight},{rot0},{rot1},{rot2},{rot3},{respawn_min},{respawn_max})",
                point = IMPORTED_POINT_BAND | u64::from(guid),
                entry = placement.entry,
                map = placement.map,
            ));
        }
    }
    let roots: BTreeSet<u64> = root_of.values().copied().collect();
    eprintln!(
        "gameobject pools: {} top-level and {} nested pools hold {} placements; {never_spawn} \
         placements belong to a pool that never spawns",
        roots.len(),
        root_of.len() - roots.len(),
        member_rows.len()
    );
    Ok(GameobjectPools {
        owned: membership.keys().copied().collect(),
        pool_rows,
        member_rows,
        link_rows,
    })
}
