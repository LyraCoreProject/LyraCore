mod census;
mod classic_columns;
mod collision;
mod creature_population;
mod dbc;
mod dbc_diff;
mod dump;
mod eventai;
mod eventai_presentation;
mod extents;
mod go_model;
mod gossip_conditions;
mod instance_vmap;
mod item_property;
mod m2_collision;
mod nav;
mod pack_client;
mod package_delta;
mod shard;
mod source_selection;
mod spell;
mod spell_dbc;
mod spell_snapshot;
mod talent;
mod terrain;
mod transport;
mod ui_transform;
mod vmangos_profile;
mod vmangos_spell;
mod vmap;
mod world_import;
mod world_import_scope;
mod world_source;

#[cfg(test)]
use lyracore_test_support as standalone_support;

use anyhow::{bail, Context, Result};
use shard::Shard;
use world_import::FAMILIES;
use world_import_scope::{WorldImportProfile, WorldImportScope};

struct Args {
    source: world_source::SourceKind,
    census: Option<String>,
    dbc_diff: bool,
    taxi_mounts: Option<String>,
    dump: Option<String>,
    dbc: Option<String>,
    terrain: Option<String>,
    dump_collision: Option<String>,
    nav: Option<String>,
    vmap: Option<String>,
    vmap_status: bool,
    vmap_prepare_coverage: Option<u64>,
    go_models: Option<String>,
    pack_client: Option<String>,
    pack_out: Option<String>,
    print_extents: bool,
    spells: bool,
    talents: bool,
    only: Vec<u32>,
    trainers: Vec<(u32, Vec<u32>)>,
    world_profile: Option<WorldImportProfile>,
    map: i64,
    include_maps: Vec<i64>,
    center: (f64, f64, f64),
    radius: f64,
    include_creatures: Vec<u64>,
    bbox: Option<(f64, f64, f64, f64)>,
    exclude: Option<(f64, f64, f64, f64)>,
    shard: Shard,
    apply: bool,
    family: Option<String>,
    source_sha: String,
    eventai_profile: String,
    packages: Option<String>,
    spell_snapshot: Option<String>,
}

fn parse_args() -> Result<Args> {
    parse_args_from(std::env::args().skip(1))
}

#[allow(clippy::too_many_lines)]
fn parse_args_from<I, S>(args: I) -> Result<Args>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut a = Args {
        source: world_source::SourceKind::ClassicDb,
        census: None,
        dbc_diff: false,
        dump: None,
        taxi_mounts: None,
        dbc: None,
        pack_client: None,
        pack_out: None,
        dump_collision: None,
        nav: None,
        vmap: None,
        vmap_status: false,
        vmap_prepare_coverage: None,
        go_models: None,
        terrain: None,
        print_extents: false,
        spells: false,
        spell_snapshot: None,
        talents: false,
        only: Vec::new(),
        trainers: Vec::new(),
        world_profile: None,
        map: 0,
        include_maps: Vec::new(),
        include_creatures: Vec::new(),
        center: (-8949.95, -132.493, 83.5312),
        radius: 180.0,
        bbox: None,
        exclude: None,
        shard: Shard {
            db: "lyracore".into(),
            server: "local".into(),
        },
        apply: false,
        family: None,
        source_sha: String::new(),
        eventai_profile: eventai::SOURCE_PROFILE_NAME.to_string(),
        packages: None,
    };
    let mut legacy_spatial_args: Vec<&'static str> = Vec::new();
    let mut explicit_source_profile = false;
    let mut it = args.into_iter().map(Into::into);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--source" => {
                a.source = it
                    .next()
                    .context("--source needs classic-db or vmangos")?
                    .parse()?
            }
            "--dbc-diff" => a.dbc_diff = true,
            "--census" => {
                a.census = Some(
                    it.next()
                        .context("--census needs the comparison classic-db dump path")?,
                )
            }
            "--taxi-mounts" => {
                a.taxi_mounts = Some(it.next().context("--taxi-mounts needs a dump path")?)
            }
            "--dump" => a.dump = Some(it.next().context("--dump needs a path")?),
            "--terrain" => {
                a.terrain = Some(it.next().context("--terrain needs the client Data/ dir")?)
            }
            "--dump-collision" => {
                a.dump_collision = Some(
                    it.next()
                        .context("--dump-collision needs the client Data/ dir")?,
                )
            }
            "--nav" => a.nav = Some(it.next().context("--nav needs the client Data/ dir")?),
            "--vmap" => a.vmap = Some(it.next().context("--vmap needs the client Data/ dir")?),
            "--vmap-status" => a.vmap_status = true,
            "--vmap-prepare-coverage" => {
                let v = it
                    .next()
                    .context("--vmap-prepare-coverage needs a generation id")?;
                a.vmap_prepare_coverage = Some(
                    v.parse()
                        .context("--vmap-prepare-coverage generation id must be a u64")?,
                );
            }
            "--go-models" => {
                a.go_models = Some(
                    it.next()
                        .context("--go-models needs the client Data/ dir")?,
                )
            }
            "--dbc" => a.dbc = Some(it.next().context("--dbc needs the client Data/ dir")?),
            "--pack-client" => {
                a.pack_client = Some(
                    it.next()
                        .context("--pack-client needs the client Data/ dir")?,
                )
            }
            "--pack-out" => a.pack_out = Some(it.next().context("--pack-out needs an output dir")?),
            "--print-extents" => a.print_extents = true,
            "--spells" => a.spells = true,
            "--talents" => a.talents = true,
            "--only" => {
                let v = it
                    .next()
                    .context("--only needs comma-separated spell ids, e.g. 7386,1160")?;
                a.only = v
                    .split(',')
                    .map(|s| s.trim().parse::<u32>())
                    .collect::<std::result::Result<_, _>>()
                    .context("--only wants comma-separated spell ids")?;
                if a.only.is_empty() {
                    bail!("--only given but no ids parsed");
                }
            }
            "--include-creatures" => {
                let v = it.next().context(
                    "--include-creatures needs comma-separated creature entries, e.g. 344,266,415",
                )?;
                a.include_creatures = v
                    .split(',')
                    .map(|s| s.trim().parse::<u64>())
                    .collect::<std::result::Result<_, _>>()
                    .context("--include-creatures wants comma-separated creature entries")?;
                if a.include_creatures.is_empty() {
                    bail!("--include-creatures given but no entries parsed");
                }
            }
            "--trainer" => {
                let v = it.next().context("--trainer needs <entry>=<id,id,…>")?;
                let (entry_s, ids_s) = v
                    .split_once('=')
                    .context("--trainer wants <entry>=<comma-ids>, e.g. 913=78,6674")?;
                let entry: u32 = entry_s
                    .trim()
                    .parse()
                    .context("--trainer <entry> must be a creature-template entry (u32)")?;
                let ids: Vec<u32> = ids_s
                    .split(',')
                    .map(|s| s.trim().parse::<u32>())
                    .collect::<std::result::Result<_, _>>()
                    .context("--trainer wants comma-separated spell ids after '='")?;
                if ids.is_empty() {
                    bail!("--trainer {entry}= given but no ids parsed");
                }
                a.trainers.push((entry, ids));
            }
            "--world-profile" => {
                let name = it.next().context("--world-profile needs a profile name")?;
                if a.world_profile.is_some() {
                    bail!("--world-profile may be given only once");
                }
                a.world_profile = Some(WorldImportProfile::parse(&name)?);
            }
            "--map" => {
                legacy_spatial_args.push("--map");
                a.map = it.next().context("--map")?.parse()?;
            }
            "--include-map" => {
                legacy_spatial_args.push("--include-map");
                let m: i64 = it
                    .next()
                    .context("--include-map <map id>")?
                    .parse()
                    .context("--include-map wants a map id (integer), e.g. 36")?;
                if !a.include_maps.contains(&m) {
                    a.include_maps.push(m);
                }
            }
            "--radius" => {
                legacy_spatial_args.push("--radius");
                a.radius = it.next().context("--radius")?.parse()?;
            }
            "--box" => {
                legacy_spatial_args.push("--box");
                let v = it.next().context("--box X0,X1,Y0,Y1")?;
                let p: Vec<f64> = v
                    .split(',')
                    .map(|s| s.trim().parse::<f64>())
                    .collect::<std::result::Result<_, _>>()
                    .context("--box wants four numbers: X0,X1,Y0,Y1")?;
                if p.len() != 4 {
                    bail!(
                        "--box wants exactly 4 comma-separated numbers, got {}",
                        p.len()
                    );
                }
                a.bbox = Some((
                    p[0].min(p[1]),
                    p[0].max(p[1]),
                    p[2].min(p[3]),
                    p[2].max(p[3]),
                ));
            }
            "--exclude" => {
                legacy_spatial_args.push("--exclude");
                let v = it.next().context("--exclude X0,X1,Y0,Y1")?;
                let p: Vec<f64> = v
                    .split(',')
                    .map(|s| s.trim().parse::<f64>())
                    .collect::<std::result::Result<_, _>>()
                    .context("--exclude wants four numbers: X0,X1,Y0,Y1")?;
                if p.len() != 4 {
                    bail!(
                        "--exclude wants exactly 4 comma-separated numbers, got {}",
                        p.len()
                    );
                }
                a.exclude = Some((
                    p[0].min(p[1]),
                    p[0].max(p[1]),
                    p[2].min(p[3]),
                    p[2].max(p[3]),
                ));
            }
            "--db" => a.shard.db = it.next().context("--db")?,
            "--server" => a.shard.server = it.next().context("--server")?,
            "--apply" => a.apply = true,
            "--family" => {
                let v = it.next().context("--family <name>")?;
                if !FAMILIES.contains(&v.as_str()) && !matches!(v.as_str(), "spell" | "dbc") {
                    bail!(
                        "--family {v}: unknown family (valid: {}, spell, dbc)",
                        FAMILIES.join(", ")
                    );
                }
                a.family = Some(v);
            }
            "--source-sha" => a.source_sha = it.next().context("--source-sha <sha>")?,
            "--spell-snapshot" => {
                a.spell_snapshot = Some(
                    it.next()
                        .context("--spell-snapshot needs the path to write the Base Snapshot to")?,
                )
            }
            "--packages" => {
                a.packages = Some(
                    it.next()
                        .context("--packages needs the enabled packages root")?,
                )
            }
            "--eventai-profile" => {
                explicit_source_profile = true;
                let profile = it.next().context("--eventai-profile <name>")?;
                if profile != vmangos_profile::NAME {
                    eventai::source_profile(&profile).map_err(anyhow::Error::msg)?;
                }
                a.eventai_profile = profile;
            }
            "--center" => {
                legacy_spatial_args.push("--center");
                let v = it.next().context("--center X,Y,Z")?;
                let p: Vec<f64> = v
                    .split(',')
                    .map(|s| s.trim().parse::<f64>())
                    .collect::<std::result::Result<_, _>>()
                    .context("--center wants three numbers: X,Y,Z")?;
                if p.len() != 3 {
                    bail!(
                        "--center wants exactly 3 comma-separated numbers, got {}",
                        p.len()
                    );
                }
                a.center = (p[0], p[1], p[2]);
            }
            other => bail!("unknown arg: {other}"),
        }
    }
    if let Some(profile) = a.world_profile {
        if !legacy_spatial_args.is_empty() {
            legacy_spatial_args.sort_unstable();
            legacy_spatial_args.dedup();
            bail!(
                "--world-profile {} cannot be mixed with legacy spatial arguments: {}",
                profile.name(),
                legacy_spatial_args.join(", ")
            );
        }
    }
    if a.taxi_mounts.is_some()
        && (a.dbc.is_none()
            || a.dump.is_some()
            || a.spells
            || a.talents
            || a.spell_snapshot.is_some()
            || a.terrain.is_some()
            || a.dump_collision.is_some()
            || a.nav.is_some()
            || a.vmap.is_some()
            || a.vmap_status
            || a.vmap_prepare_coverage.is_some()
            || a.go_models.is_some()
            || a.pack_client.is_some()
            || a.pack_out.is_some())
    {
        bail!("--taxi-mounts is only valid with standalone --dbc catalogue import");
    }
    if a.dump.is_none()
        && a.dbc.is_none()
        && a.pack_client.is_none()
        && a.pack_out.is_none()
        && a.terrain.is_none()
        && a.dump_collision.is_none()
        && a.nav.is_none()
        && a.vmap.is_none()
        && !a.vmap_status
        && a.vmap_prepare_coverage.is_none()
        && a.go_models.is_none()
    {
        bail!("need an input: --dump <classic-db .sql[.gz]>, --dbc <client Data/ dir>, --terrain <client Data/ dir>, --pack-client <client Data/ dir>, or --pack-out <dir>");
    }
    if a.pack_out.is_some() && a.apply {
        bail!("--pack-out writes its output directory directly and takes no --apply (--apply guards the operator's own client, which --pack-out never touches)");
    }
    if a.pack_out.is_some() && a.pack_client.is_some() {
        bail!("--pack-client and --pack-out are separate outputs and cannot run together: --pack-client writes into the operator's own client, --pack-out writes a distributable directory. Run one, then the other");
    }
    if a.go_models.is_some() && a.dump.is_none() {
        bail!("--go-models needs --dump too (gameobject_template is the template source)");
    }
    if a.print_extents && a.dump.is_none() {
        bail!("--print-extents is only valid with --dump (it reads the cmangos creature table)");
    }
    if !a.only.is_empty() && !a.spells {
        bail!("--only is only valid with --spells (the additive Spell.dbc allowlist)");
    }
    if a.spell_snapshot.is_some() {
        if a.dbc.is_none() || (a.dump.is_some() && a.source != world_source::SourceKind::Vmangos) {
            bail!("--spell-snapshot needs --dbc <client Data/ dir> and nothing else");
        }
        if a.spells
            || a.talents
            || a.apply
            || a.family.is_some()
            || a.census.is_some()
            || a.go_models.is_some()
            || a.terrain.is_some()
            || a.nav.is_some()
            || a.vmap.is_some()
            || a.vmap_status
            || a.vmap_prepare_coverage.is_some()
            || a.dump_collision.is_some()
            || a.pack_client.is_some()
            || a.pack_out.is_some()
            || a.print_extents
        {
            bail!(
                "--spell-snapshot writes the Base Snapshot and touches no database; run it on its \
                 own, then run the import separately"
            );
        }
    }
    if !a.include_creatures.is_empty() && a.dump.is_none() {
        bail!("--include-creatures is only valid with --dump (it force-imports cmangos creature rows)");
    }
    if a.family.is_some() && a.dump.is_none() {
        bail!("--family is only valid with --dump (it names one of the cmangos-dump ETL families)");
    }
    if !a.include_maps.is_empty() {
        if a.dump.is_none() {
            bail!("--include-map is only valid with --dump (it widens the cmangos content slice)");
        }
        if a.include_maps.contains(&a.map) {
            bail!(
                "--include-map {} duplicates --map {} (the primary map is already in the slice)",
                a.map,
                a.map
            );
        }
    }
    if a.dbc_diff {
        if a.source != world_source::SourceKind::Vmangos
            || a.dump.is_none()
            || a.dbc.is_none()
            || a.apply
            || a.family.is_some()
            || a.census.is_some()
            || a.spells
            || a.spell_snapshot.is_some()
            || a.talents
            || a.go_models.is_some()
            || a.pack_client.is_some()
            || a.pack_out.is_some()
            || a.terrain.is_some()
            || a.nav.is_some()
            || a.vmap.is_some()
            || a.vmap_status
            || a.vmap_prepare_coverage.is_some()
            || a.dump_collision.is_some()
            || a.print_extents
        {
            bail!("--dbc-diff needs only --source vmangos --dump <sqlite> --dbc <client Data> and optional --source-sha");
        }
    } else if a.census.is_some() {
        if a.source != world_source::SourceKind::Vmangos
            || a.dump.is_none()
            || a.dbc.is_none()
            || !matches!(
                a.family.as_deref(),
                Some(
                    "globals"
                        | "items"
                        | "loot"
                        | "gameobjects"
                        | "trainers"
                        | "quests"
                        | "gossip"
                        | "creatures"
                        | "casts"
                        | "creature-ai"
                        | "spell"
                        | "spellmeta"
                        | "dbc"
                )
            )
            || a.apply
            || a.pack_client.is_some()
            || a.pack_out.is_some()
            || a.terrain.is_some()
            || a.nav.is_some()
            || a.vmap.is_some()
            || a.vmap_status
            || a.vmap_prepare_coverage.is_some()
            || a.dump_collision.is_some()
            || a.go_models.is_some()
            || a.spells
            || a.talents
            || a.print_extents
        {
            bail!("--census needs --source vmangos --dump <sqlite> --dbc <client Data> --family spell|spellmeta|dbc|creatures|casts|creature-ai|globals|items|loot|gameobjects|trainers|quests|gossip, without --apply");
        }
    } else if a.source == world_source::SourceKind::Vmangos
        && (a.dump.is_none()
            || (a.go_models.is_none()
                && a.spell_snapshot.is_none()
                && !matches!(
                    a.family.as_deref(),
                    Some(
                        "globals"
                            | "creatures"
                            | "creature-ai"
                            | "casts"
                            | "spell"
                            | "spellmeta"
                            | "dbc"
                            | "items"
                            | "loot"
                            | "gameobjects"
                            | "trainers"
                            | "quests"
                            | "gossip"
                    )
                ))
            || (a.go_models.is_some() && a.family.is_some())
            || a.pack_client.is_some()
            || a.pack_out.is_some()
            || a.terrain.is_some()
            || a.nav.is_some()
            || a.vmap.is_some()
            || a.vmap_status
            || a.vmap_prepare_coverage.is_some()
            || a.dump_collision.is_some()
            || a.spells
            || a.talents
            || a.print_extents)
    {
        bail!("vmangos needs --dump <sqlite> and --family spell|spellmeta|dbc|creatures|creature-ai|casts|globals|items|loot|gameobjects|trainers|quests|gossip, or standalone --go-models <client Data>");
    }
    a.world_import_scope()?;
    if !a.trainers.is_empty() {
        if !a.spells {
            bail!(
                "--trainer is only valid with --spells (it derives required_level from Spell.dbc)"
            );
        }
        for (_entry, ids) in &a.trainers {
            for &id in ids {
                if !a.only.contains(&id) {
                    a.only.push(id);
                }
            }
        }
    }
    if a.source == world_source::SourceKind::Vmangos {
        if !explicit_source_profile {
            a.eventai_profile = vmangos_profile::NAME.into();
        }
        if a.eventai_profile != vmangos_profile::NAME {
            bail!("--source vmangos requires its vmangos Source Profile");
        }
        if a.apply
            && (a.source_sha.len() != 40
                || !a.source_sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            bail!("vmangos apply requires --source-sha with the actual snapshot commit");
        }
    } else if a.eventai_profile == vmangos_profile::NAME {
        bail!("the vmangos Source Profile requires --source vmangos");
    }
    Ok(a)
}

impl Args {
    fn world_import_scope(&self) -> Result<WorldImportScope> {
        let mut scope = match self.world_profile {
            Some(profile) => WorldImportScope::canonical(profile),
            None => WorldImportScope::legacy(
                self.map,
                self.bbox,
                self.center,
                self.radius,
                self.exclude,
                self.include_maps.clone(),
            ),
        }?;
        scope.extend_forced_creatures(&self.include_creatures);
        Ok(scope)
    }

    fn world_import_args(&self) -> Result<world_import::WorldImportArgs> {
        Ok(world_import::WorldImportArgs {
            source: self.source,
            dbc: self.dbc.clone(),
            family: self.family.clone(),
            world_profile: self.world_profile,
            scope: self.world_import_scope()?,
            include_creatures: self.include_creatures.clone(),
            eventai_profile: self.eventai_profile.clone(),
            packages: self.packages.clone(),
            source_sha: self.source_sha.clone(),
            apply: self.apply,
            shard: self.shard.clone(),
        })
    }

    fn spell_args(&self) -> Result<spell::SpellArgs> {
        Ok(spell::SpellArgs {
            only: self.only.clone(),
            trainers: self.trainers.clone(),
            source_sha: self.source_sha.clone(),
            packages: self.packages.clone(),
            scope: self.world_import_scope()?,
            apply: self.apply,
            shard: self.shard.clone(),
        })
    }
}

fn main() -> Result<()> {
    let args = parse_args()?;
    if args.dbc_diff {
        print!(
            "{}",
            dbc_diff::run(
                std::path::Path::new(args.dump.as_deref().context("missing source")?),
                std::path::Path::new(args.dbc.as_deref().context("missing client")?),
                &args.source_sha
            )?
        );
        return Ok(());
    }
    if let Some(classic) = &args.census {
        print!(
            "{}",
            census::run(
                std::path::Path::new(classic),
                std::path::Path::new(args.dump.as_deref().context("missing census source")?),
                args.dbc.as_deref().context("missing census client")?,
                &args.source_sha,
                &args.world_import_args()?,
            )?
        );
        return Ok(());
    }

    if let Some(dir) = args.pack_client.clone() {
        return pack_client::run(&dir, args.apply);
    }

    if let Some(dir) = args.pack_out.clone() {
        return pack_client::run_pack_out(&dir);
    }

    if let Some(dir) = &args.terrain {
        return terrain::run(dir, &args.world_import_scope()?, args.apply, &args.shard);
    }

    if let Some(dir) = &args.dump_collision {
        return collision::run(dir, args.map, args.center);
    }

    if let Some(dir) = &args.nav {
        return nav::run(dir, &args.world_import_scope()?, args.apply, &args.shard);
    }

    if args.vmap.is_some() || args.vmap_status {
        return vmap::run(&vmap::VmapArgs {
            data_dir: args.vmap.clone(),
            status: args.vmap_status,
            map: args.map,
            scope: args.world_import_scope()?,
            apply: args.apply,
            shard: args.shard.clone(),
        });
    }

    if let Some(generation_id) = args.vmap_prepare_coverage {
        return vmap::run_coverage(generation_id, args.apply, &args.shard);
    }

    if let Some(dir) = &args.go_models {
        return go_model::run(
            dir,
            args.dump.as_deref(),
            args.source,
            args.apply,
            &args.shard,
        );
    }

    if args.dump.is_none() {
        let dir = args
            .dbc
            .clone()
            .expect("parse_args guarantees --dump or --dbc");
        if args.talents {
            return talent::run_talents(&dir, args.apply, &args.shard);
        }
        if let Some(path) = args.spell_snapshot.clone() {
            return spell::run_spell_snapshot(&dir, &path);
        }
        if args.spells {
            return spell::run_spells(&dir, &args.spell_args()?);
        }
        return dbc::run(&dir, args.taxi_mounts.as_deref(), args.apply, &args.shard);
    }

    let dump_path = args
        .dump
        .clone()
        .expect("parse_args guarantees --dump here");

    if let Some(path) = &args.spell_snapshot {
        return spell::run_vmangos_snapshot(
            &dump_path,
            args.dbc.as_deref().context("missing client")?,
            path,
        );
    }

    if args.print_extents {
        return extents::print_extents(&dump_path, args.map, args.bbox);
    }

    world_import::run(&dump_path, &args.world_import_args()?, &args.spell_args()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbc_family_accepts_each_source_and_read_only_census() {
        for source in ["classic-db", "vmangos"] {
            let args = [
                "--source", source, "--dump", "source", "--dbc", "client", "--family", "dbc",
            ];
            assert_eq!(
                parse_args_from(args).unwrap().family.as_deref(),
                Some("dbc")
            );
            if source == "vmangos" {
                assert!(
                    parse_args_from(args.into_iter().chain(["--census", "classic.sql"])).is_ok()
                );
            }
        }
    }

    #[test]
    fn vmangos_snapshot_accepts_its_source_and_refuses_mutating_or_competing_modes() {
        let args = [
            "--source",
            "vmangos",
            "--dump",
            "source.sqlite",
            "--dbc",
            "client",
            "--spell-snapshot",
            "out.json",
        ];
        assert!(parse_args_from(args).is_ok());
        for extra in [
            vec!["--apply"],
            vec!["--go-models", "client"],
            vec!["--family", "spell"],
        ] {
            assert!(parse_args_from(args.into_iter().chain(extra)).is_err());
        }
    }

    #[test]
    fn source_defaults_to_classic_and_vmangos_census_cannot_apply() {
        assert_eq!(
            parse_args_from(["--dump", "source.sql"]).unwrap().source,
            world_source::SourceKind::ClassicDb
        );
        assert!(parse_args_from(["--dump", "source.sql", "--source", "unknown"]).is_err());
        let args = [
            "--dump",
            "source.sqlite",
            "--source",
            "vmangos",
            "--census",
            "classic.sql",
            "--dbc",
            "client/Data",
            "--family",
            "globals",
        ];
        assert_eq!(
            parse_args_from(args).unwrap().source,
            world_source::SourceKind::Vmangos
        );
        assert!(parse_args_from(args.into_iter().chain(["--apply"])).is_err());
    }

    #[test]
    fn the_enabled_packages_root_is_absent_until_the_operator_names_it() {
        let unnamed = parse_args_from(["--dbc", "/path/need-not-exist", "--spells"])
            .expect("the spell importer parses without a packages root");
        assert_eq!(unnamed.packages, None);

        let named = parse_args_from([
            "--dbc",
            "/path/need-not-exist",
            "--spells",
            "--packages",
            "packages",
        ])
        .expect("a named packages root parses");
        assert_eq!(named.packages.as_deref(), Some("packages"));
    }

    #[test]
    fn the_base_snapshot_mode_is_standalone() {
        let alone = parse_args_from([
            "--dbc",
            "/path/need-not-exist",
            "--spell-snapshot",
            "datascripts/generated/base-snapshot.json",
        ])
        .expect("the snapshot mode parses on its own");
        assert_eq!(
            alone.spell_snapshot.as_deref(),
            Some("datascripts/generated/base-snapshot.json")
        );

        for extra in [["--spells"], ["--talents"], ["--apply"]] {
            let mut argv = vec![
                "--dbc",
                "/path/need-not-exist",
                "--spell-snapshot",
                "out.json",
            ];
            argv.extend(extra);
            let error = parse_args_from(argv)
                .err()
                .expect("a snapshot beside an import is refused");
            assert!(
                format!("{error:#}").contains("touches no database"),
                "{error:#}"
            );
        }
    }

    #[test]
    fn a_base_snapshot_without_client_data_is_refused() {
        let error = parse_args_from(["--spell-snapshot", "out.json"])
            .err()
            .expect("a snapshot with no client dir is refused");
        assert!(format!("{error:#}").contains("--dbc"), "{error:#}");
    }

    #[test]
    fn the_client_pack_output_dir_is_an_input_on_its_own() {
        let args = parse_args_from(["--pack-out", "target/client-artifact"])
            .expect("--pack-out parses on its own");
        assert_eq!(args.pack_out.as_deref(), Some("target/client-artifact"));
        assert_eq!(args.pack_client, None);
    }

    #[test]
    fn the_client_pack_output_dir_refuses_apply() {
        let error = parse_args_from(["--pack-out", "target/client-artifact", "--apply"])
            .err()
            .expect("--apply beside --pack-out is refused");
        assert!(
            format!("{error:#}").contains("takes no --apply"),
            "{error:#}"
        );
    }

    #[test]
    fn the_two_client_packers_refuse_to_run_together() {
        let error = parse_args_from([
            "--pack-client",
            "/games/WoW/Data",
            "--pack-out",
            "target/client-artifact",
        ])
        .err()
        .expect("--pack-client beside --pack-out is refused");
        assert!(
            format!("{error:#}").contains("cannot run together"),
            "{error:#}"
        );
    }

    #[test]
    fn an_enabled_packages_root_with_a_package_delta_family_active_parses() {
        let mut argvs: Vec<Vec<&str>> = vec![vec![
            "--dump",
            "/definitely/not/read.sql",
            "--packages",
            "packages",
        ]];
        for family in FAMILIES {
            argvs.push(vec![
                "--dump",
                "/definitely/not/read.sql",
                "--family",
                family,
                "--packages",
                "packages",
            ]);
        }
        for argv in argvs {
            parse_args_from(argv.clone()).unwrap_or_else(|e| panic!("{argv:?} must parse: {e}"));
        }
    }

    #[test]
    fn command_selects_only_the_committed_eventai_source_profile() {
        let default = parse_args_from(["--dump", "/path/need-not-exist.sql"])
            .expect("default Source Profile");
        assert_eq!(default.eventai_profile, eventai::SOURCE_PROFILE_NAME);

        let selected = parse_args_from([
            "--dump",
            "/path/need-not-exist.sql",
            "--eventai-profile",
            eventai::SOURCE_PROFILE_NAME,
        ])
        .expect("named Source Profile");
        assert_eq!(selected.eventai_profile, eventai::SOURCE_PROFILE_NAME);

        let error = parse_args_from([
            "--dump",
            "/definitely/not/read.sql",
            "--eventai-profile",
            "cmangos-classic-z2815-moving-target",
        ])
        .err()
        .expect("unknown EventAI Source Profile");
        assert!(format!("{error:#}").contains("unknown EventAI source profile"));
    }

    #[test]
    fn vmangos_selects_its_profile_and_apply_requires_actual_provenance() {
        let base = [
            "--source",
            "vmangos",
            "--dump",
            "source.sqlite",
            "--family",
            "creature-ai",
        ];
        let args = parse_args_from(base).unwrap();
        assert_eq!(args.eventai_profile, vmangos_profile::NAME);
        assert!(parse_args_from(
            base.into_iter()
                .chain(["--eventai-profile", eventai::SOURCE_PROFILE_NAME])
        )
        .is_err());
        assert!(parse_args_from(base.into_iter().chain(["--apply"])).is_err());
        let actual = "0123456789012345678901234567890123456789";
        let args =
            parse_args_from(base.into_iter().chain(["--apply", "--source-sha", actual])).unwrap();
        assert_eq!(args.source_sha, actual);
    }

    #[test]
    fn command_selects_each_canonical_world_profile() {
        for name in WorldImportProfile::NAMES {
            let args = parse_args_from([
                "--dump",
                "/path/need-not-exist.sql",
                "--world-profile",
                name,
            ])
            .unwrap_or_else(|error| panic!("profile {name} should parse: {error:#}"));
            let scope = args
                .world_import_scope()
                .expect("profile resolves to a scope");
            assert_eq!(scope.name(), *name);
        }
    }

    #[test]
    fn command_refuses_profile_and_legacy_spatial_arguments_before_input_reads() {
        let error = parse_args_from([
            "--dump",
            "/definitely/not/read.sql",
            "--world-profile",
            "alliance-eastern",
            "--map",
            "0",
            "--box",
            "-1,1,-1,1",
        ])
        .err()
        .expect("ambiguous command must fail");
        let message = format!("{error:#}");
        assert!(message.contains("alliance-eastern"), "{message}");
        assert!(message.contains("--map"), "{message}");
        assert!(message.contains("--box"), "{message}");
    }

    #[test]
    fn command_refuses_malformed_or_empty_legacy_scopes() {
        let flat = parse_args_from(["--dump", "/path/need-not-exist.sql", "--box", "1,1,-1,1"])
            .err()
            .expect("flat box must fail");
        assert!(format!("{flat:#}").contains("positive width and height"));

        let removed = parse_args_from([
            "--dump",
            "/path/need-not-exist.sql",
            "--box",
            "-1,1,-1,1",
            "--exclude",
            "-2,2,-2,2",
        ])
        .err()
        .expect("fully excluded box must fail");
        assert!(format!("{removed:#}").contains("scope is empty"));
    }

    #[test]
    fn row_in_slice_keeps_box_geometry_for_the_primary_map_and_whole_map_for_included() {
        let mut args = test_args();
        args.bbox = Some((-9000.0, -8900.0, -200.0, -100.0));
        args.include_maps = vec![36];
        assert!(
            row_in_slice(&args, 0, -8949.95, -132.493, 83.5),
            "in-box map-0 row imports"
        );
        assert!(
            !row_in_slice(&args, 0, -10650.0, 1180.0, 34.0),
            "out-of-box map-0 row does not"
        );
        assert!(
            row_in_slice(&args, 36, -16.4, -383.07, -33.35),
            "map-36 row imports anywhere"
        );
        assert!(!row_in_slice(&args, 1, -16.4, -383.07, -33.35));
        args.exclude = Some((-8960.0, -8940.0, -140.0, -120.0));
        assert!(
            !row_in_slice(&args, 0, -8949.95, -132.493, 83.5),
            "excluded map-0 row drops"
        );
        assert!(
            row_in_slice(&args, 36, -8949.95, -132.493, 83.5),
            "an included map ignores --exclude"
        );
    }

    #[test]
    fn force_include_creatures_keeps_an_out_of_box_row_by_entry() {
        let mut args = test_args();
        args.bbox = Some((-9000.0, -8900.0, -200.0, -100.0));
        assert!(!creature_row_kept(&args, 0, -9219.0, -2222.0, 68.0, 344));
        args.include_creatures = vec![344, 266];
        assert!(
            creature_row_kept(&args, 0, -9219.0, -2222.0, 68.0, 344),
            "force-listed entry bypasses box"
        );
        assert!(
            creature_row_kept(&args, 0, -8950.0, -150.0, 83.5, 999),
            "in-box row still kept"
        );
        assert!(
            !creature_row_kept(&args, 0, -9219.0, -2222.0, 68.0, 999),
            "a non-listed out-of-box entry still drops"
        );
        assert!(!creature_row_kept(&args, 0, -9219.0, -2222.0, 68.0, 0));
    }

    #[test]
    fn the_map_fence_drops_a_force_included_entry_that_spawns_on_a_foreign_map() {
        let mut args = test_args();
        args.map = 1;
        args.bbox = Some((-700.0, -400.0, -4300.0, -4000.0));
        args.include_creatures = vec![344];
        assert!(
            !creature_row_kept(&args, 0, -9219.0, -2222.0, 68.0, 344),
            "a map-0 spawn must NEVER enter a map-1 run, force-listed or not"
        );
        assert!(
            creature_row_kept(&args, 1, -550.0, -4150.0, 39.0, 344),
            "the in-box map-1 row is kept"
        );
        let mut m0 = test_args();
        m0.bbox = Some((-11400.0, -8000.0, -3100.0, 2000.0));
        m0.include_creatures = vec![344];
        assert!(m0.map == 0 && creature_row_kept(&m0, 0, -9219.0, -2222.0, 68.0, 344));
        assert!(!creature_row_kept(&m0, 1, -550.0, -4150.0, 39.0, 344));
        m0.include_maps = vec![36];
        assert!(creature_row_kept(&m0, 36, -16.4, -383.07, -33.35, 644));
        assert!(map_in_run(&m0, 0) && map_in_run(&m0, 36) && !map_in_run(&m0, 1));
    }

    #[test]
    fn taxi_mount_dump_belongs_to_standalone_dbc_mode() {
        let args =
            parse_args_from(["--dbc", "client/Data", "--taxi-mounts", "world.sql.gz"]).unwrap();
        assert_eq!(args.taxi_mounts.as_deref(), Some("world.sql.gz"));
        assert!(args.dump.is_none());
        for mode in ["--spells", "--talents"] {
            assert!(
                parse_args_from(["--dbc", "client/Data", "--taxi-mounts", "world.sql", mode])
                    .is_err()
            );
        }
        assert!(parse_args_from(["--taxi-mounts", "world.sql"]).is_err());
        assert!(parse_args_from([
            "--dbc",
            "client/Data",
            "--taxi-mounts",
            "world.sql",
            "--dump",
            "world.sql"
        ])
        .is_err());
    }

    fn test_args() -> Args {
        Args {
            source: world_source::SourceKind::ClassicDb,
            census: None,
            dbc_diff: false,
            dump: None,
            taxi_mounts: None,
            dbc: None,
            terrain: None,
            packages: None,
            spell_snapshot: None,
            pack_client: None,
            pack_out: None,
            dump_collision: None,
            nav: None,
            vmap: None,
            vmap_status: false,
            vmap_prepare_coverage: None,
            go_models: None,
            print_extents: false,
            spells: false,
            talents: false,
            only: Vec::new(),
            trainers: Vec::new(),
            world_profile: None,
            map: 0,
            include_maps: Vec::new(),
            include_creatures: Vec::new(),
            center: (-8949.95, -132.493, 83.5312),
            radius: 180.0,
            bbox: None,
            exclude: None,
            shard: Shard {
                db: "lyracore".into(),
                server: "local".into(),
            },
            apply: false,
            family: None,
            source_sha: String::new(),
            eventai_profile: eventai::SOURCE_PROFILE_NAME.to_string(),
        }
    }

    fn creature_row_kept(args: &Args, map: i64, x: f64, y: f64, z: f64, raw_entry: u64) -> bool {
        let scope = args
            .world_import_scope()
            .expect("Args contains a scope validated by parse_args");
        world_import::creature_row_kept_in_scope(&scope, map, x, y, z, raw_entry)
    }

    fn map_in_run(args: &Args, map: i64) -> bool {
        args.world_import_scope()
            .expect("Args contains a scope validated by parse_args")
            .contains_map(map)
    }

    fn row_in_slice(args: &Args, map: i64, x: f64, y: f64, z: f64) -> bool {
        args.world_import_scope()
            .expect("Args contains a scope validated by parse_args")
            .contains(map, x, y, z)
    }
}
