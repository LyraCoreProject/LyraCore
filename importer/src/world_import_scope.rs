use anyhow::{bail, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorldImportProfile {
    AllianceEastern,
    AllianceKalimdor,
    AllianceSingle,
    Instances,
}

impl WorldImportProfile {
    pub(crate) const NAMES: &[&str] = &[
        "alliance-eastern",
        "alliance-kalimdor",
        "alliance-single",
        "instances",
    ];

    pub(crate) fn parse(name: &str) -> Result<Self> {
        match name {
            "alliance-eastern" => Ok(Self::AllianceEastern),
            "alliance-kalimdor" => Ok(Self::AllianceKalimdor),
            "alliance-single" => Ok(Self::AllianceSingle),
            "instances" => Ok(Self::Instances),
            _ => bail!(
                "--world-profile {name}: unknown profile (valid: {})",
                Self::NAMES.join(", ")
            ),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::AllianceEastern => "alliance-eastern",
            Self::AllianceKalimdor => "alliance-kalimdor",
            Self::AllianceSingle => "alliance-single",
            Self::Instances => "instances",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct WorldImportScope {
    name: String,
    pub(crate) bounded_slices: Vec<BoundedMapSlice>,
    pub(crate) whole_maps: Vec<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct BoundedMapSlice {
    pub(crate) name: String,
    pub(crate) map_id: i64,
    pub(crate) bounds: (f64, f64, f64, f64),
    pub(crate) sample: (f64, f64, f64),
    geometry: SliceGeometry,
}

#[derive(Clone, Debug)]
enum SliceGeometry {
    Rectangle,
    LegacyRectangle {
        exclude: Option<(f64, f64, f64, f64)>,
    },
    LegacySphere {
        radius: f64,
        exclude: Option<(f64, f64, f64, f64)>,
    },
}

impl WorldImportScope {
    pub(crate) fn canonical(profile: WorldImportProfile) -> Result<Self> {
        // Human bounds are the existing canonical importer rectangle. Dwarf/Gnome and Night Elf
        // bounds were narrowed with `--print-extents` against the local classic-db dump. Their
        // samples are `playercreateinfo` anchors or real creature rows emitted by that command.
        // Keeping the catalogue here lets later Verification refine data without changing callers.
        let eastern = || -> Result<Vec<BoundedMapSlice>> {
            Ok(vec![
                BoundedMapSlice::rectangle(
                    "human-elwynn-westfall-redridge",
                    0,
                    (-11_400.0, -8_000.0, -3_100.0, 2_000.0),
                    (-8_949.95, -132.493, 83.5312),
                )?,
                BoundedMapSlice::rectangle(
                    "dun-morogh",
                    0,
                    (-7_000.0, -4_500.0, -1_600.0, 1_600.0),
                    (-6_240.32, 331.03, 382.76),
                )?,
                BoundedMapSlice::rectangle(
                    "loch-modan",
                    0,
                    (-6_000.0, -4_000.0, -4_500.0, -1_601.0),
                    (-4_988.90, -2_958.24, 315.71),
                )?,
            ])
        };
        let kalimdor = || -> Result<Vec<BoundedMapSlice>> {
            Ok(vec![
                BoundedMapSlice::rectangle(
                    "teldrassil",
                    1,
                    (8_301.0, 12_000.0, -2_500.0, 3_500.0),
                    (10_311.30, 831.46, 1_326.41),
                )?,
                BoundedMapSlice::rectangle(
                    "darkshore",
                    1,
                    (3_500.0, 8_300.0, -2_500.0, 3_500.0),
                    (5_625.44, -485.95, 378.34),
                )?,
            ])
        };

        let (bounded_slices, whole_maps) = match profile {
            WorldImportProfile::AllianceEastern => (eastern()?, vec![]),
            WorldImportProfile::AllianceKalimdor => (kalimdor()?, vec![]),
            WorldImportProfile::AllianceSingle => {
                let mut slices = eastern()?;
                slices.extend(kalimdor()?);
                (slices, vec![36])
            }
            WorldImportProfile::Instances => (vec![], vec![36]),
        };
        Self::new(profile.name(), bounded_slices, whole_maps)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn legacy(
        map_id: i64,
        bounds: Option<(f64, f64, f64, f64)>,
        sample: (f64, f64, f64),
        radius: f64,
        exclude: Option<(f64, f64, f64, f64)>,
        whole_maps: Vec<i64>,
    ) -> Result<Self> {
        if !radius.is_finite() || radius <= 0.0 {
            bail!("--radius must be a positive finite number");
        }
        let exclude = exclude.map(normalize_bounds).transpose()?;
        let slice = match bounds {
            Some(bounds) => BoundedMapSlice::legacy_rectangle(
                format!("legacy-map-{map_id}"),
                map_id,
                bounds,
                sample,
                exclude,
            )?,
            None => BoundedMapSlice::legacy_sphere(
                format!("legacy-map-{map_id}"),
                map_id,
                sample,
                radius,
                exclude,
            )?,
        };
        Self::new("legacy", vec![slice], whole_maps)
    }

    fn new(
        name: impl Into<String>,
        bounded_slices: Vec<BoundedMapSlice>,
        mut whole_maps: Vec<i64>,
    ) -> Result<Self> {
        whole_maps.sort_unstable();
        whole_maps.dedup();
        if bounded_slices.is_empty() && whole_maps.is_empty() {
            bail!("world import scope is empty: add a bounded slice or whole map");
        }
        for map in &whole_maps {
            if bounded_slices.iter().any(|slice| slice.map_id == *map) {
                bail!("world import scope plans map {map} as both bounded and whole; choose one");
            }
        }
        Ok(Self {
            name: name.into(),
            bounded_slices,
            whole_maps,
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn contains(&self, map_id: i64, x: f64, y: f64, z: f64) -> bool {
        self.whole_maps.contains(&map_id)
            || self
                .bounded_slices
                .iter()
                .any(|slice| slice.contains(map_id, x, y, z))
    }

    pub(crate) fn contains_map(&self, map_id: i64) -> bool {
        self.whole_maps.contains(&map_id)
            || self
                .bounded_slices
                .iter()
                .any(|slice| slice.map_id == map_id)
    }
}

impl BoundedMapSlice {
    fn rectangle(
        name: impl Into<String>,
        map_id: i64,
        bounds: (f64, f64, f64, f64),
        sample: (f64, f64, f64),
    ) -> Result<Self> {
        let bounds = normalize_bounds(bounds)?;
        validate_sample(sample)?;
        if !point_in_bounds(bounds, sample.0, sample.1) {
            bail!("bounded slice sample must lie inside its rectangle");
        }
        Ok(Self {
            name: name.into(),
            map_id,
            bounds,
            sample,
            geometry: SliceGeometry::Rectangle,
        })
    }

    fn legacy_rectangle(
        name: impl Into<String>,
        map_id: i64,
        bounds: (f64, f64, f64, f64),
        sample: (f64, f64, f64),
        exclude: Option<(f64, f64, f64, f64)>,
    ) -> Result<Self> {
        let bounds = normalize_bounds(bounds)?;
        validate_sample(sample)?;
        if exclude.is_some_and(|excluded| bounds_covers(excluded, bounds)) {
            bail!("--exclude removes the entire --box; world import scope is empty");
        }
        Ok(Self {
            name: name.into(),
            map_id,
            bounds,
            sample,
            geometry: SliceGeometry::LegacyRectangle { exclude },
        })
    }

    fn legacy_sphere(
        name: impl Into<String>,
        map_id: i64,
        sample: (f64, f64, f64),
        radius: f64,
        exclude: Option<(f64, f64, f64, f64)>,
    ) -> Result<Self> {
        validate_sample(sample)?;
        let bounds = normalize_bounds((
            sample.0 - radius,
            sample.0 + radius,
            sample.1 - radius,
            sample.1 + radius,
        ))?;
        if exclude.is_some_and(|excluded| bounds_covers(excluded, bounds)) {
            bail!("--exclude removes the entire --radius slice; world import scope is empty");
        }
        Ok(Self {
            name: name.into(),
            map_id,
            bounds,
            sample,
            geometry: SliceGeometry::LegacySphere { radius, exclude },
        })
    }

    pub(crate) fn contains(&self, map_id: i64, x: f64, y: f64, z: f64) -> bool {
        if map_id != self.map_id || !x.is_finite() || !y.is_finite() || !z.is_finite() {
            return false;
        }
        match self.geometry {
            SliceGeometry::Rectangle => point_in_bounds(self.bounds, x, y),
            SliceGeometry::LegacyRectangle { exclude } => {
                point_in_bounds(self.bounds, x, y)
                    && !exclude.is_some_and(|bounds| point_in_bounds(bounds, x, y))
            }
            SliceGeometry::LegacySphere { radius, exclude } => {
                let (cx, cy, cz) = self.sample;
                (x - cx).powi(2) + (y - cy).powi(2) + (z - cz).powi(2) <= radius * radius
                    && !exclude.is_some_and(|bounds| point_in_bounds(bounds, x, y))
            }
        }
    }
}

fn normalize_bounds((x0, x1, y0, y1): (f64, f64, f64, f64)) -> Result<(f64, f64, f64, f64)> {
    if ![x0, x1, y0, y1].iter().all(|value| value.is_finite()) {
        bail!("world import bounds must contain only finite numbers");
    }
    let bounds = (x0.min(x1), x0.max(x1), y0.min(y1), y0.max(y1));
    if bounds.0 == bounds.1 || bounds.2 == bounds.3 {
        bail!("world import bounds must have positive width and height");
    }
    Ok(bounds)
}

fn validate_sample((x, y, z): (f64, f64, f64)) -> Result<()> {
    if ![x, y, z].iter().all(|value| value.is_finite()) {
        bail!("world import sample must contain only finite numbers");
    }
    Ok(())
}

fn point_in_bounds((x0, x1, y0, y1): (f64, f64, f64, f64), x: f64, y: f64) -> bool {
    x >= x0 && x <= x1 && y >= y0 && y <= y1
}

fn bounds_covers(
    (outer_x0, outer_x1, outer_y0, outer_y1): (f64, f64, f64, f64),
    (inner_x0, inner_x1, inner_y0, inner_y1): (f64, f64, f64, f64),
) -> bool {
    outer_x0 <= inner_x0 && outer_x1 >= inner_x1 && outer_y0 <= inner_y0 && outer_y1 >= inner_y1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_are_normalized_and_membership_is_map_fenced() {
        let slice =
            BoundedMapSlice::rectangle("test", 0, (10.0, -10.0, 20.0, -20.0), (0.0, 0.0, 1.0))
                .expect("valid slice");
        assert_eq!(slice.bounds, (-10.0, 10.0, -20.0, 20.0));
        assert!(slice.contains(0, 5.0, 5.0, 999.0));
        assert!(!slice.contains(1, 5.0, 5.0, 999.0));
        assert!(!slice.contains(0, 11.0, 5.0, 999.0));
    }

    #[test]
    fn canonical_profiles_have_stable_names_and_shapes() {
        let eastern = WorldImportScope::canonical(WorldImportProfile::AllianceEastern)
            .expect("eastern profile");
        assert_eq!(eastern.name(), "alliance-eastern");
        assert_eq!(eastern.bounded_slices.len(), 3);
        assert!(eastern.whole_maps.is_empty());

        let kalimdor = WorldImportScope::canonical(WorldImportProfile::AllianceKalimdor)
            .expect("kalimdor profile");
        assert_eq!(kalimdor.bounded_slices.len(), 2);
        assert!(kalimdor
            .bounded_slices
            .iter()
            .all(|slice| slice.map_id == 1));

        let single = WorldImportScope::canonical(WorldImportProfile::AllianceSingle)
            .expect("single profile");
        assert_eq!(single.bounded_slices.len(), 5);
        assert_eq!(single.whole_maps, vec![36]);

        let instances =
            WorldImportScope::canonical(WorldImportProfile::Instances).expect("instances profile");
        assert!(instances.bounded_slices.is_empty());
        assert_eq!(instances.whole_maps, vec![36]);
    }

    #[test]
    fn disjoint_slices_on_one_map_union_without_importing_the_gap() {
        let scope = WorldImportScope::new(
            "test",
            vec![
                BoundedMapSlice::rectangle("west", 0, (-20.0, -10.0, -5.0, 5.0), (-15.0, 0.0, 1.0))
                    .expect("west slice"),
                BoundedMapSlice::rectangle("east", 0, (10.0, 20.0, -5.0, 5.0), (15.0, 0.0, 1.0))
                    .expect("east slice"),
            ],
            vec![],
        )
        .expect("valid scope");
        assert!(scope.contains(0, -15.0, 0.0, 0.0));
        assert!(scope.contains(0, 15.0, 0.0, 0.0));
        assert!(!scope.contains(0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn slices_on_two_maps_and_a_whole_map_share_one_scope() {
        let scope = WorldImportScope::new(
            "test",
            vec![
                BoundedMapSlice::rectangle("map-zero", 0, (-5.0, 5.0, -5.0, 5.0), (0.0, 0.0, 1.0))
                    .expect("map-zero slice"),
                BoundedMapSlice::rectangle("map-one", 1, (-5.0, 5.0, -5.0, 5.0), (0.0, 0.0, 1.0))
                    .expect("map-one slice"),
            ],
            vec![36],
        )
        .expect("valid scope");
        assert!(scope.contains(0, 0.0, 0.0, 0.0));
        assert!(scope.contains(1, 0.0, 0.0, 0.0));
        assert!(scope.contains(36, 90_000.0, -90_000.0, 0.0));
        assert!(!scope.contains(2, 0.0, 0.0, 0.0));
    }

    #[test]
    fn legacy_geometry_preserves_sphere_and_exclusion_behavior() {
        let scope = WorldImportScope::legacy(
            0,
            None,
            (0.0, 0.0, 0.0),
            10.0,
            Some((-1.0, 1.0, -1.0, 1.0)),
            vec![36],
        )
        .expect("legacy scope");
        assert!(scope.contains(0, 6.0, 6.0, 0.0));
        assert!(!scope.contains(0, 8.0, 8.0, 0.0));
        assert!(!scope.contains(0, 0.0, 0.0, 0.0));
        assert!(scope.contains(36, 8.0, 8.0, 8.0));
    }

    #[test]
    fn malformed_or_empty_scopes_are_refused() {
        assert!(
            BoundedMapSlice::rectangle("flat", 0, (1.0, 1.0, 0.0, 2.0), (1.0, 1.0, 0.0)).is_err()
        );
        assert!(WorldImportScope::new("empty", vec![], vec![]).is_err());
        assert!(WorldImportScope::legacy(
            0,
            Some((-1.0, 1.0, -1.0, 1.0)),
            (0.0, 0.0, 0.0),
            10.0,
            Some((-2.0, 2.0, -2.0, 2.0)),
            vec![]
        )
        .is_err());
    }
}
