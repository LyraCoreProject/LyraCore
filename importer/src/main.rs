mod cli;
mod collision;
mod creature_population;
mod dbc;
mod dbc_diff;
mod dump;
mod eventai;
mod extents;
mod go_model;
mod gossip_conditions;
mod instance_vmap;
mod item_property;
mod m2_collision;
mod map_catalogue;
mod nav;
mod pack_client;
mod package_delta;
mod shard;
mod source_selection;
mod spell;
mod spell_dbc;
mod spell_editor;
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

use anyhow::Result;
use cli::Command;

fn main() -> Result<()> {
    let cli::Invocation { shard, command } = cli::parse(std::env::args().skip(1))?;
    match command {
        Command::CompileSpellEdits { input, dbc } => spell_editor::run(&input, &dbc),
        Command::ReadSpellSource { id, dump } => spell_editor::read_source(&dump, id),
        Command::DbcDiff {
            dump,
            dbc,
            source_sha,
        } => {
            print!(
                "{}",
                dbc_diff::run(
                    std::path::Path::new(&dump),
                    std::path::Path::new(&dbc),
                    &source_sha
                )?
            );
            Ok(())
        }
        Command::PackClient { data_dir, apply } => pack_client::run(&data_dir, apply),
        Command::PackOut { out_dir } => pack_client::run_pack_out(&out_dir),
        Command::Terrain {
            data_dir,
            scope,
            apply,
        } => terrain::run(&data_dir, &scope, apply, &shard),
        Command::DumpCollision {
            data_dir,
            map,
            center,
        } => collision::run(&data_dir, map, center),
        Command::Nav {
            data_dir,
            scope,
            apply,
        } => nav::run(&data_dir, &scope, apply, &shard),
        Command::Vmap {
            data_dir,
            scope,
            apply,
        } => vmap::run(&data_dir, &scope, apply, &shard),
        Command::VmapStatus { map } => vmap::active_generation_status(&shard, map as u32),
        Command::VmapCoverage {
            generation_id,
            apply,
        } => vmap::run_coverage(generation_id, apply, &shard),
        Command::GoModels {
            data_dir,
            dump,
            apply,
        } => go_model::run(&data_dir, &dump, apply, &shard),
        Command::Talents { dbc, apply } => talent::run_talents(&dbc, apply, &shard),
        Command::DbcCatalogue {
            dbc,
            taxi_mounts,
            apply,
        } => {
            let source = world_source::WorldSource::open(std::path::Path::new(&taxi_mounts))?;
            dbc::run_from_dump(&dbc, &source, apply, &shard)
        }
        Command::SpellSnapshot { dump, dbc, out } => spell::run_vmangos_snapshot(&dump, &dbc, &out),
        Command::PrintExtents { dump, map, bbox } => extents::print_extents(&dump, map, bbox),
        Command::PrintScopeMaps { dump, scope } => {
            let source = world_source::WorldSource::open(std::path::Path::new(&dump))?;
            for map in scope.source_maps(&source)? {
                println!("{map}");
            }
            Ok(())
        }
        Command::PrintPrimaryMap { profile } => {
            println!("{}", profile.primary_map());
            Ok(())
        }
        Command::PrintLegacyDefaults => {
            print!("{}", world_import_scope::legacy_defaults_for_shell());
            Ok(())
        }
        Command::Spells {
            dump,
            dbc,
            only,
            trainers,
            world,
        } => {
            let source = world_source::WorldSource::open(std::path::Path::new(&dump))?;
            let plan = spell::selected_world_plan(&source, &dbc, &only, &trainers)?;
            world_import::emit_plan(&source, &world, &plan)
        }
        Command::WorldImport { dump, world } => world_import::run(&dump, &world),
    }
}
