use spacetimedb::{table, ReducerContext, Table};

#[table(accessor = game_graveyard, public)]
pub struct GraveyardLoc {
    #[primary_key]
    pub id: u32,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,

    pub o: f32,
    pub name: String,
}

#[table(
    accessor = game_graveyard_zone,
    public,
    index(accessor = by_zone, btree(columns = [zone_id]))
)]
pub struct GraveyardZone {
    #[primary_key]
    #[auto_inc]
    pub row_id: u64,
    pub safe_loc_id: u32,
    pub zone_id: u32,
    pub faction: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct Graveyard {
    pub(crate) map: u32,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) z: f32,
    pub(crate) o: f32,
}

const NORTHSHIRE: Graveyard = Graveyard {
    map: 0,
    x: -8935.33,
    y: -188.646,
    z: 80.4165,
    o: 2.72271,
};

const GOLDSHIRE: Graveyard = Graveyard {
    map: 0,
    x: -9339.59,
    y: 171.73,
    z: 63.5258,
    o: 0.0,
};

const EASTVALE: Graveyard = Graveyard {
    map: 0,
    x: -9552.73,
    y: -1374.84,
    z: 57.0867,
    o: 0.0,
};

// These Westfall fallback coordinates are unsourced estimates.
const SENTINEL_HILL: Graveyard = Graveyard {
    map: 0,
    x: -10650.0,
    y: 1180.0,
    z: 34.0,
    o: 0.0,
};

const WESTFALL_COAST: Graveyard = Graveyard {
    map: 0,
    x: -11390.0,
    y: 1590.0,
    z: 6.0,
    o: 0.0,
};

const STATIC_CANDIDATES: [Graveyard; 5] = [
    NORTHSHIRE,
    GOLDSHIRE,
    EASTVALE,
    SENTINEL_HILL,
    WESTFALL_COAST,
];

pub(crate) fn instance_release_zone(map_id: u32) -> Option<u32> {
    crate::instance::dungeon(map_id).map(|d| d.release_zone)
}

pub(crate) fn instance_static_fallback(map_id: u32) -> Option<Graveyard> {
    crate::instance::dungeon(map_id).map(|d| {
        let (map, x, y, z, o) = d.release_fallback;
        Graveyard { map, x, y, z, o }
    })
}

pub(crate) fn pick_instance_graveyard(
    linked: &[Graveyard],
    static_fallback: Option<Graveyard>,
    px: f32,
    py: f32,
) -> Option<Graveyard> {
    nearest_of(linked, px, py).or(static_fallback)
}

pub(crate) fn nearest_of(candidates: &[Graveyard], px: f32, py: f32) -> Option<Graveyard> {
    candidates
        .iter()
        .min_by(|a, b| {
            let da = (a.x - px).powi(2) + (a.y - py).powi(2);
            let db = (b.x - px).powi(2) + (b.y - py).powi(2);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
}

pub(crate) fn nearest(px: f32, py: f32) -> Graveyard {
    nearest_of(&STATIC_CANDIDATES, px, py).unwrap_or(NORTHSHIRE)
}

pub(crate) fn pick_graveyard(
    zone_linked: &[Graveyard],
    all: &[Graveyard],
    px: f32,
    py: f32,
) -> Graveyard {
    nearest_of(zone_linked, px, py)
        .or_else(|| nearest_of(all, px, py))
        .unwrap_or_else(|| nearest(px, py))
}

pub(crate) use lyracore_shared::faction::team_for_race;

fn to_gy(g: GraveyardLoc) -> Graveyard {
    Graveyard {
        map: g.map_id,
        x: g.x,
        y: g.y,
        z: g.z,
        o: g.o,
    }
}

fn linked_to(
    ctx: &ReducerContext,
    zone_id: u32,
    team: u32,
    map_filter: Option<u32>,
    outside_instance: bool,
) -> Vec<Graveyard> {
    ctx.db
        .game_graveyard_zone()
        .by_zone()
        .filter(&zone_id)
        .filter(|gz| gz.faction == 0 || gz.faction == team)
        .filter_map(|gz| ctx.db.game_graveyard().id().find(gz.safe_loc_id))
        .filter(|g| map_filter.is_none_or(|m| g.map_id == m))
        .filter(|g| {
            !outside_instance || !lyracore_shared::instance::has_instance_population(g.map_id)
        })
        .map(to_gy)
        .collect()
}

fn area_then_zone_linked(
    ctx: &ReducerContext,
    area_id: Option<u32>,
    zones: &[u32],
    team: u32,
    map_filter: Option<u32>,
    outside_instance: bool,
) -> Vec<Graveyard> {
    if let Some(area) = area_id {
        let linked = linked_to(ctx, area, team, map_filter, outside_instance);
        if !linked.is_empty() {
            return linked;
        }
    }
    zones
        .iter()
        .filter(|&&zone| Some(zone) != area_id)
        .flat_map(|&zone| linked_to(ctx, zone, team, map_filter, outside_instance))
        .collect()
}

fn all_on_map(ctx: &ReducerContext, map_id: u32) -> Vec<Graveyard> {
    ctx.db
        .game_graveyard()
        .iter()
        .filter(|g| g.map_id == map_id)
        .map(to_gy)
        .collect()
}

pub(crate) fn resolve_graveyard(
    ctx: &ReducerContext,
    map_id: u32,
    x: f32,
    y: f32,
    race: u8,
) -> Graveyard {
    let team = team_for_race(race);
    let area_id = crate::terrain::area_id_at(ctx, map_id, x, y);

    if crate::instance::is_dungeon_map(map_id) {
        use crate::game_area;
        let entrance = crate::instance::imported_entrance_fallback(ctx, map_id);
        let mut zones: Vec<u32> = ctx
            .db
            .game_area()
            .iter()
            .filter(|area| area.map_id == map_id && area.parent_area_id == 0)
            .map(|area| area.id)
            .collect();
        if let Some(zone) = instance_release_zone(map_id) {
            zones.push(zone);
        }
        zones.sort_unstable();
        zones.dedup();
        let linked: Vec<Graveyard> = area_then_zone_linked(
            ctx,
            area_id,
            &zones,
            team,
            entrance.map(|point| point.0),
            true,
        );
        // Dungeon coordinates cannot be compared with positions on the outside map.
        if let Some((outside_map, outside_x, outside_y, outside_z, outside_o)) = entrance {
            let fallback = nearest_of(&all_on_map(ctx, outside_map), outside_x, outside_y)
                .or_else(|| instance_static_fallback(map_id));
            if let Some(graveyard) =
                pick_instance_graveyard(&linked, fallback, outside_x, outside_y)
            {
                return graveyard;
            }
            return Graveyard {
                map: outside_map,
                x: outside_x,
                y: outside_y,
                z: outside_z,
                o: outside_o,
            };
        }
        return linked
            .into_iter()
            .next()
            .or_else(|| instance_static_fallback(map_id))
            .unwrap_or(NORTHSHIRE);
    }
    let zone = crate::terrain::zone_id_at(ctx, map_id, x, y);
    let linked = area_then_zone_linked(ctx, area_id, zone.as_slice(), team, Some(map_id), false);
    let all = all_on_map(ctx, map_id);
    pick_graveyard(&linked, &all, x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadmines_release_resolves_cross_map_to_westfall_never_by_raw_distance() {
        assert_eq!(instance_release_zone(36), Some(1581));
        assert_eq!(
            instance_release_zone(0),
            None,
            "open-world maps use the normal chain"
        );
        let fallback = instance_static_fallback(36).expect("map 36 has a static floor");
        assert_eq!(
            fallback.map, 0,
            "the Deadmines release floor is on map 0 (Westfall), not map 36"
        );
        assert_eq!(
            (fallback.x, fallback.y),
            (-10650.0, 1180.0),
            "…at Sentinel Hill"
        );
        assert!(instance_static_fallback(0).is_none());

        let linked = Graveyard {
            map: 0,
            x: -10600.0,
            y: 1100.0,
            z: 30.0,
            o: 0.0,
        };
        let picked = pick_instance_graveyard(&[linked], Some(fallback), -16.4, -383.0)
            .expect("linked candidate resolves");
        assert_eq!(
            (picked.x, picked.y),
            (-10600.0, 1100.0),
            "an imported link beats the static floor"
        );
        let floor = pick_instance_graveyard(&[], Some(fallback), -16.4, -383.0).unwrap();
        assert_eq!(
            (floor.x, floor.y),
            (fallback.x, fallback.y),
            "no import → the Sentinel Hill floor"
        );
        assert!(pick_instance_graveyard(&[], None, 0.0, 0.0).is_none());
    }

    #[test]
    fn nearest_graveyard_picks_the_closest_of_the_three_by_squared_distance() {
        let n = nearest(-8935.33, -188.646);
        assert_eq!(
            (n.x, n.y),
            (-8935.33, -188.646),
            "Northshire death releases at Northshire"
        );
        let g = nearest(-9339.59, 171.73);
        assert_eq!(
            (g.x, g.y),
            (-9339.59, 171.73),
            "Goldshire death releases at Goldshire"
        );
        let e = nearest(-9552.73, -1374.84);
        assert_eq!(
            (e.x, e.y),
            (-9552.73, -1374.84),
            "Eastvale death releases at Eastvale"
        );
        let s = nearest(-9500.0, -1000.0);
        assert_eq!((s.x, s.y), (-9552.73, -1374.84));
        let w = nearest(-10700.0, 1100.0);
        assert_eq!((w.x, w.y), (-10650.0, 1180.0));
    }

    #[test]
    fn pick_graveyard_prefers_a_zone_linked_graveyard_over_a_closer_unlinked_one() {
        let close_unlinked = Graveyard {
            map: 0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            o: 0.0,
        };
        let far_linked = Graveyard {
            map: 0,
            x: 500.0,
            y: 500.0,
            z: 0.0,
            o: 0.0,
        };
        let linked = [far_linked];
        let all = [close_unlinked, far_linked];
        let picked = pick_graveyard(&linked, &all, 0.0, 0.0);
        assert_eq!(
            (picked.x, picked.y),
            (500.0, 500.0),
            "zone-linked wins despite being farther"
        );
    }

    #[test]
    fn pick_graveyard_falls_back_through_all_then_to_the_static_consts() {
        let only_all = [Graveyard {
            map: 0,
            x: 42.0,
            y: 42.0,
            z: 0.0,
            o: 0.0,
        }];
        let picked = pick_graveyard(&[], &only_all, 0.0, 0.0);
        assert_eq!((picked.x, picked.y), (42.0, 42.0));
        let picked_static = pick_graveyard(&[], &[], -10700.0, 1100.0);
        let expected = nearest(-10700.0, 1100.0);
        assert_eq!((picked_static.x, picked_static.y), (expected.x, expected.y));
    }

    #[test]
    fn team_for_race_maps_alliance_and_horde_and_defaults_alliance() {
        assert_eq!(team_for_race(1), 469);
        assert_eq!(team_for_race(3), 469);
        assert_eq!(team_for_race(4), 469);
        assert_eq!(team_for_race(7), 469);
        assert_eq!(team_for_race(2), 67);
        assert_eq!(team_for_race(5), 67);
        assert_eq!(team_for_race(6), 67);
        assert_eq!(team_for_race(8), 67);
        assert_eq!(
            team_for_race(200),
            469,
            "an unrecognized race byte defaults Alliance"
        );
    }
}
