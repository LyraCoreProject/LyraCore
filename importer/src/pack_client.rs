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
//!
//! UI TRANSFORMS: a source may also ship a `ui-transforms.json` beside its `mpq/` and `addons/`
//! trees. Each entry anchors an insertion into one stock FrameXML or GlueXML file. The packer reads
//! that file's baseline out of the operator's own UI archives, composes every Package's edits for
//! it, and packs the result — so two Packages can extend one stock file without either owning it.
//! A composed file is [`Origin::BaselineDerived`]: it is the client's own bytes with our edits in
//! it, and it never leaves this machine.
//!
//! `--pack-out <dir>` is the second output: the same collected sources written into a plain
//! directory, for distribution. It never opens a client, so it refuses every baseline-derived file
//! by its [`Origin`] and names where those bytes came from.

use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use wow_dbc::vanilla_tables::area_table::AreaTable;
use wow_dbc::DbcTable;
use wow_mpq::{compression::flags, ArchiveBuilder, FormatVersion, PatchChain};

use crate::ui_transform::{self, Edit};
use crate::Args;

/// First free numeric patch slot on this client (base < patch < patch-2 < patch-3); higher wins.
const PATCH_MPQ: &str = "patch-3.MPQ";
/// Source-controlled inputs (ours only) — see the firewall note above.
const SRC: &str = "client-patch";

/// Dropped inside an installed addon's own folder, naming the source that put it there
/// (`client-patch` or `package <name>`). This is the ONLY provenance this packer keeps — one file
/// per addon, gone if the addon folder goes — never a central inventory of every file it has ever
/// written. It exists solely to power [`stale_addons`]'s warning; nothing here reads it to decide
/// what to overwrite, and nothing here deletes on its account.
const SOURCE_MARKER: &str = ".lyracore-source";

/// A source's UI Transform declaration, read from the contribution root itself (beside `mpq/` and
/// `addons/`) rather than from inside either, so it is never mistaken for content to pack.
const UI_TRANSFORMS: &str = "ui-transforms.json";

/// Where a packed file's bytes came from. `source` is the human label; this is the provenance the
/// licensing firewall reads. Only package-authored bytes may leave this machine.
#[derive(Debug, PartialEq, Eq)]
enum Origin {
    /// Bytes an author committed under `client-patch/` or `packages/<name>/client/`.
    PackageAuthored,
    /// Bytes computed from the operator's own client: a DBC overlay or a UI Transform output.
    /// `from` names the baseline input, for the refusal message.
    BaselineDerived { from: String },
}

/// One file destined for the patch MPQ: the internal archive path (backslash-separated) + its
/// bytes + which source contributed it (for dry-run provenance and collision messages) + the
/// provenance that decides whether it may be distributed.
struct PackFile {
    archive_path: String,
    data: Vec<u8>,
    source: String,
    origin: Origin,
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
///
/// Every file this yields is [`Origin::PackageAuthored`]; the roots hold committed bytes only.
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

/// Parse every source's [`UI_TRANSFORMS`] declaration. Most sources have none; that is normal, and
/// a source with one still contributes its `mpq/` and `addons/` trees as usual. The source label
/// (`client-patch` or `package <name>`) names the owner in every refusal the engine raises.
fn collect_ui_transforms(sources: &[(String, PathBuf)]) -> Result<Vec<Edit>> {
    let mut edits = Vec::new();
    for (label, root) in sources {
        let declaration = root.join(UI_TRANSFORMS);
        if !declaration.is_file() {
            continue;
        }
        let json = fs::read_to_string(&declaration)
            .with_context(|| format!("read {}", declaration.display()))?;
        edits.extend(ui_transform::parse(label, &json)?);
    }
    Ok(edits)
}

/// Everything the repo contributes, collected once for either output. The roots are the same; only
/// what happens afterwards differs.
fn collect(
    repo_src: &Path,
    packages_root: &Path,
) -> Result<(Vec<PackFile>, Vec<AddonDir>, Vec<Edit>)> {
    let sources = client_sources(repo_src, packages_root)?;
    let (files, addons) = collect_client_content(&sources)?;
    let transforms = collect_ui_transforms(&sources)?;
    Ok((files, addons, transforms))
}

/// Group edits by the file they change, matching paths case-insensitively because the archive
/// does. The key orders the groups, so the composed output does not depend on which Package the
/// walk reached first; the displayed path keeps its first author's casing.
fn group_by_path(edits: &[Edit]) -> Vec<(&str, Vec<&Edit>)> {
    let mut grouped: BTreeMap<String, (&str, Vec<&Edit>)> = BTreeMap::new();
    for edit in edits {
        grouped
            .entry(edit.path.to_ascii_lowercase())
            .or_insert_with(|| (edit.path.as_str(), Vec::new()))
            .1
            .push(edit);
    }
    grouped.into_values().collect()
}

/// The Packages behind one group, in source order and without repeats, for a refusal message.
fn transforming_packages(edits: &[&Edit]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for edit in edits {
        if !names.contains(&edit.package) {
            names.push(edit.package.clone());
        }
    }
    names
}

/// Put the record header in front of the composed file, or, when an XML document opens with an XML
/// declaration, straight after it. A declaration has to be the first thing in the document, so a
/// comment above it would make the client reject the file.
fn with_record_header(header: &str, composed: &str) -> String {
    if composed.starts_with("<?xml") {
        if let Some(end) = composed.find("?>") {
            let (declaration, rest) = composed.split_at(end + 2);
            let rest = rest
                .strip_prefix("\r\n")
                .or_else(|| rest.strip_prefix('\n'))
                .unwrap_or(rest);
            return format!("{declaration}\n{header}{rest}");
        }
    }
    format!("{header}{composed}")
}

/// An `mpq/` file replaces a stock file whole; a UI Transform edits the stock file in place. Both
/// at one path is a contradiction with no winner worth guessing at, so it fails the pack the same
/// way a cross-source collision does.
fn refuse_override_of_a_transformed_path(
    files: &[PackFile],
    groups: &[(&str, Vec<&Edit>)],
) -> Result<()> {
    for (path, edits) in groups {
        let lower = path.to_ascii_lowercase();
        let Some(override_file) = files
            .iter()
            .find(|f| f.archive_path.to_ascii_lowercase() == lower)
        else {
            continue;
        };
        bail!(
            "{} is shipped as a whole-file override by {} AND edited by a UI Transform from {} — a file can be replaced or patched, not both; drop the override or the transform",
            override_file.archive_path,
            override_file.source,
            transforming_packages(edits).join(", "),
        );
    }
    Ok(())
}

/// Compose every transformed path against the operator's own client and return the results as
/// packed files, ready to join the collision checks like any other path.
///
/// The bytes that come back are the client's own baseline with the Packages' edits applied, so
/// each one is [`Origin::BaselineDerived`] and names the archive it was read out of. A
/// [`ui_transform::record_header`] line goes in front, carrying the baseline hash and the
/// transforms hash: the same client and the same declarations rebuild byte-identical output.
fn compose_ui_transforms(data: &Path, groups: &[(&str, Vec<&Edit>)]) -> Result<Vec<PackFile>> {
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let mut chain = crate::dbc::open_ui_baseline_chain(data)?;
    let searched: Vec<String> = crate::dbc::ui_baseline_archives(data)
        .iter()
        .map(|p| p.display().to_string())
        .collect();

    let mut packed = Vec::with_capacity(groups.len());
    for (path, edits) in groups {
        let archive = chain
            .find_file_archive(path)
            .map(|a| a.display().to_string())
            .unwrap_or_else(|| searched.join(", "));
        let bytes = chain.read_file(path).with_context(|| {
            format!(
                "{path} is not in this client's UI archives ({}) — check the path against the stock UI",
                searched.join(", ")
            )
        })?;
        let baseline_hash = blake3::hash(&bytes).to_hex().to_string();
        let baseline = String::from_utf8(bytes)
            .with_context(|| format!("{path} in {archive} is not UTF-8 text"))?;

        let composed = ui_transform::compose(path, &baseline, edits)?;
        let header = ui_transform::record_header(
            path,
            &baseline_hash,
            &ui_transform::transforms_hash(edits),
        );
        packed.push(PackFile {
            archive_path: (*path).to_string(),
            data: with_record_header(&header, &composed).into_bytes(),
            source: format!("ui-transform ({})", transforming_packages(edits).join(", ")),
            origin: Origin::BaselineDerived {
                from: format!("{archive}:{path}"),
            },
        });
    }
    Ok(packed)
}

/// `--pack-client <client Data/ dir>` mode. Without `--apply` it's a dry run (prints what it would do).
pub fn run(data_dir: &str, args: &Args) -> Result<()> {
    pack_client(
        Path::new(data_dir),
        args.apply,
        Path::new(SRC),
        Path::new("packages"),
    )
}

fn pack_client(data: &Path, apply: bool, repo_src: &Path, packages_root: &Path) -> Result<()> {
    if !data.join("dbc.MPQ").exists() && !data.join("patch.MPQ").exists() {
        bail!(
            "{} doesn't look like a client Data/ dir (no dbc.MPQ/patch.MPQ)",
            data.display()
        );
    }
    let client_root = data.parent().context("client Data/ has no parent dir")?;

    // 1) Raw assets (ours), from EVERY source: client-patch/mpq/ plus each packages/<name>/client/mpq/
    //    → the MPQ at its relative path. This is the generic Tier-1 path — BLP icons, sounds, fonts,
    //    FrameXML overrides, loading screens, etc. Collisions and firewall violations bail here.
    let (mut files, addon_dirs, transforms) = collect(repo_src, packages_root)?;

    // 1b) UI Transforms, declared by the same sources. The repo-only refusals (a bad declaration,
    //     an override contradicting a transform) land here, before the client is opened at all.
    let groups = group_by_path(&transforms);
    refuse_override_of_a_transformed_path(&files, &groups)?;

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
        origin: Origin::BaselineDerived {
            from: format!("DBFilesClient\\AreaTable.dbc in {}", data.display()),
        },
    });

    // 2b) UI Transforms: compose each edited stock file against this client's UI archives. The
    //     AreaTable read above already refused a wrong client version, so no second check is needed.
    files.extend(compose_ui_transforms(data, &groups)?);

    // 3) Addons (ours), already collected per source: → <client>/Interface/AddOns/<Name>/.
    let addons_dst = client_root.join("Interface").join("AddOns");

    // Best-effort: an addon this or an earlier `--apply` installed, whose source (a Package, most
    // often) is disabled or gone this run. Shown on a dry run too — it costs only a directory
    // listing and an operator deciding whether to `rm -rf` it should not need `--apply` first.
    for warning in stale_addons(&addons_dst, &addon_dirs)? {
        eprintln!("pack-client: WARNING — {warning}");
    }

    if !apply {
        println!("-- DRY RUN (--pack-client). Re-run with --apply to write to your client.");
        println!(
            "would build {} with {} file(s):",
            data.join(PATCH_MPQ).display(),
            files.len()
        );
        for f in &files {
            // The origin is on the listing because it decides where the file may go: a
            // baseline-derived one reaches this client and no artifact.
            let derived = match &f.origin {
                Origin::PackageAuthored => String::new(),
                Origin::BaselineDerived { from } => format!(", derived from {from}"),
            };
            println!(
                "    {}  ({} bytes, from {}{})",
                f.archive_path,
                f.data.len(),
                f.source,
                derived
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

    let out = data.join(PATCH_MPQ);
    build_patch_mpq(&files, &out)?;
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
        write_source_marker(&to, &a.source)
            .with_context(|| format!("stamp addon {} with its source", a.name))?;
        eprintln!(
            "pack-client: installed addon {} → {} (from {})",
            a.name,
            to.display(),
            a.source
        );
    }

    clear_wdb(client_root)?;
    verify_area_table(&out)?;
    eprintln!("pack-client: done + verified. Restart the client (DBC/MPQ changes) or /reload (addons) to apply.");
    Ok(())
}

/// `--pack-out <dir>` mode: the client half of the Client Artifact, for distribution. It collects
/// the same sources as `--pack-client` and opens no client, so a baseline-derived file has no way
/// in and is refused by name. Writes `<dir>/Data/patch-3.MPQ` and `<dir>/Interface/AddOns/<Name>/`,
/// and nothing else. Ownership of `<dir>` (replacement, the manifest, the zip) is the CLI's.
pub fn run_pack_out(out_dir: &str) -> Result<()> {
    pack_out(Path::new(out_dir), Path::new(SRC), Path::new("packages"))
}

fn pack_out(out_dir: &Path, repo_src: &Path, packages_root: &Path) -> Result<()> {
    let (files, addon_dirs, transforms) = collect(repo_src, packages_root)?;
    write_pack_out(out_dir, &files, &addon_dirs, &transforms)
}

fn write_pack_out(
    out_dir: &Path,
    files: &[PackFile],
    addon_dirs: &[AddonDir],
    transforms: &[Edit],
) -> Result<()> {
    refuse_baseline_derived(files, transforms)?;

    // Every refusal is behind us, so nothing below can leave a half-written artifact.
    fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;

    if files.is_empty() {
        eprintln!("pack-out: no mpq/ content in any source, so no {PATCH_MPQ} is written");
    } else {
        let data_dir = out_dir.join("Data");
        fs::create_dir_all(&data_dir).with_context(|| format!("create {}", data_dir.display()))?;
        let out = data_dir.join(PATCH_MPQ);
        build_patch_mpq(files, &out)?;
        verify_readback(&out, &files[0])?;
        eprintln!(
            "pack-out: built {} ({} file(s))",
            out.display(),
            files.len()
        );
    }

    let addons_dst = out_dir.join("Interface").join("AddOns");
    for a in addon_dirs {
        let to = addons_dst.join(&a.name);
        copy_dir(&a.path, &to)
            .with_context(|| format!("pack addon {} (from {})", a.name, a.source))?;
        eprintln!(
            "pack-out: packed addon {} → {} (from {})",
            a.name,
            to.display(),
            a.source
        );
    }
    eprintln!(
        "pack-out: done. {} holds package-authored content only.",
        out_dir.display()
    );
    Ok(())
}

/// The licensing firewall for a distributable artifact: baseline-derived bytes stay on the
/// operator's machine, where `--pack-client` puts them. Refuses before the first write and names
/// the file and the baseline input it came from.
///
/// A UI Transform is refused from its declaration rather than from its output, because `--pack-out`
/// opens no client and so never composes one. Declaring an edit is enough: whatever it produces is
/// the client's own baseline with that edit in it.
fn refuse_baseline_derived(files: &[PackFile], transforms: &[Edit]) -> Result<()> {
    for f in files {
        if let Origin::BaselineDerived { from } = &f.origin {
            bail!(
                "{}: {} is derived from the operator's own client ({from}). A distributable artifact carries package-authored content only, so this file reaches a client through `client sync` alone",
                f.source,
                f.archive_path,
            );
        }
    }

    if !transforms.is_empty() {
        let mut declared: Vec<String> = transforms
            .iter()
            .map(|e| format!("{} edits {}", e.package, e.path))
            .collect();
        declared.sort();
        declared.dedup();
        bail!(
            "UI Transform declared: {}. A composed UI file is the operator's own client baseline with those edits applied, so a distributable artifact cannot carry it; it reaches a client through `client sync` alone",
            declared.join("; "),
        );
    }
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
                origin: Origin::PackageAuthored,
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

/// Stamp an installed addon directory with the source that put it there — see [`SOURCE_MARKER`].
fn write_source_marker(addon_dir: &Path, source: &str) -> Result<()> {
    fs::write(addon_dir.join(SOURCE_MARKER), source).context("write addon source marker")
}

/// Best-effort, no-ledger staleness check: which currently-installed addon directories carry a
/// [`SOURCE_MARKER`] this run's `current` sources no longer produce.
///
/// The only file read is the marker inside each candidate addon's own folder, so this can never
/// flag an addon nobody's `--apply` ever wrote to — an addon the operator installed by hand, or
/// one from before this marker existed, is silently left alone rather than guessed at. Matches
/// addon names case-insensitively, the same as the collision checks above (the destination
/// filesystem may not distinguish case even where the marker's own text does).
fn stale_addons(addons_dst: &Path, current: &[AddonDir]) -> Result<Vec<String>> {
    if !addons_dst.is_dir() {
        return Ok(Vec::new());
    }
    let current_names: std::collections::HashSet<String> = current
        .iter()
        .map(|a| a.name.to_ascii_lowercase())
        .collect();
    let mut names: Vec<String> = fs::read_dir(addons_dst)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();

    let mut stale = Vec::new();
    for name in names {
        if current_names.contains(&name.to_ascii_lowercase()) {
            continue;
        }
        let marker = addons_dst.join(&name).join(SOURCE_MARKER);
        if let Ok(source) = fs::read_to_string(&marker) {
            stale.push(format!(
                "Interface/AddOns/{name} was installed from {} by an earlier sync, and no current \
                 source provides it (a disabled or removed Package?) — left in place; remove it \
                 by hand if you no longer want it",
                source.trim(),
            ));
        }
    }
    Ok(stale)
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

/// Build the patch MPQ (vanilla V1, zlib, UNENCRYPTED — the client rejects encrypted patch files).
fn build_patch_mpq(files: &[PackFile], out: &Path) -> Result<()> {
    let mut b = ArchiveBuilder::new()
        .version(FormatVersion::V1)
        .default_compression(flags::ZLIB);
    for f in files {
        b = b.add_file_data_with_options(f.data.clone(), &f.archive_path, flags::ZLIB, false, 0);
    }
    b.build(out)
        .with_context(|| format!("build {}", out.display()))
}

/// Self-verify for `--pack-out`: re-open the built MPQ and confirm one packed file comes back
/// byte for byte. There is no DBC in a package-authored archive, so this is the round-trip proof.
fn verify_readback(mpq: &Path, file: &PackFile) -> Result<()> {
    let mut chain = PatchChain::new();
    chain.add_archive(mpq, 0).context("reopen built MPQ")?;
    let bytes = chain
        .read_file(&file.archive_path)
        .with_context(|| format!("read {} back from the built MPQ", file.archive_path))?;
    if bytes != file.data {
        bail!(
            "{} read back from {} with different bytes (MPQ round-trip failed)",
            file.archive_path,
            mpq.display()
        );
    }
    Ok(())
}

/// Self-verify: re-open the built patch MPQ and confirm a packed DBC reads back + re-parses (proves the
/// wow-mpq write + wow_dbc round-trip without needing the client). The live-client load is the operator's
/// final check.
fn verify_area_table(mpq: &Path) -> Result<()> {
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

    /// The sorted directory entry names under `dir`, for "and nothing else" assertions.
    fn entry_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
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

    #[test]
    fn write_source_marker_round_trips_into_the_addon_directory() {
        let t = Scratch::new("marker-write");
        let addon = t.0.join("Interface/AddOns/AutoLoot");
        fs::create_dir_all(&addon).unwrap();
        write_source_marker(&addon, "package autoloot").unwrap();
        let recorded = fs::read_to_string(addon.join(SOURCE_MARKER)).unwrap();
        assert_eq!(recorded, "package autoloot");
    }

    #[test]
    fn stale_addons_flags_only_marked_addons_the_current_sources_no_longer_provide() {
        let t = Scratch::new("stale-warn");
        let dst = t.0.join("Interface/AddOns");
        // Still produced by this run: marked, and named by `current` — never flagged.
        let current_addon = dst.join("AutoLoot");
        fs::create_dir_all(&current_addon).unwrap();
        write_source_marker(&current_addon, "package autoloot").unwrap();
        // The Package behind it was disabled or removed: marked, but NOT named by `current`.
        let retired_addon = dst.join("Retired");
        fs::create_dir_all(&retired_addon).unwrap();
        write_source_marker(&retired_addon, "package retired").unwrap();
        // The operator's own addon: never marked by any sync, so it must never be flagged.
        t.write("Interface/AddOns/BigWigs/BigWigs.toc", b"");

        let current = vec![AddonDir {
            name: "AutoLoot".to_string(),
            path: t.0.join("client-patch/addons/AutoLoot"),
            source: "package autoloot".to_string(),
        }];
        let warnings = stale_addons(&dst, &current).unwrap();

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("Retired"), "{warnings:?}");
        assert!(warnings[0].contains("package retired"), "{warnings:?}");
        assert!(
            !warnings.iter().any(|w| w.contains("BigWigs")),
            "{warnings:?}"
        );
        assert!(
            !warnings.iter().any(|w| w.contains("AutoLoot")),
            "{warnings:?}"
        );
    }

    #[test]
    fn stale_addons_matches_names_case_insensitively_like_the_collision_checks() {
        let t = Scratch::new("stale-case");
        let dst = t.0.join("Interface/AddOns");
        let addon = dst.join("AutoLoot");
        fs::create_dir_all(&addon).unwrap();
        write_source_marker(&addon, "package autoloot").unwrap();

        let current = vec![AddonDir {
            name: "AUTOLOOT".to_string(),
            path: t.0.join("client-patch/addons/AutoLoot"),
            source: "package autoloot".to_string(),
        }];
        assert!(stale_addons(&dst, &current).unwrap().is_empty());
    }

    #[test]
    fn stale_addons_is_empty_on_a_fresh_client_with_no_addons_dir_yet() {
        let t = Scratch::new("stale-fresh");
        let dst = t.0.join("Interface/AddOns");
        assert!(stale_addons(&dst, &[]).unwrap().is_empty());
    }

    /// The Client Artifact's client half: every `mpq/` file in one archive, every addon directory
    /// beside it, and nothing the operator's own client would need (no marker, no WDB).
    #[test]
    fn pack_out_writes_every_collected_file_and_addon_and_nothing_else() {
        let t = Scratch::new("pack-out-full");
        t.write(
            "packages/alpha/client/mpq/Interface/Icons/Alpha.blp",
            b"blp",
        );
        t.write("packages/alpha/client/addons/Alpha/Alpha.toc", b"toc");
        t.write(
            "packages/zeta/client/mpq/Interface/FrameXML/Zeta.lua",
            b"lua",
        );
        t.write("packages/zeta/client/addons/Zeta/Zeta.lua", b"z");
        let out = t.0.join("artifact");

        pack_out(&out, &t.0.join("client-patch"), &t.0.join("packages")).unwrap();

        let mut chain = PatchChain::new();
        chain
            .add_archive(out.join("Data").join(PATCH_MPQ), 0)
            .unwrap();
        assert_eq!(
            chain.read_file("Interface\\Icons\\Alpha.blp").unwrap(),
            b"blp"
        );
        assert_eq!(
            chain.read_file("Interface\\FrameXML\\Zeta.lua").unwrap(),
            b"lua"
        );

        let addons = out.join("Interface").join("AddOns");
        assert_eq!(
            fs::read_to_string(addons.join("Alpha/Alpha.toc")).unwrap(),
            "toc"
        );
        assert_eq!(
            fs::read_to_string(addons.join("Zeta/Zeta.lua")).unwrap(),
            "z"
        );
        assert!(
            !addons.join("Alpha").join(SOURCE_MARKER).exists(),
            "the source marker belongs to the operator's own client, not a distributable artifact"
        );
        assert_eq!(entry_names(&out), ["Data", "Interface"]);
        assert_eq!(entry_names(&out.join("Data")), [PATCH_MPQ]);
    }

    /// A repo whose packages ship addons only has nothing to put in an archive. wow-mpq will build
    /// an empty one, but an artifact holding a patch MPQ with no content of ours would be a lie.
    #[test]
    fn pack_out_without_mpq_content_writes_the_addons_and_no_archive() {
        let t = Scratch::new("pack-out-addons-only");
        t.write("packages/alpha/client/addons/Alpha/Alpha.toc", b"toc");
        let out = t.0.join("artifact");

        pack_out(&out, &t.0.join("client-patch"), &t.0.join("packages")).unwrap();

        assert_eq!(entry_names(&out), ["Interface"]);
        assert!(out.join("Interface/AddOns/Alpha/Alpha.toc").exists());
    }

    /// The licensing firewall: a file computed out of the operator's client never reaches a
    /// distributable artifact, and the refusal names the file and the baseline it came from.
    #[test]
    fn pack_out_refuses_a_baseline_derived_file_before_writing_anything() {
        let t = Scratch::new("pack-out-provenance");
        let out = t.0.join("artifact");
        let files = vec![
            PackFile {
                archive_path: "Interface\\Icons\\Alpha.blp".into(),
                data: b"blp".to_vec(),
                source: "package alpha".into(),
                origin: Origin::PackageAuthored,
            },
            PackFile {
                archive_path: "DBFilesClient\\AreaTable.dbc".into(),
                data: b"dbc".to_vec(),
                source: "dbc-overlay".into(),
                origin: Origin::BaselineDerived {
                    from: "DBFilesClient\\AreaTable.dbc in /wowclient/Data".into(),
                },
            },
        ];

        let Err(e) = write_pack_out(&out, &files, &[], &[]) else {
            panic!("expected a provenance refusal")
        };
        let err = e.to_string();
        assert!(err.contains("DBFilesClient\\AreaTable.dbc"), "{err}");
        assert!(err.contains("/wowclient/Data"), "{err}");
        assert!(
            !out.exists(),
            "a refusal must leave the output dir untouched"
        );
    }

    /// `--pack-out` reads the repo, never a client. It runs where no client `Data/` dir exists.
    #[test]
    fn pack_out_needs_no_client_data_directory() {
        let t = Scratch::new("pack-out-no-client");
        t.write(
            "packages/alpha/client/mpq/Interface/Icons/Alpha.blp",
            b"blp",
        );
        let out = t.0.join("artifact");

        pack_out(&out, &t.0.join("client-patch"), &t.0.join("packages")).unwrap();

        assert!(out.join("Data").join(PATCH_MPQ).exists());
        assert!(!t.0.join("Data").exists());
    }

    /// Both outputs collect the same sources, so a collision fails the artifact the same way it
    /// fails a client sync, and before anything is written.
    #[test]
    fn pack_out_refuses_a_cross_source_collision() {
        let t = Scratch::new("pack-out-collide");
        t.write(
            "packages/alpha/client/mpq/Interface/FrameXML/LootFrame.lua",
            b"a",
        );
        t.write(
            "packages/zeta/client/mpq/Interface/FrameXML/lootframe.lua",
            b"b",
        );
        let out = t.0.join("artifact");

        let Err(e) = pack_out(&out, &t.0.join("client-patch"), &t.0.join("packages")) else {
            panic!("expected collision error")
        };
        let err = e.to_string();
        assert!(err.contains("collision"), "{err}");
        assert!(
            !out.exists(),
            "a refusal must leave the output dir untouched"
        );
    }

    // ---------------------------------------------------------------------------------------
    //  UI Transforms
    // ---------------------------------------------------------------------------------------

    const LOOT_FRAME_LUA: &str =
        "function LootFrame_OnLoad()\n\tthis:RegisterEvent(\"LOOT_OPENED\");\nend\n";
    const FRAMEXML_TOC: &str = "## Interface: 11200\nLootFrame.xml\nMainMenuBar.xml\n";

    fn build_scratch_mpq(out: &Path, files: &[(&str, &[u8])]) {
        fs::create_dir_all(out.parent().unwrap()).unwrap();
        let mut b = ArchiveBuilder::new()
            .version(FormatVersion::V1)
            .default_compression(flags::ZLIB);
        for (path, data) in files {
            b = b.add_file_data_with_options(data.to_vec(), path, flags::ZLIB, false, 0);
        }
        b.build(out).unwrap();
    }

    /// A scratch client `Data/` dir: an empty `AreaTable.dbc` in `dbc.MPQ` (the version check reads
    /// it) and the given stock UI files in `interface.MPQ`. Never a real client.
    fn scratch_client(t: &Scratch, ui: &[(&str, &[u8])]) -> PathBuf {
        let data = t.0.join("client/Data");
        let mut dbc = Vec::new();
        AreaTable { rows: Vec::new() }.write(&mut dbc).unwrap();
        build_scratch_mpq(
            &data.join("dbc.MPQ"),
            &[("DBFilesClient\\AreaTable.dbc", &dbc)],
        );
        build_scratch_mpq(&data.join("interface.MPQ"), ui);
        data
    }

    /// One file's text out of a built patch MPQ.
    fn packed_text(mpq: &Path, archive_path: &str) -> String {
        let mut chain = PatchChain::new();
        chain.add_archive(mpq, 0).unwrap();
        String::from_utf8(chain.read_file(archive_path).unwrap()).unwrap()
    }

    /// The whole point: a Package extends a stock FrameXML file it does not own, and the composed
    /// result carries the header, the insertion and the baseline it was composed from.
    #[test]
    fn a_transform_composes_the_stock_file_into_the_patch_mpq() {
        let t = Scratch::new("ui-compose");
        let data = scratch_client(
            &t,
            &[(
                "Interface\\FrameXML\\LootFrame.lua",
                LOOT_FRAME_LUA.as_bytes(),
            )],
        );
        t.write(
            "packages/loot/client/ui-transforms.json",
            br#"[{ "path": "Interface/FrameXML/LootFrame.lua",
                  "after": "function LootFrame_OnLoad()",
                  "insert": "\n\tPkgLoot_OnLoad();" }]"#,
        );

        pack_client(
            &data,
            true,
            &t.0.join("client-patch"),
            &t.0.join("packages"),
        )
        .unwrap();

        let composed = packed_text(&data.join(PATCH_MPQ), "Interface\\FrameXML\\LootFrame.lua");
        assert!(
            composed.starts_with("-- Generated by LyraCore from the operator's client. baseline="),
            "{composed}"
        );
        assert!(composed.contains("\tPkgLoot_OnLoad();"), "{composed}");
        assert!(
            composed.contains("this:RegisterEvent(\"LOOT_OPENED\");"),
            "the baseline must survive the edit: {composed}"
        );
        assert_eq!(
            entry_names(&t.0.join("packages/loot/client")),
            ["ui-transforms.json"],
            "the baseline is read from the client and never written back into the repo"
        );
    }

    /// The namespaced-file pattern: the Package ships `PkgLoot.lua` under its own `mpq/` and adds
    /// the TOC line that loads it with a transform. Nothing else is needed to extend the stock UI.
    #[test]
    fn a_package_owned_file_and_the_toc_line_that_loads_it_both_land() {
        let t = Scratch::new("ui-namespaced");
        let data = scratch_client(
            &t,
            &[("Interface\\FrameXML\\FrameXML.toc", FRAMEXML_TOC.as_bytes())],
        );
        t.write(
            "packages/loot/client/mpq/Interface/FrameXML/PkgLoot.lua",
            b"function PkgLoot_OnLoad() end\n",
        );
        t.write(
            "packages/loot/client/ui-transforms.json",
            br#"[{ "path": "Interface/FrameXML/FrameXML.toc",
                  "before": "LootFrame.xml",
                  "insert": "PkgLoot.lua\n" }]"#,
        );

        pack_client(
            &data,
            true,
            &t.0.join("client-patch"),
            &t.0.join("packages"),
        )
        .unwrap();

        let mpq = data.join(PATCH_MPQ);
        assert_eq!(
            packed_text(&mpq, "Interface\\FrameXML\\PkgLoot.lua"),
            "function PkgLoot_OnLoad() end\n"
        );
        let toc = packed_text(&mpq, "Interface\\FrameXML\\FrameXML.toc");
        assert!(toc.starts_with("# Generated by LyraCore"), "{toc}");
        assert!(toc.contains("PkgLoot.lua\nLootFrame.xml\n"), "{toc}");
    }

    /// Two Packages extend one stock file at different anchors. Neither owns the file, and the
    /// composed output holds both edits in the baseline's own order.
    #[test]
    fn two_packages_edit_one_file_at_disjoint_anchors() {
        let t = Scratch::new("ui-two-packages");
        let data = scratch_client(
            &t,
            &[("Interface\\FrameXML\\FrameXML.toc", FRAMEXML_TOC.as_bytes())],
        );
        t.write(
            "packages/alpha/client/ui-transforms.json",
            br#"[{ "path": "Interface/FrameXML/FrameXML.toc",
                  "before": "LootFrame.xml",
                  "insert": "Alpha.lua\n" }]"#,
        );
        t.write(
            "packages/zeta/client/ui-transforms.json",
            br#"[{ "path": "Interface/FrameXML/FrameXML.toc",
                  "after": "MainMenuBar.xml",
                  "insert": "\nZeta.lua" }]"#,
        );

        pack_client(
            &data,
            true,
            &t.0.join("client-patch"),
            &t.0.join("packages"),
        )
        .unwrap();

        let toc = packed_text(&data.join(PATCH_MPQ), "Interface\\FrameXML\\FrameXML.toc");
        let body = toc.split_once('\n').expect("a record header line").1;
        assert_eq!(
            body,
            "## Interface: 11200\nAlpha.lua\nLootFrame.xml\nMainMenuBar.xml\nZeta.lua\n"
        );
    }

    /// An XML declaration has to stay the first thing in the document, so the header goes under it
    /// rather than above it. Every other file takes the header first.
    #[test]
    fn the_record_header_never_displaces_an_xml_declaration() {
        assert_eq!(
            with_record_header("<!-- h -->\n", "<?xml version=\"1.0\"?>\n<Ui/>\n"),
            "<?xml version=\"1.0\"?>\n<!-- h -->\n<Ui/>\n"
        );
        assert_eq!(
            with_record_header("<!-- h -->\n", "<Ui/>\n"),
            "<!-- h -->\n<Ui/>\n"
        );
    }

    /// Two Packages claiming byte ranges that intersect have no correct merge, so the pack fails
    /// naming both of them, and the client keeps the patch it already had.
    #[test]
    fn overlapping_transforms_refuse_and_write_nothing() {
        let t = Scratch::new("ui-overlap");
        let data = scratch_client(
            &t,
            &[(
                "Interface\\FrameXML\\LootFrame.lua",
                LOOT_FRAME_LUA.as_bytes(),
            )],
        );
        t.write(
            "packages/alpha/client/ui-transforms.json",
            br#"[{ "path": "Interface/FrameXML/LootFrame.lua",
                  "after": "function LootFrame", "insert": "a" }]"#,
        );
        t.write(
            "packages/zeta/client/ui-transforms.json",
            br#"[{ "path": "Interface/FrameXML/LootFrame.lua",
                  "before": "LootFrame_OnLoad()", "insert": "z" }]"#,
        );

        let Err(e) = pack_client(
            &data,
            true,
            &t.0.join("client-patch"),
            &t.0.join("packages"),
        ) else {
            panic!("expected an overlap refusal")
        };
        let err = e.to_string();
        assert!(
            err.contains("package alpha") && err.contains("package zeta"),
            "{err}"
        );
        assert!(err.contains("LootFrame.lua"), "{err}");
        assert!(err.contains("overlap"), "{err}");
        assert!(!data.join(PATCH_MPQ).exists(), "a refusal writes nothing");
    }

    /// An anchor the baseline holds twice could attach in two places, so the pack refuses rather
    /// than pick one.
    #[test]
    fn an_ambiguous_anchor_refuses_and_writes_nothing() {
        let t = Scratch::new("ui-ambiguous");
        let data = scratch_client(
            &t,
            &[("Interface\\FrameXML\\FrameXML.toc", FRAMEXML_TOC.as_bytes())],
        );
        t.write(
            "packages/loot/client/ui-transforms.json",
            br#"[{ "path": "Interface/FrameXML/FrameXML.toc",
                  "after": ".xml", "insert": "x" }]"#,
        );

        let Err(e) = pack_client(
            &data,
            true,
            &t.0.join("client-patch"),
            &t.0.join("packages"),
        ) else {
            panic!("expected an ambiguous-anchor refusal")
        };
        let err = e.to_string();
        assert!(err.contains("package loot"), "{err}");
        assert!(err.contains("FrameXML.toc"), "{err}");
        assert!(err.contains("ambiguous"), "{err}");
        assert!(!data.join(PATCH_MPQ).exists(), "a refusal writes nothing");
    }

    /// One source replaces a stock file whole while another patches it in place. The override would
    /// swallow the transform, so the pack refuses and names both sources and the path.
    #[test]
    fn an_override_of_a_transformed_path_refuses_and_writes_nothing() {
        let t = Scratch::new("ui-override");
        let data = scratch_client(
            &t,
            &[(
                "Interface\\FrameXML\\LootFrame.lua",
                LOOT_FRAME_LUA.as_bytes(),
            )],
        );
        t.write(
            "packages/alpha/client/mpq/Interface/FrameXML/LootFrame.lua",
            b"-- alpha owns the whole file\n",
        );
        t.write(
            "packages/zeta/client/ui-transforms.json",
            br#"[{ "path": "Interface/FrameXML/lootframe.lua",
                  "after": "function LootFrame_OnLoad()", "insert": "z" }]"#,
        );

        let Err(e) = pack_client(
            &data,
            true,
            &t.0.join("client-patch"),
            &t.0.join("packages"),
        ) else {
            panic!("expected an override-versus-transform refusal")
        };
        let err = e.to_string();
        assert!(
            err.contains("package alpha") && err.contains("package zeta"),
            "{err}"
        );
        assert!(err.contains("LootFrame.lua"), "{err}");
        assert!(!data.join(PATCH_MPQ).exists(), "a refusal writes nothing");
    }

    /// The licensing firewall for a transform: `--pack-out` never opens a client, so it refuses the
    /// declaration itself and names every Package and path behind it.
    #[test]
    fn pack_out_refuses_a_declared_transform_without_any_client() {
        let t = Scratch::new("ui-pack-out");
        t.write(
            "packages/loot/client/ui-transforms.json",
            br#"[{ "path": "Interface/FrameXML/LootFrame.lua",
                  "after": "function LootFrame_OnLoad()", "insert": "x" }]"#,
        );
        let out = t.0.join("artifact");

        let Err(e) = pack_out(&out, &t.0.join("client-patch"), &t.0.join("packages")) else {
            panic!("expected a provenance refusal")
        };
        let err = e.to_string();
        assert!(err.contains("package loot"), "{err}");
        assert!(err.contains("Interface\\FrameXML\\LootFrame.lua"), "{err}");
        assert!(err.contains("client sync"), "{err}");
        assert!(
            !out.exists(),
            "a refusal must leave the output dir untouched"
        );
        assert!(
            !t.0.join("client").exists(),
            "--pack-out must not need a client Data/ dir anywhere"
        );
    }

    /// The DBC read is the one client-version check. A client whose `AreaTable.dbc` is not the
    /// build-5875 schema fails there, before a single baseline byte is read.
    #[test]
    fn a_wrong_client_version_refuses_before_any_baseline_read() {
        let t = Scratch::new("ui-wrong-version");
        let data = t.0.join("client/Data");
        build_scratch_mpq(
            &data.join("dbc.MPQ"),
            &[("DBFilesClient\\AreaTable.dbc", b"not a 5875 DBC")],
        );
        // No interface.MPQ: reaching the baseline read would fail with a different message.
        t.write(
            "packages/loot/client/ui-transforms.json",
            br#"[{ "path": "Interface/FrameXML/LootFrame.lua",
                  "after": "function LootFrame_OnLoad()", "insert": "x" }]"#,
        );

        let Err(e) = pack_client(
            &data,
            true,
            &t.0.join("client-patch"),
            &t.0.join("packages"),
        ) else {
            panic!("expected a client-version refusal")
        };
        let err = e.to_string();
        assert!(err.contains("AreaTable.dbc"), "{err}");
        assert!(err.contains("wrong client version"), "{err}");
        assert!(!data.join(PATCH_MPQ).exists(), "a refusal writes nothing");
    }
}
