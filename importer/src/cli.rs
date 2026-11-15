use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};

use crate::shard::Shard;
use crate::world_import::{WorldImportArgs, FAMILIES};
use crate::world_import_scope::{WorldImportProfile, WorldImportScope};

pub(crate) struct Invocation {
    pub(crate) shard: Shard,
    pub(crate) command: Command,
}

pub(crate) enum Command {
    CompileSpellEdits {
        input: String,
        dbc: String,
    },
    ReadSpellSource {
        id: u32,
        dump: String,
    },
    DbcDiff {
        dump: String,
        dbc: String,
        source_sha: String,
    },
    PackClient {
        data_dir: String,
        apply: bool,
    },
    PackOut {
        out_dir: String,
    },
    Terrain {
        data_dir: String,
        scope: WorldImportScope,
        apply: bool,
    },
    DumpCollision {
        data_dir: String,
        map: i64,
        center: (f64, f64, f64),
    },
    Nav {
        data_dir: String,
        scope: WorldImportScope,
        apply: bool,
    },
    Vmap {
        data_dir: String,
        scope: WorldImportScope,
        apply: bool,
    },
    VmapStatus {
        map: i64,
    },
    VmapCoverage {
        generation_id: u64,
        apply: bool,
    },
    GoModels {
        data_dir: String,
        dump: String,
        apply: bool,
    },
    Talents {
        dbc: String,
        apply: bool,
    },
    DbcCatalogue {
        dbc: String,
        taxi_mounts: String,
        apply: bool,
    },
    SpellSnapshot {
        dump: String,
        dbc: String,
        out: String,
    },
    PrintExtents {
        dump: String,
        map: i64,
        bbox: Option<(f64, f64, f64, f64)>,
    },
    PrintScopeMaps {
        dump: String,
        scope: WorldImportScope,
    },
    PrintPrimaryMap {
        profile: WorldImportProfile,
    },
    PrintLegacyDefaults,
    Spells {
        dump: String,
        dbc: String,
        only: Vec<u32>,
        trainers: Vec<(u32, Vec<u32>)>,
        world: WorldImportArgs,
    },
    WorldImport {
        dump: String,
        world: WorldImportArgs,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum Arity {
    Switch,
    Value,
}

const FLAGS: &[(&str, Arity)] = &[
    ("--compile-spell-edits", Arity::Value),
    ("--read-spell-source", Arity::Value),
    ("--dbc-diff", Arity::Switch),
    ("--taxi-mounts", Arity::Value),
    ("--dump", Arity::Value),
    ("--dbc", Arity::Value),
    ("--terrain", Arity::Value),
    ("--dump-collision", Arity::Value),
    ("--nav", Arity::Value),
    ("--vmap", Arity::Value),
    ("--vmap-status", Arity::Switch),
    ("--vmap-prepare-coverage", Arity::Value),
    ("--go-models", Arity::Value),
    ("--pack-client", Arity::Value),
    ("--pack-out", Arity::Value),
    ("--print-extents", Arity::Switch),
    ("--print-scope-maps", Arity::Switch),
    ("--print-primary-map", Arity::Switch),
    ("--print-legacy-defaults", Arity::Switch),
    ("--spells", Arity::Switch),
    ("--talents", Arity::Switch),
    ("--spell-snapshot", Arity::Value),
    ("--only", Arity::Value),
    ("--trainer", Arity::Value),
    ("--world-profile", Arity::Value),
    ("--map", Arity::Value),
    ("--include-map", Arity::Value),
    ("--radius", Arity::Value),
    ("--box", Arity::Value),
    ("--exclude", Arity::Value),
    ("--center", Arity::Value),
    ("--include-creatures", Arity::Value),
    ("--db", Arity::Value),
    ("--server", Arity::Value),
    ("--apply", Arity::Switch),
    ("--family", Arity::Value),
    ("--source-sha", Arity::Value),
    ("--packages", Arity::Value),
];

// Exactly one mode flag may appear; without one, --dump selects the world import.
const MODES: &[&str] = &[
    "--compile-spell-edits",
    "--read-spell-source",
    "--dbc-diff",
    "--pack-client",
    "--pack-out",
    "--terrain",
    "--dump-collision",
    "--nav",
    "--vmap",
    "--vmap-status",
    "--vmap-prepare-coverage",
    "--go-models",
    "--talents",
    "--taxi-mounts",
    "--spell-snapshot",
    "--print-extents",
    "--print-scope-maps",
    "--print-primary-map",
    "--print-legacy-defaults",
    "--spells",
];

const LEGACY_SPATIAL: &[&str] = &[
    "--map",
    "--include-map",
    "--radius",
    "--box",
    "--exclude",
    "--center",
];

const DEFAULT_CENTER: (f64, f64, f64) = (-8949.95, -132.493, 83.5312);
const DEFAULT_RADIUS: f64 = 180.0;

/// Flags given on the command line; each mode takes what it uses and refuses the rest.
struct Flags {
    given: BTreeMap<&'static str, Vec<String>>,
}

impl Flags {
    fn scan<I, S>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut given: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
        let mut args = args.into_iter().map(Into::into);
        while let Some(arg) = args.next() {
            let Some(&(flag, arity)) = FLAGS.iter().find(|(flag, _)| *flag == arg) else {
                bail!("unknown arg: {arg}");
            };
            let value = match arity {
                Arity::Switch => String::new(),
                Arity::Value => args
                    .next()
                    .with_context(|| format!("{flag} needs a value"))?,
            };
            given.entry(flag).or_default().push(value);
        }
        Ok(Self { given })
    }

    fn has(&self, flag: &str) -> bool {
        self.given.contains_key(flag)
    }

    fn switch(&mut self, flag: &str) -> bool {
        self.given.remove(flag).is_some()
    }

    fn values(&mut self, flag: &str) -> Vec<String> {
        self.given.remove(flag).unwrap_or_default()
    }

    fn value(&mut self, flag: &str) -> Option<String> {
        self.values(flag).pop()
    }

    fn required(&mut self, flag: &str, mode: &str) -> Result<String> {
        self.value(flag)
            .with_context(|| format!("{mode} needs {flag}"))
    }

    fn parsed<T: std::str::FromStr>(&mut self, flag: &str) -> Result<Option<T>>
    where
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        self.value(flag)
            .map(|value| {
                value
                    .trim()
                    .parse()
                    .with_context(|| format!("{flag} {value}: not a valid value"))
            })
            .transpose()
    }

    fn finish(self, mode: &str) -> Result<()> {
        if let Some(flag) = self.given.keys().next() {
            bail!("{flag} is not valid with {mode}");
        }
        Ok(())
    }

    fn scope(&mut self) -> Result<(Option<WorldImportProfile>, WorldImportScope, Vec<u64>)> {
        let include_creatures = match self.value("--include-creatures") {
            Some(list) => number_list::<u64>(&list).with_context(|| {
                format!("--include-creatures {list}: wants comma-separated creature entries")
            })?,
            None => Vec::new(),
        };
        let mut profiles = self.values("--world-profile");
        if profiles.len() > 1 {
            bail!("--world-profile may be given only once");
        }
        let (profile, mut scope) = if let Some(name) = profiles.pop() {
            let profile = WorldImportProfile::parse(&name)?;
            let legacy: Vec<&str> = LEGACY_SPATIAL
                .iter()
                .copied()
                .filter(|flag| self.has(flag))
                .collect();
            if !legacy.is_empty() {
                bail!(
                    "--world-profile {} cannot be mixed with legacy spatial arguments: {}",
                    profile.name(),
                    legacy.join(", ")
                );
            }
            (Some(profile), WorldImportScope::canonical(profile)?)
        } else {
            let map = self.parsed::<i64>("--map")?.unwrap_or(0);
            let mut include_maps = Vec::new();
            for value in self.values("--include-map") {
                let include: i64 = value
                    .trim()
                    .parse()
                    .with_context(|| format!("--include-map {value}: wants a map id"))?;
                if include == map {
                    bail!(
                        "--include-map {map} duplicates --map {map} (the primary map is already in the slice)"
                    );
                }
                include_maps.push(include);
            }
            let radius = self.parsed::<f64>("--radius")?.unwrap_or(DEFAULT_RADIUS);
            let bbox = self.rectangle("--box")?;
            let exclude = self.rectangle("--exclude")?;
            let center = self.center()?;
            (
                None,
                WorldImportScope::legacy(map, bbox, center, radius, exclude, include_maps)?,
            )
        };
        scope.extend_forced_creatures(&include_creatures);
        Ok((profile, scope, include_creatures))
    }

    fn rectangle(&mut self, flag: &str) -> Result<Option<(f64, f64, f64, f64)>> {
        let Some(value) = self.value(flag) else {
            return Ok(None);
        };
        let p = number_list::<f64>(&value)
            .with_context(|| format!("{flag} wants four numbers: X0,X1,Y0,Y1"))?;
        if p.len() != 4 {
            bail!(
                "{flag} wants exactly 4 comma-separated numbers, got {}",
                p.len()
            );
        }
        Ok(Some((
            p[0].min(p[1]),
            p[0].max(p[1]),
            p[2].min(p[3]),
            p[2].max(p[3]),
        )))
    }

    fn center(&mut self) -> Result<(f64, f64, f64)> {
        let Some(value) = self.value("--center") else {
            return Ok(DEFAULT_CENTER);
        };
        let p = number_list::<f64>(&value).context("--center wants three numbers: X,Y,Z")?;
        if p.len() != 3 {
            bail!(
                "--center wants exactly 3 comma-separated numbers, got {}",
                p.len()
            );
        }
        Ok((p[0], p[1], p[2]))
    }

    fn source_sha(&mut self, apply: bool) -> Result<String> {
        let sha = self.value("--source-sha").unwrap_or_default();
        if apply && (sha.len() != 40 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit())) {
            bail!("apply requires --source-sha with the actual snapshot commit");
        }
        Ok(sha)
    }

    fn world_import_args(
        &mut self,
        shard: &Shard,
        family: Option<String>,
    ) -> Result<WorldImportArgs> {
        let (world_profile, scope, include_creatures) = self.scope()?;
        let dbc = self.value("--dbc");
        let packages = self.value("--packages");
        let apply = self.switch("--apply");
        let source_sha = self.source_sha(apply)?;
        Ok(WorldImportArgs {
            dbc,
            family,
            world_profile,
            scope,
            include_creatures,
            packages,
            source_sha,
            apply,
            shard: shard.clone(),
        })
    }
}

fn number_list<T: std::str::FromStr>(list: &str) -> Result<Vec<T>>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    Ok(list
        .split(',')
        .map(|item| item.trim().parse::<T>())
        .collect::<std::result::Result<_, _>>()?)
}

pub(crate) fn parse<I, S>(args: I) -> Result<Invocation>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut flags = Flags::scan(args)?;
    let shard = Shard {
        db: flags.value("--db").unwrap_or_else(|| "lyracore".into()),
        server: flags.value("--server").unwrap_or_else(|| "local".into()),
    };
    let modes: Vec<&str> = MODES
        .iter()
        .copied()
        .filter(|mode| flags.has(mode))
        .collect();
    let mode = match modes.as_slice() {
        [] if flags.has("--dump") => "--dump",
        [] if flags.has("--dbc") => {
            bail!("standalone --dbc needs --taxi-mounts <sqlite> or --talents")
        }
        [] => bail!(
            "need an input: --dump <vmangos.sqlite>, --dbc <client Data/ dir>, --terrain <client Data/ dir>, --pack-client <client Data/ dir>, or --pack-out <dir>"
        ),
        [mode] => mode,
        [first, second, ..] => {
            bail!("{first} and {second} are separate modes; run one at a time")
        }
    };
    let command = command(mode, &mut flags, &shard)?;
    flags.finish(mode)?;
    Ok(Invocation { shard, command })
}

fn command(mode: &'static str, flags: &mut Flags, shard: &Shard) -> Result<Command> {
    Ok(match mode {
        "--compile-spell-edits" => Command::CompileSpellEdits {
            input: flags.required(mode, mode)?,
            dbc: flags.required("--dbc", mode)?,
        },
        "--read-spell-source" => Command::ReadSpellSource {
            id: flags
                .parsed(mode)?
                .context("--read-spell-source needs an id")?,
            dump: flags.required("--dump", mode)?,
        },
        "--dbc-diff" => {
            flags.switch(mode);
            Command::DbcDiff {
                dump: flags.required("--dump", mode)?,
                dbc: flags.required("--dbc", mode)?,
                source_sha: flags.source_sha(false)?,
            }
        }
        "--pack-client" => Command::PackClient {
            data_dir: flags.required(mode, mode)?,
            apply: flags.switch("--apply"),
        },
        "--pack-out" => Command::PackOut {
            out_dir: flags.required(mode, mode)?,
        },
        "--terrain" => Command::Terrain {
            data_dir: flags.required(mode, mode)?,
            scope: flags.scope()?.1,
            apply: flags.switch("--apply"),
        },
        "--dump-collision" => Command::DumpCollision {
            data_dir: flags.required(mode, mode)?,
            map: flags.parsed("--map")?.unwrap_or(0),
            center: flags.center()?,
        },
        "--nav" => Command::Nav {
            data_dir: flags.required(mode, mode)?,
            scope: flags.scope()?.1,
            apply: flags.switch("--apply"),
        },
        "--vmap" => Command::Vmap {
            data_dir: flags.required(mode, mode)?,
            scope: flags.scope()?.1,
            apply: flags.switch("--apply"),
        },
        "--vmap-status" => {
            flags.switch(mode);
            Command::VmapStatus {
                map: flags.parsed("--map")?.unwrap_or(0),
            }
        }
        "--vmap-prepare-coverage" => Command::VmapCoverage {
            generation_id: flags
                .parsed(mode)?
                .context("--vmap-prepare-coverage needs a generation id")?,
            apply: flags.switch("--apply"),
        },
        "--go-models" => Command::GoModels {
            data_dir: flags.required(mode, mode)?,
            dump: flags.required("--dump", mode)?,
            apply: flags.switch("--apply"),
        },
        "--talents" => {
            flags.switch(mode);
            Command::Talents {
                dbc: flags.required("--dbc", mode)?,
                apply: flags.switch("--apply"),
            }
        }
        "--taxi-mounts" => {
            let taxi_mounts = flags.required(mode, mode)?;
            let dbc = flags.required("--dbc", mode)?;
            let apply = flags.switch("--apply");
            flags.source_sha(apply)?;
            Command::DbcCatalogue {
                dbc,
                taxi_mounts,
                apply,
            }
        }
        "--spell-snapshot" => Command::SpellSnapshot {
            out: flags.required(mode, mode)?,
            dump: flags.required("--dump", mode)?,
            dbc: flags.required("--dbc", mode)?,
        },
        "--print-extents" => {
            flags.switch(mode);
            Command::PrintExtents {
                dump: flags.required("--dump", mode)?,
                map: flags.parsed("--map")?.unwrap_or(0),
                bbox: flags.rectangle("--box")?,
            }
        }
        "--print-scope-maps" => {
            flags.switch(mode);
            Command::PrintScopeMaps {
                dump: flags.required("--dump", mode)?,
                scope: flags.scope()?.1,
            }
        }
        "--print-primary-map" => {
            flags.switch(mode);
            Command::PrintPrimaryMap {
                profile: WorldImportProfile::parse(&flags.required("--world-profile", mode)?)?,
            }
        }
        "--print-legacy-defaults" => {
            flags.switch(mode);
            Command::PrintLegacyDefaults
        }
        "--spells" => {
            flags.switch(mode);
            let dump = flags.required("--dump", mode)?;
            let mut only = match flags.value("--only") {
                Some(list) => {
                    number_list::<u32>(&list).context("--only wants comma-separated spell ids")?
                }
                None => Vec::new(),
            };
            let mut trainers = Vec::new();
            for value in flags.values("--trainer") {
                let (entry, ids) = value
                    .split_once('=')
                    .context("--trainer wants <entry>=<comma-ids>, e.g. 913=78,6674")?;
                let entry: u32 = entry
                    .trim()
                    .parse()
                    .context("--trainer <entry> must be a creature-template entry (u32)")?;
                let ids = number_list::<u32>(ids)
                    .context("--trainer wants comma-separated spell ids after '='")?;
                for &id in &ids {
                    if !only.contains(&id) {
                        only.push(id);
                    }
                }
                trainers.push((entry, ids));
            }
            let world = flags.world_import_args(shard, Some("spell".into()))?;
            let dbc = world.dbc.clone().context("--spells needs --dbc")?;
            Command::Spells {
                dump,
                dbc,
                only,
                trainers,
                world,
            }
        }
        "--dump" => {
            let dump = flags.required("--dump", "world import")?;
            let family = flags.value("--family");
            if let Some(family) = &family {
                if !FAMILIES.contains(&family.as_str())
                    && !matches!(family.as_str(), "spell" | "dbc")
                {
                    bail!(
                        "--family {family}: unknown family (valid: {}, spell, dbc)",
                        FAMILIES.join(", ")
                    );
                }
            }
            Command::WorldImport {
                dump,
                world: flags.world_import_args(shard, family)?,
            }
        }
        _ => unreachable!("MODES lists every mode flag"),
    })
}

#[cfg(test)]
pub(crate) fn world_import_args<I, S>(args: I) -> WorldImportArgs
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    match parse(args).expect("world import arguments parse").command {
        Command::WorldImport { world, .. } => world,
        _ => panic!("arguments select another mode"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(args: &[&str]) -> String {
        format!(
            "{:#}",
            parse(args.iter().copied())
                .err()
                .expect("arguments must be refused")
        )
    }

    fn scope(extra: &[&str]) -> WorldImportScope {
        world_import_args(["--dump", "source.sqlite"].iter().chain(extra).copied()).scope
    }

    #[test]
    fn a_base_snapshot_stands_alone_and_needs_its_source_and_client() {
        let args = [
            "--dump",
            "source.sqlite",
            "--dbc",
            "client",
            "--spell-snapshot",
            "out.json",
        ];
        assert!(matches!(
            parse(args).unwrap().command,
            Command::SpellSnapshot { .. }
        ));
        for extra in [
            vec!["--apply"],
            vec!["--family", "spell"],
            vec!["--packages", "packages"],
        ] {
            let mut argv = args.to_vec();
            argv.extend(extra.iter().copied());
            assert!(refusal(&argv).contains("is not valid with --spell-snapshot"));
        }
        for extra in [
            vec!["--go-models", "client"],
            vec!["--spells"],
            vec!["--talents"],
        ] {
            let mut argv = args.to_vec();
            argv.extend(extra.iter().copied());
            assert!(refusal(&argv).contains("are separate modes"));
        }
        assert!(refusal(&["--spell-snapshot", "out.json"]).contains("--dump"));
        assert!(refusal(&["--dump", "s", "--spell-snapshot", "out.json"]).contains("--dbc"));
    }

    #[test]
    fn the_enabled_packages_root_is_absent_until_the_operator_names_it() {
        let packages = |extra: &[&str]| {
            let mut argv = vec!["--dump", "source.sqlite", "--dbc", "client", "--spells"];
            argv.extend(extra);
            match parse(argv).unwrap().command {
                Command::Spells { world, .. } => world.packages,
                _ => panic!("--spells selects the spell import"),
            }
        };
        assert_eq!(packages(&[]), None);
        assert_eq!(
            packages(&["--packages", "packages"]).as_deref(),
            Some("packages")
        );
        for family in FAMILIES {
            let argv = [
                "--dump",
                "source.sqlite",
                "--family",
                family,
                "--packages",
                "packages",
            ];
            assert_eq!(
                world_import_args(argv).packages.as_deref(),
                Some("packages")
            );
        }
    }

    #[test]
    fn the_client_packers_take_only_their_own_output() {
        assert!(matches!(
            parse(["--pack-out", "target/client-artifact"]).unwrap().command,
            Command::PackOut { out_dir } if out_dir == "target/client-artifact"
        ));
        assert!(
            refusal(&["--pack-out", "target/client-artifact", "--apply"])
                .contains("--apply is not valid with --pack-out")
        );
        assert!(refusal(&[
            "--pack-client",
            "/games/WoW/Data",
            "--pack-out",
            "target/client-artifact",
        ])
        .contains("are separate modes"));
    }

    #[test]
    fn spell_editor_modes_refuse_apply_and_import_modes() {
        for argv in [
            vec!["--compile-spell-edits", "-", "--dbc", "dbc", "--apply"],
            vec![
                "--read-spell-source",
                "133",
                "--dump",
                "world.sqlite",
                "--apply",
            ],
            vec![
                "--compile-spell-edits",
                "-",
                "--dbc",
                "dbc",
                "--dump",
                "world.sqlite",
            ],
        ] {
            assert!(refusal(&argv).contains("is not valid with"), "{argv:?}");
        }
        assert!(
            refusal(&["--compile-spell-edits", "-", "--dbc", "dbc", "--spells"])
                .contains("are separate modes")
        );
    }

    #[test]
    fn eventai_source_profile_is_no_longer_a_flag() {
        assert_eq!(
            refusal(&[
                "--dump",
                "source.sqlite",
                "--eventai-profile",
                "vmangos-4641790"
            ]),
            "unknown arg: --eventai-profile"
        );
    }

    #[test]
    fn apply_requires_the_actual_source_commit() {
        let base = ["--dump", "source.sqlite", "--family", "creature-ai"];
        assert_eq!(
            refusal(&[&base[..], &["--apply"]].concat()),
            "apply requires --source-sha with the actual snapshot commit"
        );
        let actual = "0123456789012345678901234567890123456789";
        let args = world_import_args(base.into_iter().chain(["--apply", "--source-sha", actual]));
        assert_eq!(args.source_sha, actual);
        assert!(args.apply);
    }

    #[test]
    fn each_canonical_world_profile_selects_its_scope() {
        for name in WorldImportProfile::names() {
            assert_eq!(scope(&["--world-profile", name]).name(), name);
        }
    }

    #[test]
    fn a_profile_refuses_legacy_spatial_arguments_before_input_reads() {
        let message = refusal(&[
            "--dump",
            "/definitely/not/read.sql",
            "--world-profile",
            "alliance-eastern",
            "--map",
            "0",
            "--box",
            "-1,1,-1,1",
        ]);
        assert!(message.contains("alliance-eastern"), "{message}");
        assert!(message.contains("--map"), "{message}");
        assert!(message.contains("--box"), "{message}");
    }

    #[test]
    fn malformed_or_empty_legacy_scopes_are_refused() {
        assert!(
            refusal(&["--dump", "s", "--box", "1,1,-1,1"]).contains("positive width and height")
        );
        assert!(refusal(&[
            "--dump",
            "s",
            "--box",
            "-1,1,-1,1",
            "--exclude",
            "-2,2,-2,2",
        ])
        .contains("scope is empty"));
    }

    #[test]
    fn a_legacy_box_keeps_its_geometry_on_the_primary_map_and_whole_included_maps() {
        let slice = scope(&["--box", "-9000,-8900,-200,-100", "--include-map", "36"]);
        assert!(slice.contains(0, -8949.95, -132.493, 83.5));
        assert!(!slice.contains(0, -10650.0, 1180.0, 34.0));
        assert!(slice.contains(36, -16.4, -383.07, -33.35));
        assert!(!slice.contains(1, -16.4, -383.07, -33.35));
        let excluded = scope(&[
            "--box",
            "-9000,-8900,-200,-100",
            "--include-map",
            "36",
            "--exclude",
            "-8960,-8940,-140,-120",
        ]);
        assert!(!excluded.contains(0, -8949.95, -132.493, 83.5));
        assert!(excluded.contains(36, -8949.95, -132.493, 83.5));
    }

    #[test]
    fn forced_creatures_bypass_the_box_but_never_the_map_fence() {
        let kept = |extra: &[&str], map, x, y, z, entry| {
            crate::world_import::creature_row_kept_in_scope(&scope(extra), map, x, y, z, entry)
        };
        let human = ["--box", "-9000,-8900,-200,-100"];
        assert!(!kept(&human, 0, -9219.0, -2222.0, 68.0, 344));
        let forced = [&human[..], &["--include-creatures", "344,266"]].concat();
        assert!(kept(&forced, 0, -9219.0, -2222.0, 68.0, 344));
        assert!(kept(&forced, 0, -8950.0, -150.0, 83.5, 999));
        assert!(!kept(&forced, 0, -9219.0, -2222.0, 68.0, 999));
        assert!(!kept(&forced, 0, -9219.0, -2222.0, 68.0, 0));

        let kalimdor = [
            "--map",
            "1",
            "--box",
            "-700,-400,-4300,-4000",
            "--include-creatures",
            "344",
        ];
        assert!(!kept(&kalimdor, 0, -9219.0, -2222.0, 68.0, 344));
        assert!(kept(&kalimdor, 1, -550.0, -4150.0, 39.0, 344));
        let eastern = [
            "--box",
            "-11400,-8000,-3100,2000",
            "--include-creatures",
            "344",
            "--include-map",
            "36",
        ];
        assert!(kept(&eastern, 0, -9219.0, -2222.0, 68.0, 344));
        assert!(!kept(&eastern, 1, -550.0, -4150.0, 39.0, 344));
        assert!(kept(&eastern, 36, -16.4, -383.07, -33.35, 644));
        let eastern = scope(&eastern);
        assert!(eastern.contains_map(0) && eastern.contains_map(36) && !eastern.contains_map(1));
    }

    #[test]
    fn taxi_mounts_belong_to_the_standalone_dbc_catalogue() {
        assert!(matches!(
            parse(["--dbc", "client/Data", "--taxi-mounts", "world.sqlite"]).unwrap().command,
            Command::DbcCatalogue { taxi_mounts, .. } if taxi_mounts == "world.sqlite"
        ));
        for mode in ["--spells", "--talents"] {
            assert!(refusal(&[
                "--dbc",
                "client/Data",
                "--taxi-mounts",
                "world.sqlite",
                mode
            ])
            .contains("are separate modes"));
        }
        assert!(refusal(&["--taxi-mounts", "world.sqlite"]).contains("--dbc"));
        assert_eq!(
            refusal(&[
                "--dbc",
                "client/Data",
                "--taxi-mounts",
                "world.sqlite",
                "--dump",
                "world.sqlite"
            ]),
            "--dump is not valid with --taxi-mounts"
        );
    }
}
