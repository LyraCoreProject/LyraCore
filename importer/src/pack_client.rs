//! `--pack-client` — the dev-time CLIENT PATCH packager. Builds a `patch-3.MPQ` (DBC overlays + our
//! own raw assets) into the operator's OWN client `Data/`, installs our addons into `Interface/AddOns/`,
//! and clears the `WDB/` cache — so a lyracore developer can push client-side changes to their
//! local client in ONE command. (Player distribution is a separate, out-of-scope problem.)
//!
//! SOURCES: the repo-global `client-patch/{mpq,addons}` tree PLUS every package's optional
//! `packages/<name>/client/{mpq,addons}` — a feature ships its client UI next to its server code
//! (the packages doctrine), and a client-only package (a `client/` with no `src/`) is legal. Both
//! channels are first-class: addons are additive and coexist; an `mpq/` file at
//! `Interface\FrameXML\...` is a whole-file OVERRIDE of the stock UI (reach for it only when
//! addon hooks can't — packages/ is private-only tooling, not part of the public
//! mirror). Cross-source collisions (same archive path or
//! same addon name from two sources) fail the pack loudly naming both sources — never
//! last-write-wins.
//!
//! FIREWALL (see `dbc.rs`): the repo-committed inputs under `client-patch/` are OURS only (our addons,
//! our raw assets like BLP icons, and overlay code) — NEVER Blizzard files (a `client-patch/.gitignore`
//! blocks `*.dbc`/`*.MPQ`). DBCs are read from the operator's base archives IN MEMORY, our rows applied,
//! and re-serialized into the patch MPQ; the emitted `patch-3.MPQ` + installed addons are local build
//! artifacts in the operator's own client, never committed and never shipped by us.
//!
//! Load order on this client: base archives < `patch.MPQ` < `patch-2.MPQ` < `patch-3.MPQ` (higher wins),
//! so our overlay shadows the base. Patch files must be UNENCRYPTED. After a DBC change the `WDB/` cache
//! must be cleared or the client serves stale name/tooltip/icon data.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::{bail, Context, Result};
use wow_dbc::vanilla_tables::area_table::AreaTable;
use wow_dbc::DbcTable;
use wow_mpq::{compression::flags, ArchiveBuilder, FormatVersion, PatchChain};

use crate::Args;

/// First free numeric patch slot on this client (base < patch < patch-2 < patch-3); higher wins.
const PATCH_MPQ: &str = "patch-3.MPQ";
/// Source-controlled inputs (ours only) — see the firewall note above.
const SRC: &str = "client-patch";

/// One file destined for the patch MPQ: the internal archive path (backslash-separated) + its
/// bytes + which source contributed it (for dry-run provenance and collision messages).
struct PackFile {
    archive_path: String,
    data: Vec<u8>,
    source: String,
}

/// One addon directory to install: its name (the `Interface/AddOns/<name>` target) + where it
/// lives + which source contributed it.
struct AddonDir {
    name: String,
    path: std::path::PathBuf,
    source: String,
}

/// The client-content contribution roots, in deterministic order: `client-patch/` first, then
/// every `packages/<name>/client/` alphabetically. Each yields raw MPQ files and/or addon dirs.
fn client_sources(
    repo_src: &Path,
    packages_root: &Path,
) -> Result<Vec<(String, std::path::PathBuf)>> {
    let mut sources = vec![("client-patch".to_string(), repo_src.to_path_buf())];
    if packages_root.is_dir() {
        let mut names: Vec<String> = fs::read_dir(packages_root)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("client").is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for n in &names {
            sources.push((format!("package {n}"), packages_root.join(n).join("client")));
        }
    }
    Ok(sources)
}

/// Collect every source's `mpq/` files and `addons/` dirs, failing LOUDLY on cross-source
/// collisions (MPQ paths compared case-insensitively — the archive is; addon names likewise —
/// the target filesystem may be) and on firewall violations (a committed `.dbc`/`.mpq` is never
/// ours to ship raw; DBC overlays go through the in-memory stage).
fn collect_client_content(
    sources: &[(String, std::path::PathBuf)],
) -> Result<(Vec<PackFile>, Vec<AddonDir>)> {
    use std::collections::HashMap;
    let mut files: Vec<PackFile> = Vec::new();
    let mut addons: Vec<AddonDir> = Vec::new();
    let mut seen_paths: HashMap<String, String> = HashMap::new(); // lower(archive path) → source
    let mut seen_addons: HashMap<String, String> = HashMap::new(); // lower(addon name) → source

    for (label, root) in sources {
        let mpq_root = root.join("mpq");
        if mpq_root.is_dir() {
            let mut raw = Vec::new();
            collect_raw(&mpq_root, &mpq_root, &mut raw)?;
            // fs::read_dir order is filesystem-dependent — sort so the dry-run listing and the
            // packed MPQ layout are stable across machines/runs (diff-able).
            raw.sort_by(|a, b| a.archive_path.cmp(&b.archive_path));
            for mut f in raw {
                let lower = f.archive_path.to_ascii_lowercase();
                if lower.ends_with(".dbc") || lower.ends_with(".mpq") {
                    bail!(
                        "{label}: {} is a raw {} file — the licensing firewall forbids shipping Blizzard-format binaries from the repo (DBC overlays are applied in memory; see dbc.rs)",
                        f.archive_path,
                        &lower[lower.len() - 4..],
                    );
                }
                if let Some(prev) = seen_paths.insert(lower, label.clone()) {
                    bail!(
                        "archive path collision: {} is shipped by BOTH {prev} and {label} — an MPQ holds one copy and the client loads one file, so this can never merge; move one copy or make one package own the file",
                        f.archive_path,
                    );
                }
                f.source = label.clone();
                files.push(f);
            }
        }
        let addons_root = root.join("addons");
        if addons_root.is_dir() {
            let mut names: Vec<String> = fs::read_dir(&addons_root)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            for name in names {
                if let Some(prev) = seen_addons.insert(name.to_ascii_lowercase(), label.clone()) {
                    bail!(
                        "addon name collision: Interface/AddOns/{name} is shipped by BOTH {prev} and {label} — rename one (addon dirs install to one target)",
                    );
                }
                addons.push(AddonDir {
                    name: name.clone(),
                    path: addons_root.join(&name),
                    source: label.clone(),
                });
            }
        }
    }
    Ok((files, addons))
}

/// `--pack-client <client Data/ dir>` mode. Without `--apply` it's a dry run (prints what it would do).
pub fn run(data_dir: &str, args: &Args) -> Result<()> {
    let data = Path::new(data_dir);
    if !data.join("dbc.MPQ").exists() && !data.join("patch.MPQ").exists() {
        bail!(
            "{} doesn't look like a client Data/ dir (no dbc.MPQ/patch.MPQ)",
            data.display()
        );
    }
    let client_root = data.parent().context("client Data/ has no parent dir")?;
    let src = Path::new(SRC);

    // 1) Raw assets (ours), from EVERY source: client-patch/mpq/ plus each packages/<name>/client/mpq/
    //    → the MPQ at its relative path. This is the generic Tier-1 path — BLP icons, sounds, fonts,
    //    FrameXML overrides, loading screens, etc. Collisions and firewall violations bail here.
    let sources = client_sources(src, Path::new("packages"))?;
    let (mut files, addon_dirs) = collect_client_content(&sources)?;

    // 2) DBC overlays: read the operator's base DBC IN MEMORY, apply our additions, re-serialize into
    //    the MPQ (never a committed .dbc). [SPIKE] MVP payload: AreaTable round-trip UNCHANGED — a
    //    do-nothing identity stage TODAY. It proves wow_dbc::write + the MPQ packing produce a
    //    client-loadable DBC, and is the exact hook where custom rows go (e.g. a quest-log zone/sort
    //    header: `area.rows_mut().push(..)`). TODO: when a real DBC overlay lands, apply rows here; if
    //    none is planned, delete this stage — a patch MPQ with zero DBC overlays is valid and simpler.
    let mut chain = crate::dbc::open_chain(data)?;
    let area: AreaTable = crate::dbc::read_table(&mut chain)?;
    // OVERLAY HOOK: apply our row additions/edits here before re-serializing, e.g.
    //   area.rows_mut().push(custom_areatable_header_row);
    let mut buf = Vec::new();
    area.write(&mut buf).context("re-serialize AreaTable.dbc")?;
    files.push(PackFile {
        archive_path: "DBFilesClient\\AreaTable.dbc".into(),
        data: buf,
        source: "dbc-overlay".into(),
    });

    // 3) Addons (ours), already collected per source: → <client>/Interface/AddOns/<Name>/.
    let addons_dst = client_root.join("Interface").join("AddOns");

    if !args.apply {
        println!("-- DRY RUN (--pack-client). Re-run with --apply to write to your client.");
        println!(
            "would build {} with {} file(s):",
            data.join(PATCH_MPQ).display(),
            files.len()
        );
        for f in &files {
            println!(
                "    {}  ({} bytes, from {})",
                f.archive_path,
                f.data.len(),
                f.source
            );
        }
        println!(
            "would install {} addon(s) → {}:",
            addon_dirs.len(),
            addons_dst.display()
        );
        for a in &addon_dirs {
            println!("    {}  (from {})", a.name, a.source);
        }
        println!(
            "would clear the WDB cache at {}",
            client_root.join("WDB").display()
        );
        return Ok(());
    }

    // Build the patch MPQ (vanilla V1, zlib, UNENCRYPTED — the client rejects encrypted patch files).
    let out = data.join(PATCH_MPQ);
    let mut b = ArchiveBuilder::new()
        .version(FormatVersion::V1)
        .default_compression(flags::ZLIB);
    for f in &files {
        b = b.add_file_data_with_options(f.data.clone(), &f.archive_path, flags::ZLIB, false, 0);
    }
    b.build(&out)
        .with_context(|| format!("build {}", out.display()))?;
    eprintln!(
        "pack-client: built {} ({} file(s))",
        out.display(),
        files.len()
    );

    // Install addons (every source's).
    for a in &addon_dirs {
        let to = addons_dst.join(&a.name);
        copy_dir(&a.path, &to)
            .with_context(|| format!("install addon {} (from {})", a.name, a.source))?;
        eprintln!(
            "pack-client: installed addon {} → {} (from {})",
            a.name,
            to.display(),
            a.source
        );
    }

    clear_wdb(client_root)?;
    verify(&out)?;
    eprintln!("pack-client: done + verified. Restart the client (DBC/MPQ changes) or /reload (addons) to apply.");
    Ok(())
}

/// Recursively collect files under `dir` (rooted at `root`) as `PackFile`s with backslash archive paths.
fn collect_raw(root: &Path, dir: &Path, out: &mut Vec<PackFile>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        // Skip hidden files (.gitkeep, .gitignore, editor dotfiles) — never real assets.
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if path.is_dir() {
            collect_raw(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .expect("walked under root")
                .to_string_lossy()
                .replace('/', "\\");
            out.push(PackFile {
                archive_path: rel,
                data: fs::read(&path)?,
                source: String::new(),
            });
        }
    }
    Ok(())
}

/// Recursively copy `from` into `to` (used to install an addon folder).
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let path = entry?.path();
        let dst = to.join(path.file_name().expect("dir entry has a name"));
        if path.is_dir() {
            copy_dir(&path, &dst)?;
        } else {
            fs::copy(&path, &dst)?;
        }
    }
    Ok(())
}

/// Delete the `*.wdb` cache files (stale name/tooltip/icon caches mask DBC changes). Idempotent.
fn clear_wdb(client_root: &Path) -> Result<()> {
    let wdb = client_root.join("WDB");
    if !wdb.is_dir() {
        return Ok(());
    }
    let mut n = 0u32;
    for entry in fs::read_dir(&wdb)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("wdb"))
        {
            fs::remove_file(&path)?;
            n += 1;
        }
    }
    eprintln!("pack-client: cleared {n} WDB cache file(s)");
    Ok(())
}

/// Self-verify: re-open the built patch MPQ and confirm a packed DBC reads back + re-parses (proves the
/// wow-mpq write + wow_dbc round-trip without needing the client). The live-client load is the operator's
/// final check.
fn verify(mpq: &Path) -> Result<()> {
    let mut chain = PatchChain::new();
    chain
        .add_archive(mpq, 0)
        .context("reopen built patch MPQ")?;
    let bytes = chain
        .read_file("DBFilesClient\\AreaTable.dbc")
        .context("read AreaTable back from the built MPQ")?;
    AreaTable::read(&mut Cursor::new(bytes))
        .context("re-parse packed AreaTable (round-trip failed)")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A unique scratch tree per test (std-only; no tempfile dep). Cleaned on drop.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "pack-client-test-{}-{}",
                std::process::id(),
                tag
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Scratch(p)
        }
        fn write(&self, rel: &str, bytes: &[u8]) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, bytes).unwrap();
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn sources_are_client_patch_first_then_packages_alphabetically() {
        let t = Scratch::new("sources");
        t.write("client-patch/mpq/.gitkeep", b"");
        t.write("packages/zeta/client/mpq/z.blp", b"z");
        t.write("packages/alpha/client/addons/A/A.toc", b"a");
        t.write("packages/no-client/src/mod.rs", b""); // no client/ → not a source
        let sources = client_sources(&t.0.join("client-patch"), &t.0.join("packages")).unwrap();
        let labels: Vec<&str> = sources.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["client-patch", "package alpha", "package zeta"]);
    }

    #[test]
    fn packages_contribute_mpq_files_and_addons_with_provenance() {
        let t = Scratch::new("contrib");
        t.write("client-patch/mpq/Interface/FrameXML/Stock.lua", b"cp");
        t.write("client-patch/addons/WireQA/WireQA.toc", b"");
        t.write("packages/autoloot/client/addons/AutoLoot/AutoLoot.toc", b"");
        t.write(
            "packages/autoloot/client/mpq/Interface/FrameXML/LootFrame.lua",
            b"al",
        );
        let sources = client_sources(&t.0.join("client-patch"), &t.0.join("packages")).unwrap();
        let (files, addons) = collect_client_content(&sources).unwrap();
        let by_path: Vec<(&str, &str)> = files
            .iter()
            .map(|f| (f.archive_path.as_str(), f.source.as_str()))
            .collect();
        assert!(by_path.contains(&("Interface\\FrameXML\\Stock.lua", "client-patch")));
        assert!(by_path.contains(&("Interface\\FrameXML\\LootFrame.lua", "package autoloot")));
        let by_addon: Vec<(&str, &str)> = addons
            .iter()
            .map(|a| (a.name.as_str(), a.source.as_str()))
            .collect();
        assert!(by_addon.contains(&("WireQA", "client-patch")));
        assert!(by_addon.contains(&("AutoLoot", "package autoloot")));
    }

    #[test]
    fn archive_path_collision_names_both_sources_case_insensitively() {
        let t = Scratch::new("path-collide");
        t.write("client-patch/mpq/Interface/FrameXML/LootFrame.lua", b"a");
        t.write(
            "packages/autoloot/client/mpq/Interface/FrameXML/LOOTFRAME.LUA",
            b"b",
        );
        let sources = client_sources(&t.0.join("client-patch"), &t.0.join("packages")).unwrap();
        let Err(e) = collect_client_content(&sources) else {
            panic!("expected collision error")
        };
        let err = e.to_string();
        assert!(err.contains("collision"), "{err}");
        assert!(
            err.contains("client-patch") && err.contains("package autoloot"),
            "{err}"
        );
    }

    #[test]
    fn addon_name_collision_names_both_sources() {
        let t = Scratch::new("addon-collide");
        t.write("client-patch/addons/AutoLoot/AutoLoot.toc", b"");
        t.write("packages/autoloot/client/addons/autoloot/autoloot.toc", b"");
        let sources = client_sources(&t.0.join("client-patch"), &t.0.join("packages")).unwrap();
        let Err(e) = collect_client_content(&sources) else {
            panic!("expected collision error")
        };
        let err = e.to_string();
        assert!(err.contains("addon name collision"), "{err}");
        assert!(
            err.contains("client-patch") && err.contains("package autoloot"),
            "{err}"
        );
    }

    #[test]
    fn firewall_rejects_raw_dbc_and_mpq_files() {
        for (tag, rel) in [
            ("firewall-dbc", "client-patch/mpq/DBFilesClient/Spell.dbc"),
            ("firewall-mpq", "packages/foo/client/mpq/Data/evil.MPQ"),
        ] {
            let t = Scratch::new(tag);
            t.write(rel, b"nope");
            let sources = client_sources(&t.0.join("client-patch"), &t.0.join("packages")).unwrap();
            let Err(e) = collect_client_content(&sources) else {
                panic!("expected firewall error")
            };
            let err = e.to_string();
            assert!(err.contains("firewall"), "{err}");
        }
    }
}
