//!
//! `GraveyardLoc`/`GraveyardZone` stay `pub` tables and every accessor/type here is re-exported at
//! the crate root via `pub use graveyard::*;` in `lib.rs` (mirroring `world.rs`'s own `pub use`) —
//! every existing `crate::game_graveyard`/`crate::GraveyardLoc` path (seed.rs, debug.rs) is
//! byte-identical after the move.

use spacetimedb::{table, ReducerContext, Table};

/// One `WorldSafeLocs.dbc` row (a graveyard's fixed position) — imported by the importer's `--dbc`
/// mode (see `importer/src/dbc.rs::graveyard_sql`). Replaces the hardcoded
/// `{NORTHSHIRE, GOLDSHIRE, ...}` consts below as the primary data source once imported; those
/// consts remain as the no-import fallback (`nearest`), and `seed.rs` row-seeds the SAME five points
/// here too, mirroring the `game_start_position` seed/import precedent (init seeds, import
/// clear+reloads over it). No orientation column — `WorldSafeLocs.dbc` carries none (a graveyard
/// release always faces 0.0). No Timestamp → plain SQL. [static]
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

/// One cmangos `game_graveyard_zone` row — links a `game_graveyard.id` (`safe_loc_id`) to the zone
/// it serves, with an optional faction restriction (0 = both factions; else the cmangos team-faction
/// id, see `team_for_race`). Imported by the importer's `--dump` mode; see
/// `importer/src/main.rs::build_graveyard_zone_sql`. `resolve_graveyard` reads it via `by_zone` to
/// prefer a zone-linked graveyard over a merely-closer unlinked one (the cmangos release rule). No
/// Timestamp → plain SQL. [static]
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

// world_safe_locs id 105 — Northshire Abbey
const NORTHSHIRE: Graveyard = Graveyard {
    map: 0,
    x: -8935.33,
    y: -188.646,
    z: 80.4165,
    o: 2.72271,
};

// world_safe_locs id 106 — Goldshire
const GOLDSHIRE: Graveyard = Graveyard {
    map: 0,
    x: -9339.59,
    y: 171.73,
    z: 63.5258,
    o: 0.0,
};

// world_safe_locs id 854 — Eastvale Logging Camp
const EASTVALE: Graveyard = Graveyard {
    map: 0,
    x: -9552.73,
    y: -1374.84,
    z: 57.0867,
    o: 0.0,
};

// world_safe_locs id ≈80 [V], Sentinel Hill (Westfall, zone 40). Hand-added for
// the Westfall 1-20 slice; coords are an UNVERIFIED estimate (this sandbox has no reference
// world-database to read the real row from); orientation defaults to 0.0 (not sourced). CONFIRM
// x/y/z against your own imported world_safe_locs before relying on this for a live release.
const SENTINEL_HILL: Graveyard = Graveyard {
    map: 0,
    x: -10650.0, // [V]
    y: 1180.0,   // [V]
    z: 34.0,     // [V]
    o: 0.0,      // [V] — orientation not sourced
};

// world_safe_locs id ≈81 [V] — Westfall coast (the western shoreline graveyard, zone 40). Same
// provenance caveat as SENTINEL_HILL: coords UNVERIFIED.
const WESTFALL_COAST: Graveyard = Graveyard {
    map: 0,
    x: -11390.0, // [V]
    y: 1590.0,   // [V]
    z: 6.0,      // [V]
    o: 0.0,      // [V] — orientation not sourced
};

const STATIC_CANDIDATES: [Graveyard; 5] = [
    NORTHSHIRE,
    GOLDSHIRE,
    EASTVALE,
    SENTINEL_HILL,
    WESTFALL_COAST,
];

// ---- Instance-map release --------------------------------------------------
//

pub(crate) fn instance_release_zone(map_id: u32) -> Option<u32> {
    crate::instance::dungeon(map_id).map(|d| d.release_zone)
}

/// Static release floor for an instance map when NOTHING is imported (this sandbox's default
/// state): Deadmines releases at Sentinel Hill (the Westfall graveyard nearest the Moonbrook
/// entrance in spirit — the exact safe loc is `[V]`, same provenance caveat as the const itself).
/// NEVER fall through to `nearest(px, py)` for an instance map — cross-map 2-D distance against
/// the static consts is meaningless (see the section comment above). A field read
/// off `crate::instance::DUNGEON_MAPS` (see [`instance_release_zone`]).
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

/// The pure nearest-by-squared-2D-distance picker, shared by every candidate source (the static
/// fallback list AND the DB-resolved zone-linked/all-imported lists `resolve_graveyard` builds).
/// `None` for an empty slice — never panics; the caller chains to the next fallback.
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

/// Return the graveyard whose 2-D position is closest (squared distance) to `(px, py)`, among
/// ONLY the five hardcoded fallback consts (never touches the DB). Kept for
/// the unit tests below and as `resolve_graveyard`'s last-resort floor; live release code should
/// call `resolve_graveyard` instead so an imported `game_graveyard` table actually gets consulted.
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

/// The team-faction id `game_graveyard_zone.faction` carries, for a character's race. Moved to
/// `lyracore_shared::faction` when the mail faction gate needed it: that gate runs in the GATEWAY
/// (realm-core holds no characters), and a second copy of the race table over there would agree
/// with this one only until somebody added a race to one of them.
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

/// Zone-linked graveyards for `zone_id`, faction-filtered (`faction == 0 || faction == team`),
/// resolved to positions via `game_graveyard`. A `game_graveyard_zone` row whose `safe_loc_id` has
/// no matching `game_graveyard` row is silently dropped — an inconsistent import shouldn't panic a
/// release.
fn zone_linked(
    ctx: &ReducerContext,
    zone_id: u32,
    team: u32,
    map_filter: Option<u32>,
) -> Vec<Graveyard> {
    ctx.db
        .game_graveyard_zone()
        .by_zone()
        .filter(&zone_id)
        .filter(|gz| gz.faction == 0 || gz.faction == team)
        .filter_map(|gz| ctx.db.game_graveyard().id().find(gz.safe_loc_id))
        .filter(|g| map_filter.is_none_or(|m| g.map_id == m))
        .map(to_gy)
        .collect()
}

/// Every imported graveyard on `map_id`, regardless of zone — the fallback once zone-scoping
/// isn't available or didn't resolve (see `resolve_graveyard`). Map-only — same RESOLVED note
/// as `zone_linked`: only non-instance-map releases ever reach this chain.
fn all_on_map(ctx: &ReducerContext, map_id: u32) -> Vec<Graveyard> {
    ctx.db
        .game_graveyard()
        .iter()
        .filter(|g| g.map_id == map_id)
        .map(to_gy)
        .collect()
}

/// The real entry point for a release: resolve the nearest graveyard to `(x, y)` on `map_id` for
/// a player of `race`, per the fallback chain zone-linked → all-imported-on-map → static consts
/// (see `pick_graveyard`). Never panics on an empty DB — every step degrades gracefully; the
/// static consts are the final floor.
pub(crate) fn resolve_graveyard(
    ctx: &ReducerContext,
    map_id: u32,
    x: f32,
    y: f32,
    race: u8,
) -> Graveyard {
    let team = team_for_race(race);

    if let Some(zone_id) = instance_release_zone(map_id) {
        let linked = zone_linked(ctx, zone_id, team, None);
        if let Some(g) = pick_instance_graveyard(&linked, instance_static_fallback(map_id), x, y) {
            return g;
        }
    }
    let linked = match crate::terrain::zone_id_at(ctx, map_id, x, y) {
        Some(zone_id) => zone_linked(ctx, zone_id, team, Some(map_id)),
        None => Vec::new(),
    };
    let all = all_on_map(ctx, map_id);
    pick_graveyard(&linked, &all, x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadmines_release_resolves_cross_map_to_westfall_never_by_raw_distance() {
        // Map 36 is an instance map: its release zone is The Deadmines (1581 [V]) and its no-import
        // static floor is Sentinel Hill on MAP 0 — the release itself is cross-map by design
        // (corpse stays on 36; the ghost walks back in through the portal).
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

        // The pick order: an imported zone-linked external graveyard wins over the static floor;
        // with nothing imported the floor applies; a non-instance map (no floor, no links) yields
        // None so the caller falls through to the normal open-world chain.
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
        // Dying at each graveyard's own coordinates picks that graveyard (distance 0 beats the others).
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
        // A south-east Elwynn death picks Eastvale (~380yd) over Northshire (~990yd) and Goldshire
        // (~1180yd) — the exact wrong-direction corpse run `nearest` exists to avoid.
        let s = nearest(-9500.0, -1000.0);
        assert_eq!((s.x, s.y), (-9552.73, -1374.84));
        // a Westfall-ish death point (near Sentinel Hill) picks Sentinel Hill over
        // Eastvale — the nearest Elwynn graveyard is nowhere close once Westfall is in the candidate
        // set (Sentinel Hill ~95yd away vs Eastvale's ~2700yd), proving the two new graveyards
        // actually widen the release map rather than sitting unreachable in the candidate slice.
        let w = nearest(-10700.0, 1100.0);
        assert_eq!((w.x, w.y), (-10650.0, 1180.0));
    }

    #[test]
    fn pick_graveyard_prefers_a_zone_linked_graveyard_over_a_closer_unlinked_one() {
        // game_graveyard_zone's whole point is that a graveyard EXPLICITLY linked to
        // the death position's resolved zone wins even when an unlinked (or wrong-zone) graveyard
        // sits geometrically closer — the cmangos release rule.
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
        // No zone-linked candidates (empty zone-link set, e.g. an unresolved zone) → falls to `all`
        // (every imported graveyard on the map).
        let only_all = [Graveyard {
            map: 0,
            x: 42.0,
            y: 42.0,
            z: 0.0,
            o: 0.0,
        }];
        let picked = pick_graveyard(&[], &only_all, 0.0, 0.0);
        assert_eq!((picked.x, picked.y), (42.0, 42.0));
        // BOTH empty (nothing imported at all) → falls to the static consts (never panics), matching
        // `nearest` directly.
        let picked_static = pick_graveyard(&[], &[], -10700.0, 1100.0);
        let expected = nearest(-10700.0, 1100.0);
        assert_eq!((picked_static.x, picked_static.y), (expected.x, expected.y));
    }

    #[test]
    fn team_for_race_maps_alliance_and_horde_and_defaults_alliance() {
        assert_eq!(team_for_race(1), 469); // Human
        assert_eq!(team_for_race(3), 469); // Dwarf
        assert_eq!(team_for_race(4), 469); // Night Elf
        assert_eq!(team_for_race(7), 469); // Gnome
        assert_eq!(team_for_race(2), 67); // Orc
        assert_eq!(team_for_race(5), 67); // Undead
        assert_eq!(team_for_race(6), 67); // Tauren
        assert_eq!(team_for_race(8), 67); // Troll
        assert_eq!(
            team_for_race(200),
            469,
            "an unrecognized race byte defaults Alliance"
        );
    }
}
