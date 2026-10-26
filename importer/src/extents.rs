use anyhow::{bail, Result};

use crate::dump::{cr, field, parse_table, pc, read_dump};

struct Extents {
    count: u64,
    min: (f64, f64, f64),
    max: (f64, f64, f64),
    anchor: (f64, f64, f64),
}

fn compute_extents(
    dump: &(impl crate::dump::TableRows + ?Sized),
    map: i64,
    bbox: Option<(f64, f64, f64, f64)>,
) -> Option<Extents> {
    let mut count = 0u64;
    let mut min = (f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut kept: Vec<(f64, f64, f64)> = Vec::new();
    for row in parse_table(dump, "creature") {
        let m: i64 = field(&row, cr::MAP).parse().unwrap_or(-1);
        if m != map {
            continue;
        }
        let x: f64 = field(&row, cr::X).parse().unwrap_or(0.0);
        let y: f64 = field(&row, cr::Y).parse().unwrap_or(0.0);
        let z: f64 = field(&row, cr::Z).parse().unwrap_or(0.0);
        if matches!(bbox, Some((x0, x1, y0, y1)) if x < x0 || x > x1 || y < y0 || y > y1) {
            continue;
        }
        min = (min.0.min(x), min.1.min(y), min.2.min(z));
        max = (max.0.max(x), max.1.max(y), max.2.max(z));
        kept.push((x, y, z));
        count += 1;
    }
    if count == 0 {
        return None;
    }
    let (mid_x, mid_y) = ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0);
    let anchor = *kept
        .iter()
        .min_by(|a, b| {
            let d = |p: &(f64, f64, f64)| (p.0 - mid_x).powi(2) + (p.1 - mid_y).powi(2);
            d(a).total_cmp(&d(b))
        })
        .expect("count > 0 ⇒ kept is non-empty");
    Some(Extents {
        count,
        min,
        max,
        anchor,
    })
}

fn start_positions_on_map(
    dump: &(impl crate::dump::TableRows + ?Sized),
    map: i64,
) -> Vec<(u32, u32, u32, f64, f64, f64)> {
    parse_table(dump, "playercreateinfo")
        .iter()
        .filter_map(|r| {
            let m: i64 = field(r, pc::MAP).parse().ok()?;
            if m != map {
                return None;
            }
            Some((
                field(r, pc::RACE).parse().ok()?,
                field(r, pc::CLASS).parse().ok()?,
                field(r, pc::ZONE).parse().ok()?,
                field(r, pc::X).parse().ok()?,
                field(r, pc::Y).parse().ok()?,
                field(r, pc::Z).parse().ok()?,
            ))
        })
        .collect()
}

pub(crate) fn print_extents(
    dump_path: &str,
    map: i64,
    bbox: Option<(f64, f64, f64, f64)>,
) -> Result<()> {
    let dump = &read_dump(dump_path)?;
    let scope = match bbox {
        Some((x0, x1, y0, y1)) => format!(" inside --box {x0:.0},{x1:.0},{y0:.0},{y1:.0}"),
        None => String::new(),
    };
    let Some(e) = compute_extents(dump, map, bbox) else {
        bail!("--print-extents: no creature spawns found on map {map}{scope} — check --map/--box / the dump path");
    };
    println!(
        "print-extents: map {map}{scope} — {} creature spawns",
        e.count
    );
    println!("  X: {:.2} .. {:.2}", e.min.0, e.max.0);
    println!("  Y: {:.2} .. {:.2}", e.min.1, e.max.1);
    println!("  Z: {:.2} .. {:.2}", e.min.2, e.max.2);
    println!(
        "  suggested --box \"{:.0},{:.0},{:.0},{:.0}\"   (X0,X1,Y0,Y1 — widen a little; a spawn extent is exact but a zone's terrain/GOs run past its outermost creature)",
        e.min.0, e.max.0, e.min.1, e.max.1
    );
    println!(
        "  suggested --center \"{:.2},{:.2},{:.2}\"   (the in-slice spawn nearest the rectangle's centre — the terrain/nav self-checks need a REAL ground point inside the box)",
        e.anchor.0, e.anchor.1, e.anchor.2
    );
    let starts = start_positions_on_map(dump, map);
    if starts.is_empty() {
        println!("  start positions on map {map}: none in playercreateinfo (not a starting continent, or the table is absent from this dump)");
    } else {
        println!("  start positions on map {map} (cmangos playercreateinfo — the starting-zone anchors to draw a box around):");
        for (race, class, zone, x, y, z) in starts {
            println!("    race {race} class {class}  zone {zone}  ({x:.2},{y:.2},{z:.2})");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_extents_derives_min_max_xyz_and_count_for_the_target_map_only() {
        let dump = "x INSERT INTO `creature` VALUES \
            (1,100,0,1,-11000.0,1000.0,10.0,0,300,300,0,0),\
            (2,101,0,1,-9000.0,-500.0,50.0,0,300,300,0,0),\
            (3,102,0,1,-10000.0,2000.0,-5.0,0,300,300,0,0),\
            (4,103,1,1,0.0,0.0,0.0,0,300,300,0,0); y";
        let e = compute_extents(dump, 0, None).expect("map 0 has spawns");
        assert_eq!(e.count, 3);
        assert_eq!(e.min, (-11000.0, -500.0, -5.0));
        assert_eq!(e.max, (-9000.0, 2000.0, 50.0));
        assert!(compute_extents(dump, 7, None).is_none());
        let k = compute_extents(dump, 1, None).expect("map 1 has one spawn");
        assert_eq!(
            (k.count, k.min, k.max),
            (1, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0))
        );
    }

    #[test]
    fn compute_extents_narrows_to_a_candidate_box_and_anchors_center_on_a_real_spawn() {
        let dump = "x INSERT INTO `creature` VALUES \
            (1,100,1,1,-600.0,-4200.0,38.0,0,300,300,0,0),\
            (2,101,1,1,-500.0,-4100.0,40.0,0,300,300,0,0),\
            (3,102,1,1,-550.0,-4150.0,39.0,0,300,300,0,0),\
            (4,103,1,1,6000.0,500.0,10.0,0,300,300,0,0); y";
        let e = compute_extents(dump, 1, Some((-700.0, -400.0, -4300.0, -4000.0)))
            .expect("in-box spawns");
        assert_eq!(
            e.count, 3,
            "the far-away map-1 spawn is outside the candidate box"
        );
        assert_eq!(e.min, (-600.0, -4200.0, 38.0));
        assert_eq!(e.max, (-500.0, -4100.0, 40.0));
        assert_eq!(e.anchor, (-550.0, -4150.0, 39.0));
        assert!(compute_extents(dump, 1, Some((0.0, 100.0, 0.0, 100.0))).is_none());
    }

    #[test]
    fn start_positions_are_listed_for_the_requested_map_only() {
        let dump = "x INSERT INTO `playercreateinfo` VALUES \
            (1,1,0,12,-8949.95,-132.493,83.5312,0),\
            (2,1,1,14,-618.518,-4251.67,38.718,0),\
            (6,1,1,215,-2917.58,-257.98,52.9968,0); y";
        assert_eq!(
            start_positions_on_map(dump, 0),
            vec![(1, 1, 12, -8949.95, -132.493, 83.5312)]
        );
        let kalimdor = start_positions_on_map(dump, 1);
        assert_eq!(
            kalimdor.len(),
            2,
            "both Kalimdor races are anchors: {kalimdor:?}"
        );
        assert_eq!(kalimdor[0], (2, 1, 14, -618.518, -4251.67, 38.718));
        assert_eq!(kalimdor[1].2, 215, "Mulgore's zone id rides along");
        assert!(
            start_positions_on_map(dump, 36).is_empty(),
            "an instance map has no start positions"
        );
    }
}
