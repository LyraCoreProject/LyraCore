use anyhow::{bail, Result};

use crate::dump::{cr, parse_table, pc};

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
) -> Result<Option<Extents>> {
    let mut count = 0u64;
    let mut min = (f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut kept: Vec<(f64, f64, f64)> = Vec::new();
    for row in parse_table(dump, "creature")?.iter() {
        let m: i64 = row.number(cr::MAP)?;
        if m != map {
            continue;
        }
        let x: f64 = row.number(cr::X)?;
        let y: f64 = row.number(cr::Y)?;
        let z: f64 = row.number(cr::Z)?;
        if matches!(bbox, Some((x0, x1, y0, y1)) if x < x0 || x > x1 || y < y0 || y > y1) {
            continue;
        }
        min = (min.0.min(x), min.1.min(y), min.2.min(z));
        max = (max.0.max(x), max.1.max(y), max.2.max(z));
        kept.push((x, y, z));
        count += 1;
    }
    if count == 0 {
        return Ok(None);
    }
    let (mid_x, mid_y) = ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0);
    let anchor = *kept
        .iter()
        .min_by(|a, b| {
            let d = |p: &(f64, f64, f64)| (p.0 - mid_x).powi(2) + (p.1 - mid_y).powi(2);
            d(a).total_cmp(&d(b))
        })
        .expect("count > 0 ⇒ kept is non-empty");
    Ok(Some(Extents {
        count,
        min,
        max,
        anchor,
    }))
}

type StartPosition = (u32, u32, u32, f64, f64, f64);

fn start_positions_on_map(
    dump: &(impl crate::dump::TableRows + ?Sized),
    map: i64,
) -> Result<Vec<StartPosition>> {
    parse_table(dump, "playercreateinfo")?
        .iter()
        .map(|r| -> Result<_> {
            if r.number::<i64>(pc::MAP)? != map {
                return Ok(None);
            }
            Ok(Some((
                r.number(pc::RACE)?,
                r.number(pc::CLASS)?,
                r.number(pc::ZONE)?,
                r.number(pc::X)?,
                r.number(pc::Y)?,
                r.number(pc::Z)?,
            )))
        })
        .filter_map(Result::transpose)
        .collect::<Result<_>>()
}

pub(crate) fn print_extents(
    dump_path: &str,
    map: i64,
    bbox: Option<(f64, f64, f64, f64)>,
) -> Result<()> {
    let dump = &crate::world_source::WorldSource::open(std::path::Path::new(dump_path))?;
    let scope = match bbox {
        Some((x0, x1, y0, y1)) => format!(" inside --box {x0:.0},{x1:.0},{y0:.0},{y1:.0}"),
        None => String::new(),
    };
    let Some(e) = compute_extents(dump, map, bbox)? else {
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
    let starts = start_positions_on_map(dump, map)?;
    if starts.is_empty() {
        println!("  start positions on map {map}: none in playercreateinfo (not a starting continent, or the table is absent from this dump)");
    } else {
        println!("  start positions on map {map} (source playercreateinfo — the starting-zone anchors to draw a box around):");
        for (race, class, zone, x, y, z) in starts {
            println!("    race {race} class {class}  zone {zone}  ({x:.2},{y:.2},{z:.2})");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> crate::world_source::WorldSource {
        crate::world_source::fixture_sql("CREATE TABLE creature (
            guid INTEGER PRIMARY KEY, map INTEGER, position_x REAL, position_y REAL, position_z REAL,
            patch_min INTEGER DEFAULT 0, patch_max INTEGER DEFAULT 10);
            INSERT INTO creature (guid,map,position_x,position_y,position_z) VALUES
              (1,0,-11000,1000,10),(2,0,-9000,-500,50),(3,0,-10000,2000,-5),
              (4,1,-600,-4200,38),(5,1,-500,-4100,40),(6,1,-550,-4150,39),(7,1,6000,500,10);
            CREATE TABLE playercreateinfo (race INTEGER, class INTEGER, map INTEGER, zone INTEGER,
                position_x REAL, position_y REAL, position_z REAL);
            INSERT INTO playercreateinfo VALUES (1,1,0,12,-8949.95,-132.493,83.5312),
                (2,1,1,14,-618.518,-4251.67,38.718),(6,1,1,215,-2917.58,-257.98,52.9968);")
    }

    #[test]
    fn extents_include_only_the_requested_map_and_box() {
        let source = source();
        let e = compute_extents(&source, 0, None).unwrap().unwrap();
        assert_eq!(e.count, 3);
        assert_eq!(e.min, (-11000.0, -500.0, -5.0));
        assert_eq!(e.max, (-9000.0, 2000.0, 50.0));
        assert!(compute_extents(&source, 7, None).unwrap().is_none());
        let e = compute_extents(&source, 1, Some((-700.0, -400.0, -4300.0, -4000.0)))
            .unwrap()
            .unwrap();
        assert_eq!(e.count, 3);
        assert_eq!(e.anchor, (-550.0, -4150.0, 39.0));
    }

    #[test]
    fn start_positions_are_listed_for_the_requested_map_only() {
        let source = source();
        assert_eq!(
            start_positions_on_map(&source, 0).unwrap(),
            vec![(1, 1, 12, -8949.95, -132.493, 83.5312)]
        );
        let kalimdor = start_positions_on_map(&source, 1).unwrap();
        assert_eq!(kalimdor.len(), 2);
        assert_eq!(kalimdor[0], (2, 1, 14, -618.518, -4251.67, 38.718));
        assert_eq!(kalimdor[1].2, 215);
        assert!(start_positions_on_map(&source, 36).unwrap().is_empty());
    }
}
