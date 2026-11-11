use std::collections::BTreeMap;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};

const PROFILES: &str = include_str!("../manifests/world-profiles.json");

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileData {
    format_version: u32,
    slices: BTreeMap<String, SliceData>,
    instance_slices: BTreeMap<String, InstanceSliceData>,
    forced_creatures: BTreeMap<String, Vec<u64>>,
    profiles: Vec<ProfileDefinition>,
    legacy_defaults: LegacyDefaults,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SliceData {
    map: i64,
    #[serde(rename = "box")]
    bounds: String,
    center: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct InstanceSliceData {
    map: u32,
    entry: String,
    exit: String,
    exit_radius: String,
    collar_cells: u16,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDefinition {
    name: String,
    primary_map: i64,
    #[serde(default)]
    all_maps: bool,
    #[serde(default)]
    all_templates: bool,
    #[serde(default)]
    slices: Vec<String>,
    #[serde(default)]
    whole_maps: Vec<i64>,
    #[serde(default)]
    instance_maps: bool,
    #[serde(default)]
    instance_slices: Vec<String>,
    forced_creatures: Option<String>,
    quest_catalogue: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyDefaults {
    slice: String,
    include_maps: Vec<i64>,
    forced_creatures: String,
}

fn profile_data() -> &'static ProfileData {
    static DATA: OnceLock<ProfileData> = OnceLock::new();
    DATA.get_or_init(|| {
        let data: ProfileData =
            serde_json::from_str(PROFILES).expect("manifests/world-profiles.json parses");
        assert_eq!(data.format_version, 1, "world profile format version");
        data
    })
}

/// A World Import Profile from `manifests/world-profiles.json`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WorldImportProfile(usize);

impl WorldImportProfile {
    pub(crate) fn names() -> impl Iterator<Item = &'static str> {
        profile_data()
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
    }

    pub(crate) fn parse(name: &str) -> Result<Self> {
        match profile_data()
            .profiles
            .iter()
            .position(|profile| profile.name == name)
        {
            Some(index) => Ok(Self(index)),
            None => bail!(
                "--world-profile {name}: unknown profile (valid: {})",
                Self::names().collect::<Vec<_>>().join(", ")
            ),
        }
    }

    fn definition(self) -> &'static ProfileDefinition {
        &profile_data().profiles[self.0]
    }

    pub(crate) fn name(self) -> &'static str {
        &self.definition().name
    }

    pub(crate) fn primary_map(self) -> i64 {
        self.definition().primary_map
    }

    pub(crate) fn imports_all_templates(self) -> bool {
        self.definition().all_templates
    }
}

/// `KEY=value` lines for `import-world.sh`'s default map-0 legacy slice.
pub(crate) fn legacy_defaults_for_shell() -> String {
    let data = profile_data();
    let defaults = &data.legacy_defaults;
    let join = |values: &[i64]| {
        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    };
    let creatures = forced_creature_set(&defaults.forced_creatures)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "BOX={}\nINCLUDE_MAPS={}\nINCLUDE_CREATURES={creatures}\n",
        slice_data(&defaults.slice).bounds,
        join(&defaults.include_maps)
    )
}

fn slice_data(name: &str) -> &'static SliceData {
    profile_data()
        .slices
        .get(name)
        .unwrap_or_else(|| panic!("world profile slice {name} is defined"))
}

fn forced_creature_set(name: &str) -> &'static [u64] {
    profile_data()
        .forced_creatures
        .get(name)
        .unwrap_or_else(|| panic!("forced creature set {name} is defined"))
}

fn coordinates<T: std::str::FromStr>(text: &str, count: usize) -> Result<Vec<T>> {
    let values = text
        .split(',')
        .map(|value| value.trim().parse::<T>().ok())
        .collect::<Option<Vec<_>>>()
        .with_context(|| format!("world profile coordinates {text:?} are not numbers"))?;
    if values.len() != count {
        bail!("world profile coordinates {text:?} need {count} numbers");
    }
    Ok(values)
}

#[derive(Clone, Debug)]
pub(crate) struct WorldImportScope {
    name: String,
    all_maps: bool,
    pub(crate) bounded_slices: Vec<BoundedMapSlice>,
    pub(crate) whole_maps: Vec<i64>,
    pub(crate) instance_vmap_slices: Vec<InstanceVmapSlice>,
    pub(crate) forced_creature_entries: Vec<u64>,
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
pub(crate) struct InstanceVmapSlice {
    pub(crate) name: String,
    pub(crate) map_id: u32,
    pub(crate) entry: [f32; 3],
    pub(crate) exit: [f32; 3],
    pub(crate) exit_radius: f32,
    pub(crate) collar_cells: u16,
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
    /// The scope whose quests and placed givers form this profile's quest catalogue.
    pub(crate) fn quest_catalogue(profile: WorldImportProfile) -> Result<Self> {
        let members = profile
            .definition()
            .quest_catalogue
            .iter()
            .map(|name| WorldImportProfile::parse(name))
            .collect::<Result<Vec<_>>>()?;
        if let [member] = members.as_slice() {
            return Self::canonical(*member);
        }
        let mut bounded_slices = Vec::new();
        let mut whole_maps = Vec::new();
        let mut forced_creature_entries = Vec::new();
        for member in members {
            let scope = Self::canonical(member)?;
            bounded_slices.extend(scope.bounded_slices);
            whole_maps.extend(scope.whole_maps);
            forced_creature_entries.extend(scope.forced_creature_entries);
        }
        Self::new(
            "quest-catalogue",
            bounded_slices,
            whole_maps,
            vec![],
            forced_creature_entries,
        )
    }

    pub(crate) fn canonical(profile: WorldImportProfile) -> Result<Self> {
        let definition = profile.definition();
        let bounded_slices = definition
            .slices
            .iter()
            .map(|name| {
                let slice = slice_data(name);
                let bounds = coordinates::<f64>(&slice.bounds, 4)?;
                let center = coordinates::<f64>(&slice.center, 3)?;
                BoundedMapSlice::rectangle(
                    name.as_str(),
                    slice.map,
                    (bounds[0], bounds[1], bounds[2], bounds[3]),
                    (center[0], center[1], center[2]),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let mut whole_maps = definition.whole_maps.clone();
        if definition.instance_maps {
            whole_maps.extend(
                lyracore_shared::instance::DUNGEON_MAPS
                    .iter()
                    .chain(lyracore_shared::instance::BATTLEGROUND_MAPS)
                    .map(|map| i64::from(*map)),
            );
        }
        let instance_vmap_slices = definition
            .instance_slices
            .iter()
            .map(|name| {
                let slice = profile_data()
                    .instance_slices
                    .get(name)
                    .with_context(|| format!("instance vmap slice {name} is not defined"))?;
                let entry = coordinates::<f32>(&slice.entry, 3)?;
                let exit = coordinates::<f32>(&slice.exit, 3)?;
                Ok(InstanceVmapSlice {
                    name: name.clone(),
                    map_id: slice.map,
                    entry: [entry[0], entry[1], entry[2]],
                    exit: [exit[0], exit[1], exit[2]],
                    exit_radius: coordinates::<f32>(&slice.exit_radius, 1)?[0],
                    collar_cells: slice.collar_cells,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let forced_creature_entries = definition
            .forced_creatures
            .as_deref()
            .map(|name| forced_creature_set(name).to_vec())
            .unwrap_or_default();
        let mut scope = Self::new(
            profile.name(),
            bounded_slices,
            whole_maps,
            instance_vmap_slices,
            forced_creature_entries,
        )?;
        scope.all_maps = definition.all_maps;
        Ok(scope)
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
        Self::new("legacy", vec![slice], whole_maps, vec![], vec![])
    }

    fn new(
        name: impl Into<String>,
        bounded_slices: Vec<BoundedMapSlice>,
        mut whole_maps: Vec<i64>,
        instance_vmap_slices: Vec<InstanceVmapSlice>,
        mut forced_creature_entries: Vec<u64>,
    ) -> Result<Self> {
        whole_maps.sort_unstable();
        whole_maps.dedup();
        forced_creature_entries.sort_unstable();
        forced_creature_entries.dedup();
        if bounded_slices.is_empty() && whole_maps.is_empty() {
            bail!("world import scope is empty: add a bounded slice or whole map");
        }
        for map in &whole_maps {
            if bounded_slices.iter().any(|slice| slice.map_id == *map) {
                bail!("world import scope plans map {map} as both bounded and whole; choose one");
            }
        }
        for slice in &instance_vmap_slices {
            if !whole_maps.contains(&i64::from(slice.map_id)) {
                bail!(
                    "instance vmap slice {} is outside whole map {}",
                    slice.name,
                    slice.map_id
                );
            }
            slice.cell_range()?;
        }
        Ok(Self {
            name: name.into(),
            all_maps: false,
            bounded_slices,
            whole_maps,
            instance_vmap_slices,
            forced_creature_entries,
        })
    }

    pub(crate) fn extend_forced_creatures(&mut self, entries: &[u64]) {
        self.forced_creature_entries.extend_from_slice(entries);
        self.forced_creature_entries.sort_unstable();
        self.forced_creature_entries.dedup();
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn contains(&self, map_id: i64, x: f64, y: f64, z: f64) -> bool {
        (self.all_maps && map_id >= 0)
            || self.whole_maps.contains(&map_id)
            || self
                .bounded_slices
                .iter()
                .any(|slice| slice.contains(map_id, x, y, z))
    }

    pub(crate) fn contains_map(&self, map_id: i64) -> bool {
        (self.all_maps && map_id >= 0)
            || self.whole_maps.contains(&map_id)
            || self
                .bounded_slices
                .iter()
                .any(|slice| slice.map_id == map_id)
    }

    /// Planned maps plus every in-scope map that holds a kept source creature or GameObject row.
    pub(crate) fn source_maps(
        &self,
        source: &(impl crate::dump::TableRows + ?Sized),
    ) -> Result<std::collections::BTreeSet<i64>> {
        let mut maps: std::collections::BTreeSet<i64> = self
            .whole_maps
            .iter()
            .copied()
            .chain(self.bounded_slices.iter().map(|slice| slice.map_id))
            .collect();
        for row in source.rows("creature")?.iter() {
            if row.number::<u32>("spawn_flags")? & 2 != 0 || row.number::<u32>("id")? == 0 {
                continue;
            }
            maps.insert(row.number::<i64>("map")?);
        }
        for row in source.rows("gameobject")?.iter() {
            maps.insert(row.number::<i64>("map")?);
        }
        maps.retain(|map| self.contains_map(*map));
        Ok(maps)
    }
}

impl InstanceVmapSlice {
    pub(crate) fn selection_identity(&self) -> Result<String> {
        let (x0, x1, y0, y1) = self.cell_range()?;
        Ok(format!(
            "map={};instance-slice={};cell_x={x0}..{x1};cell_y={y0}..{y1};entry={:.4},{:.4},{:.4};exit={:.4},{:.4},{:.4};exit_radius={:.1};collar={}",
            self.map_id,
            self.name,
            self.entry[0],
            self.entry[1],
            self.entry[2],
            self.exit[0],
            self.exit[1],
            self.exit[2],
            self.exit_radius,
            self.collar_cells
        ))
    }

    pub(crate) fn cell_range(&self) -> Result<(u16, u16, u16, u16)> {
        if !self.exit_radius.is_finite() || self.exit_radius < 0.0 {
            bail!("instance vmap exit radius must be a finite non-negative number");
        }
        if !self
            .entry
            .iter()
            .chain(&self.exit)
            .all(|value| value.is_finite())
        {
            bail!("instance vmap route points must contain only finite numbers");
        }
        let mut cell_x = Vec::new();
        let mut cell_y = Vec::new();
        for x in [
            self.entry[0],
            self.exit[0] - self.exit_radius,
            self.exit[0] + self.exit_radius,
        ] {
            cell_x.push(
                lyracore_shared::terrain::cell_index(x)
                    .context("instance vmap route x is outside the map square")?,
            );
        }
        for y in [
            self.entry[1],
            self.exit[1] - self.exit_radius,
            self.exit[1] + self.exit_radius,
        ] {
            cell_y.push(
                lyracore_shared::terrain::cell_index(y)
                    .context("instance vmap route y is outside the map square")?,
            );
        }
        let x0 = cell_x.iter().copied().min().unwrap();
        let x1 = cell_x.iter().copied().max().unwrap();
        let y0 = cell_y.iter().copied().min().unwrap();
        let y1 = cell_y.iter().copied().max().unwrap();
        let collar = self.collar_cells;
        Ok((
            x0.checked_sub(collar)
                .context("instance vmap collar leaves the map square")?,
            x1.checked_add(collar)
                .filter(|cell| *cell < 1024)
                .context("instance vmap collar leaves the map square")?,
            y0.checked_sub(collar)
                .context("instance vmap collar leaves the map square")?,
            y1.checked_add(collar)
                .filter(|cell| *cell < 1024)
                .context("instance vmap collar leaves the map square")?,
        ))
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
    fn every_profile_resolves_and_the_legacy_defaults_are_the_human_slice() {
        for name in WorldImportProfile::names() {
            let profile = WorldImportProfile::parse(name).unwrap();
            assert_eq!(WorldImportScope::canonical(profile).unwrap().name(), name);
            WorldImportScope::quest_catalogue(profile).unwrap();
        }
        assert_eq!(
            legacy_defaults_for_shell(),
            "BOX=-11400,-8000,-3100,2000\nINCLUDE_MAPS=36\nINCLUDE_CREATURES=344,11406,266,415,1343,6966,5165,6166,6569,5149\n"
        );
    }

    #[test]
    fn canonical_profiles_have_stable_names_and_shapes() {
        let eastern =
            WorldImportScope::canonical(WorldImportProfile::parse("alliance-eastern").unwrap())
                .expect("eastern profile");
        assert_eq!(eastern.name(), "alliance-eastern");
        assert_eq!(eastern.bounded_slices.len(), 3);
        assert_eq!(eastern.whole_maps, vec![369]);
        assert!(eastern.instance_vmap_slices.is_empty());
        assert_eq!(
            eastern.forced_creature_entries,
            vec![266, 344, 415, 1_343, 5_149, 5_165, 6_166, 6_569, 6_966, 11_406]
        );

        let kalimdor =
            WorldImportScope::canonical(WorldImportProfile::parse("alliance-kalimdor").unwrap())
                .expect("kalimdor profile");
        assert_eq!(kalimdor.bounded_slices.len(), 2);
        assert!(kalimdor
            .bounded_slices
            .iter()
            .all(|slice| slice.map_id == 1));
        assert!(kalimdor.instance_vmap_slices.is_empty());

        let single =
            WorldImportScope::canonical(WorldImportProfile::parse("alliance-single").unwrap())
                .expect("single profile");
        assert_eq!(single.bounded_slices.len(), 5);
        assert_eq!(single.whole_maps, vec![36, 369]);
        assert_eq!(single.instance_vmap_slices.len(), 1);
        assert_eq!(
            single.forced_creature_entries,
            eastern.forced_creature_entries
        );

        let instances =
            WorldImportScope::canonical(WorldImportProfile::parse("instances").unwrap())
                .expect("instances profile");
        assert!(instances.bounded_slices.is_empty());
        assert_eq!(instances.whole_maps, vec![36]);
        assert_eq!(instances.instance_vmap_slices.len(), 1);
        let route = &instances.instance_vmap_slices[0];
        assert_eq!(route.name, "deadmines-entry-exit");
        assert_eq!(route.map_id, 36);
        assert_eq!(
            route.cell_range().expect("route cell collar"),
            (511, 513, 522, 524)
        );
        assert_eq!(
            route.selection_identity().expect("selection identity"),
            "map=36;instance-slice=deadmines-entry-exit;cell_x=511..513;cell_y=522..524;entry=-14.5732,-385.4750,62.4561;exit=-14.3628,-393.3800,64.5605;exit_radius=6.0;collar=1"
        );
        assert!(instances.forced_creature_entries.is_empty());
    }

    #[test]
    fn full_profiles_partition_spawns_and_share_all_definitions() {
        let all =
            WorldImportScope::canonical(WorldImportProfile::parse("full-world").unwrap()).unwrap();
        assert!(all.contains(999, 50_000.0, 50_000.0, 0.0));
        assert!(!all.contains(-1, 0.0, 0.0, 0.0));
        let eastern =
            WorldImportScope::canonical(WorldImportProfile::parse("full-eastern").unwrap())
                .unwrap();
        let kalimdor =
            WorldImportScope::canonical(WorldImportProfile::parse("full-kalimdor").unwrap())
                .unwrap();
        let instances =
            WorldImportScope::canonical(WorldImportProfile::parse("full-instances").unwrap())
                .unwrap();
        for map in [0, 1, 369, 449, 450]
            .into_iter()
            .chain(lyracore_shared::instance::DUNGEON_MAPS.iter().copied())
            .chain(lyracore_shared::instance::BATTLEGROUND_MAPS.iter().copied())
        {
            let owners = [&eastern, &kalimdor, &instances]
                .iter()
                .filter(|scope| scope.contains(i64::from(map), 0.0, 0.0, 0.0))
                .count();
            assert_eq!(owners, 1, "map {map}");
        }
        for scope in [&all, &eastern, &kalimdor] {
            for slice in &scope.bounded_slices {
                let (x0, x1, y0, y1) =
                    crate::terrain::slice_cell_range(Some(slice.bounds), slice.sample, 0.0);
                assert_eq!((x0, x1, y0, y1), (0, 1023, 0, 1023));
            }
        }
    }

    #[test]
    fn source_maps_follow_kept_source_rows_only_for_the_all_maps_profile() {
        let source = crate::world_source::fixture_sql(
            "CREATE TABLE creature (guid INTEGER PRIMARY KEY, map INTEGER, id INTEGER DEFAULT 1,
                spawn_flags INTEGER DEFAULT 0, patch_min INTEGER DEFAULT 0,
                patch_max INTEGER DEFAULT 10);
            INSERT INTO creature (guid,map) VALUES (1,0),(2,269),(3,451),(4,1);
            INSERT INTO creature VALUES (5,77,1,0,11,11),(6,78,0,0,0,10),(7,79,1,2,0,10);
            CREATE TABLE gameobject (guid INTEGER PRIMARY KEY, map INTEGER,
                patch_min INTEGER DEFAULT 0, patch_max INTEGER DEFAULT 10);
            INSERT INTO gameobject (guid,map) VALUES (1,13),(2,-1);",
        );
        let maps = |profile| {
            WorldImportScope::canonical(profile)
                .unwrap()
                .source_maps(&source)
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>()
        };
        let mut full_world = vec![0, 1, 13, 269, 451];
        full_world.extend(
            lyracore_shared::instance::DUNGEON_MAPS
                .iter()
                .chain(lyracore_shared::instance::BATTLEGROUND_MAPS)
                .map(|map| i64::from(*map)),
        );
        full_world.sort_unstable();
        full_world.dedup();
        assert_eq!(
            maps(WorldImportProfile::parse("full-world").unwrap()),
            full_world
        );
        assert_eq!(
            maps(WorldImportProfile::parse("full-eastern").unwrap()),
            vec![0, 369, 449]
        );
        assert_eq!(
            maps(WorldImportProfile::parse("full-kalimdor").unwrap()),
            vec![1, 450]
        );
        assert_eq!(
            maps(WorldImportProfile::parse("alliance-single").unwrap()),
            vec![0, 1, 36, 369]
        );
    }

    #[test]
    fn starting_profiles_preserve_alliance_content_and_add_horde_starts() {
        for (base, expanded, additions) in [
            (
                WorldImportProfile::parse("alliance-eastern").unwrap(),
                WorldImportProfile::parse("starting-eastern").unwrap(),
                vec![(0, 1_676.35, 1_677.45, 121.67)],
            ),
            (
                WorldImportProfile::parse("alliance-kalimdor").unwrap(),
                WorldImportProfile::parse("starting-kalimdor").unwrap(),
                vec![
                    (1, -618.518, -4_251.67, 38.718),
                    (1, -2_917.58, -257.98, 52.9968),
                ],
            ),
        ] {
            let before = WorldImportScope::canonical(base).unwrap();
            let after = WorldImportScope::canonical(expanded).unwrap();
            assert_eq!(
                after.forced_creature_entries,
                before.forced_creature_entries
            );
            assert_eq!(after.whole_maps, before.whole_maps);
            for slice in before.bounded_slices {
                assert!(after.bounded_slices.iter().any(|kept| {
                    kept.name == slice.name
                        && kept.map_id == slice.map_id
                        && kept.bounds == slice.bounds
                }));
            }
            for (map, x, y, z) in additions {
                assert!(after
                    .bounded_slices
                    .iter()
                    .any(|slice| slice.contains(map, x, y, z)));
                assert!(!after
                    .bounded_slices
                    .iter()
                    .any(|slice| slice.contains(36, x, y, z)));
            }
        }
    }

    #[test]
    fn loch_modan_anchor_is_the_thelsamar_road_spawn_inside_its_own_slice() {
        let kadrell = (-5_367.58, -2_936.21, 327.64);
        let eastern =
            WorldImportScope::canonical(WorldImportProfile::parse("alliance-eastern").unwrap())
                .expect("eastern profile");
        let loch_modan = eastern
            .bounded_slices
            .iter()
            .find(|slice| slice.name == "loch-modan")
            .expect("loch-modan slice");
        assert_eq!(loch_modan.map_id, 0);
        assert_eq!(loch_modan.sample, kadrell);
        assert!(loch_modan.contains(0, kadrell.0, kadrell.1, kadrell.2));
        let owners: Vec<&str> = eastern
            .bounded_slices
            .iter()
            .filter(|slice| slice.contains(0, kadrell.0, kadrell.1, kadrell.2))
            .map(|slice| slice.name.as_str())
            .collect();
        assert_eq!(owners, vec!["loch-modan"]);
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
            vec![],
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
            vec![],
            vec![],
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
        assert_eq!(
            BoundedMapSlice::rectangle("flat", 0, (1.0, 1.0, 0.0, 2.0), (1.0, 1.0, 0.0))
                .unwrap_err()
                .to_string(),
            "world import bounds must have positive width and height"
        );
        assert_eq!(
            WorldImportScope::new("empty", vec![], vec![], vec![], vec![])
                .unwrap_err()
                .to_string(),
            "world import scope is empty: add a bounded slice or whole map"
        );
        assert_eq!(
            WorldImportScope::legacy(
                0,
                Some((-1.0, 1.0, -1.0, 1.0)),
                (0.0, 0.0, 0.0),
                10.0,
                Some((-2.0, 2.0, -2.0, 2.0)),
                vec![]
            )
            .unwrap_err()
            .to_string(),
            "--exclude removes the entire --box; world import scope is empty"
        );
    }
}
