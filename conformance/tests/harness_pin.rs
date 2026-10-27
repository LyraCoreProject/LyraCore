use anyhow::{ensure, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use toml::Value;

#[path = "../support/scratch.rs"]
mod scratch;

fn agrees(pin: &str, workspace: &str) -> Result<String> {
    let fields: Vec<_> = pin
        .lines()
        .map(|line| line.split('#').next().unwrap().trim())
        .flat_map(str::split_whitespace)
        .collect();
    ensure!(fields.len() == 2, "pin needs one tag and full SHA");
    let [tag, sha] = fields.as_slice() else {
        unreachable!()
    };
    ensure!(tag.starts_with('v'), "pin must name a release tag");
    ensure!(
        sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "pin needs a full SHA"
    );
    let manifest: Value = toml::from_str(workspace)?;
    let dependency = &manifest["workspace"]["dependencies"]["wire-client"];
    ensure!(
        dependency["git"].as_str() == Some("https://github.com/LyraCoreProject/wire-harness"),
        "wrong harness repository"
    );
    ensure!(
        dependency.get("rev").and_then(Value::as_str) == Some(sha),
        "harness rev differs from pin"
    );
    ensure!(
        dependency
            .as_table()
            .context("harness dependency table")?
            .keys()
            .all(|key| matches!(key.as_str(), "git" | "rev")),
        "harness dependency must use only git and rev"
    );
    Ok(sha.to_string())
}

#[test]
fn harness_dependency_and_smoke_pin_name_the_same_commit() {
    let revision = agrees(
        include_str!("../../.wire-harness-rev"),
        include_str!("../../Cargo.toml"),
    )
    .unwrap();
    let output = Command::new("cargo")
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
        .args(["metadata", "--locked", "--format-version", "1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = serde_json::from_slice(&output.stdout).unwrap();
    workspace_consumers_match_pin(&metadata, &revision).unwrap();
    let home = cargo_home().unwrap();
    no_workspace_overrides(&metadata, &home).unwrap();
    resolved_harness_matches_pin(&metadata, &revision, &home).unwrap();
}

fn workspace_consumers_match_pin(metadata: &serde_json::Value, revision: &str) -> Result<()> {
    let expected = format!("git+https://github.com/LyraCoreProject/wire-harness?rev={revision}");
    let packages = metadata["packages"]
        .as_array()
        .context("workspace packages")?;
    for member in metadata["workspace_members"]
        .as_array()
        .context("workspace members")?
    {
        let package = packages
            .iter()
            .find(|package| package["id"] == *member)
            .context("missing workspace member")?;
        for dependency in package["dependencies"]
            .as_array()
            .context("package dependencies")?
        {
            if dependency["name"] == "wire-client" || dependency["rename"] == "wire-client" {
                ensure!(
                    dependency["name"] == "wire-client"
                        && dependency["source"].as_str() == Some(&expected),
                    "{} wire-client source or rev differs from the harness pin: {}",
                    package["name"],
                    dependency["source"]
                );
                ensure!(
                    dependency["kind"] == "dev",
                    "{} has a non-dev wire-client dependency",
                    package["name"]
                );
            }
        }
    }
    Ok(())
}

fn cargo_home() -> Result<PathBuf> {
    std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".cargo")))
        .context("CARGO_HOME or HOME")
}

fn resolved_harness_matches_pin(
    metadata: &serde_json::Value,
    revision: &str,
    home: &Path,
) -> Result<()> {
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .context("resolved dependency graph")?;
    let packages = metadata["packages"]
        .as_array()
        .context("resolved packages")?;
    let harnesses: Vec<_> = packages
        .iter()
        .filter(|package| {
            package["name"] == "wire-client" && nodes.iter().any(|node| node["id"] == package["id"])
        })
        .collect();
    ensure!(
        harnesses.len() == 1,
        "expected exactly one resolved wire-client package"
    );
    let expected =
        format!("git+https://github.com/LyraCoreProject/wire-harness?rev={revision}#{revision}");
    ensure!(
        harnesses[0]["source"].as_str() == Some(&expected),
        "resolved wire-client source differs from the harness pin"
    );
    let manifest = Path::new(
        harnesses[0]["manifest_path"]
            .as_str()
            .context("harness manifest path")?,
    )
    .canonicalize()?;
    let checkouts = home.join("git/checkouts").canonicalize()?;
    ensure!(
        manifest.starts_with(&checkouts),
        "wire-client manifest is outside Cargo git checkouts"
    );
    let git = |args: &[&str]| -> Result<String> {
        let output = Command::new("git")
            .current_dir(manifest.parent().context("harness directory")?)
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .args(args)
            .output()?;
        ensure!(
            output.status.success(),
            "could not inspect wire-client checkout"
        );
        Ok(String::from_utf8(output.stdout)?.trim().into())
    };
    let checkout = PathBuf::from(git(&["rev-parse", "--show-toplevel"])?).canonicalize()?;
    ensure!(
        checkout.starts_with(&checkouts),
        "wire-client is not in a Cargo git checkout"
    );
    ensure!(
        git(&["rev-parse", "HEAD"])? == revision,
        "wire-client checkout HEAD differs from the harness pin"
    );
    Ok(())
}

fn no_patch(manifest: &Value) -> Result<()> {
    if let Some(sources) = manifest.get("patch").and_then(Value::as_table) {
        for entries in sources.values() {
            for (name, entry) in entries.as_table().context("patch table")? {
                ensure!(
                    name != "wire-client"
                        && entry.get("package").and_then(Value::as_str) != Some("wire-client"),
                    "wire-client patch overrides are forbidden"
                );
            }
        }
    }
    Ok(())
}

fn no_workspace_overrides(metadata: &serde_json::Value, home: &Path) -> Result<()> {
    let root = Path::new(
        metadata["workspace_root"]
            .as_str()
            .context("workspace root")?,
    );
    let mut manifests = std::collections::BTreeSet::from([root.join("Cargo.toml")]);
    for member in metadata["workspace_members"]
        .as_array()
        .context("workspace members")?
    {
        let package = metadata["packages"]
            .as_array()
            .context("packages")?
            .iter()
            .find(|package| package["id"] == *member)
            .context("workspace package")?;
        manifests.insert(
            package["manifest_path"]
                .as_str()
                .context("manifest path")?
                .into(),
        );
    }
    let mut directories = std::collections::BTreeSet::new();
    for manifest in manifests {
        no_patch(&toml::from_str(&std::fs::read_to_string(&manifest)?)?)
            .with_context(|| format!("{}", manifest.display()))?;
        directories.extend(
            manifest
                .parent()
                .context("manifest directory")?
                .ancestors()
                .map(Path::to_path_buf),
        );
    }
    check_config_directory(home)?;
    for directory in directories {
        check_config_directory(&directory.join(".cargo"))?;
    }
    Ok(())
}

fn check_config_directory(directory: &Path) -> Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if !path.is_file()
            || !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("config")
        {
            continue;
        }
        let config: Value = toml::from_str(&std::fs::read_to_string(&path)?)?;
        ensure!(
            config.get("paths").is_none(),
            "paths overrides are forbidden: {}",
            path.display()
        );
        ensure!(
            config.get("source").is_none() && !contains_replace_with(&config),
            "Cargo source replacement is forbidden: {}",
            path.display()
        );
        no_patch(&config)?;
    }
    Ok(())
}

fn contains_replace_with(value: &Value) -> bool {
    match value {
        Value::Table(table) => {
            table.contains_key("replace-with") || table.values().any(contains_replace_with)
        }
        Value::Array(array) => array.iter().any(contains_replace_with),
        _ => false,
    }
}

#[test]
fn a_patched_workspace_keeps_declared_sources_but_fails_resolved_and_override_checks() -> Result<()>
{
    let revision = agrees(
        include_str!("../../.wire-harness-rev"),
        include_str!("../../Cargo.toml"),
    )?;
    let root = scratch::Scratch::new("pin-test")?;
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::create_dir_all(root.join("fork/src"))?;
    std::fs::write(root.join("src/lib.rs"), "")?;
    std::fs::write(root.join("fork/src/lib.rs"), "")?;
    std::fs::write(
        root.join("fork/Cargo.toml"),
        "[package]\nname = 'wire-client'\nversion = '0.1.0'\nedition = '2021'\n",
    )?;
    std::fs::write(root.join("Cargo.toml"), format!("[workspace]\n[package]\nname = 'pin-test'\nversion = '0.1.0'\nedition = '2021'\n[dev-dependencies]\nwire-client = {{ git = 'https://github.com/LyraCoreProject/wire-harness', rev = '{revision}' }}\n[patch.'https://github.com/LyraCoreProject/wire-harness']\nwire-client = {{ path = 'fork' }}\n"))?;
    let output = Command::new("cargo")
        .current_dir(&*root)
        .args(["metadata", "--offline", "--format-version", "1"])
        .output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = serde_json::from_slice(&output.stdout)?;
    workspace_consumers_match_pin(&metadata, &revision)?;
    assert!(resolved_harness_matches_pin(&metadata, &revision, &cargo_home()?).is_err());
    assert!(no_workspace_overrides(&metadata, &cargo_home()?).is_err());
    std::fs::create_dir(root.join(".cargo"))?;
    for filename in ["config", "config.toml"] {
        let path = root.join(".cargo").join(filename);
        std::fs::write(&path, "paths = ['../fork']")?;
        assert!(check_config_directory(&root.join(".cargo")).is_err());
        std::fs::remove_file(path)?;
    }
    no_patch(&toml::from_str(
        "[patch.crates-io]\nunrelated = { path = 'other' }",
    )?)?;
    assert!(no_patch(&toml::from_str(
        "[patch.crates-io]\nrenamed = { package = 'wire-client', path = 'fork' }"
    )?)
    .is_err());
    Ok(())
}

#[test]
fn every_workspace_member_checks_harness_source_revision_and_dev_kind() {
    use serde_json::json;
    let mut metadata = json!({
        "workspace_members": ["module", "conformance", "another_crate"],
        "packages": [
            {"id":"module", "name":"module", "dependencies":[]},
            {"id":"conformance", "name":"conformance", "dependencies":[]},
            {"id":"another_crate", "name":"another_crate", "dependencies":[
                {"name":"wire-client", "source":"git+https://github.com/LyraCoreProject/wire-harness?rev=abc", "rename":"renamed_harness", "kind":"dev", "target":"cfg(windows)", "optional":true}
            ]}
        ]
    });
    assert!(workspace_consumers_match_pin(&metadata, "abc").is_ok());
    let original = metadata.clone();
    for source in [
        serde_json::Value::Null,
        json!("registry+https://github.com/rust-lang/crates.io-index"),
        json!("git+https://github.com/elsewhere/wire-harness?rev=abc"),
        json!("git+https://github.com/LyraCoreProject/wire-harness?rev=def"),
        json!("git+https://github.com/LyraCoreProject/wire-harness?branch=abc"),
        json!("git+https://github.com/LyraCoreProject/wire-harness?tag=abc"),
    ] {
        metadata["packages"][2]["dependencies"][0]["source"] = source;
        assert!(workspace_consumers_match_pin(&metadata, "abc")
            .unwrap_err()
            .to_string()
            .contains("another_crate"));
    }
    metadata = original.clone();
    metadata["packages"][2]["dependencies"][0]["name"] = json!("other-package");
    metadata["packages"][2]["dependencies"][0]["rename"] = json!("wire-client");
    assert!(workspace_consumers_match_pin(&metadata, "abc").is_err());
    metadata = original;
    for kind in [serde_json::Value::Null, json!("build")] {
        metadata["packages"][2]["dependencies"][0]["kind"] = kind;
        assert!(workspace_consumers_match_pin(&metadata, "abc")
            .unwrap_err()
            .to_string()
            .contains("another_crate"));
    }
}

#[test]
fn the_pin_check_detects_drift_and_unpinned_dependencies() {
    let pin = include_str!("../../.wire-harness-rev");
    let manifest = include_str!("../../Cargo.toml");
    let parsed: Value = toml::from_str(manifest).unwrap();
    let sha = parsed["workspace"]["dependencies"]["wire-client"]["rev"]
        .as_str()
        .unwrap();
    assert!(agrees(&pin.replace(sha, &"0".repeat(40)), manifest).is_err());
    assert!(agrees(&pin.replace(sha, "short"), manifest).is_err());
    assert!(agrees(pin, &manifest.replace(sha, &"0".repeat(40))).is_err());
    assert!(agrees(pin, &manifest.replace("wire-harness\"", "different\"")).is_err());
    assert!(agrees(pin, &manifest.replace("rev =", "branch =")).is_err());
}

#[test]
fn every_cargo_config_location_rejects_sources_and_replacements() -> Result<()> {
    let root = scratch::Scratch::new("config-test")?;
    let workspace = root.join("workspace");
    let member = workspace.join("member");
    let home = root.join("cargo-home");
    std::fs::create_dir_all(&member)?;
    for manifest in [workspace.join("Cargo.toml"), member.join("Cargo.toml")] {
        std::fs::write(manifest, "")?;
    }
    let metadata = serde_json::json!({"workspace_root": workspace, "workspace_members": ["member"],
        "packages": [{"id": "member", "manifest_path": member.join("Cargo.toml")}]});
    for directory in [
        root.join(".cargo"),
        workspace.join(".cargo"),
        member.join(".cargo"),
        home.clone(),
    ] {
        std::fs::create_dir_all(&directory)?;
        for filename in ["config", "config.toml", "config.backup"] {
            let path = directory.join(filename);
            for contents in [
                "[source.vend]\ndirectory = 'vendor'\n",
                "[source.pinned]\nreplace-with = 'vend'\n",
                "[build]\nreplace-with = 'vend'\n",
                "paths = ['fork']\n",
            ] {
                std::fs::write(&path, contents)?;
                assert!(
                    no_workspace_overrides(&metadata, &home).is_err(),
                    "{}: {contents}",
                    path.display()
                );
            }
            std::fs::remove_file(path)?;
        }
    }
    no_workspace_overrides(&metadata, &home)
}

#[test]
fn directory_replacement_preserves_metadata_source_but_cannot_pass_the_pin_check() -> Result<()> {
    let revision = agrees(
        include_str!("../../.wire-harness-rev"),
        include_str!("../../Cargo.toml"),
    )?;
    let root = scratch::Scratch::new("replacement-test")?;
    for path in ["src", "vendor/wire-client/src", ".cargo"] {
        std::fs::create_dir_all(root.join(path))?;
    }
    std::fs::write(root.join("src/lib.rs"), "")?;
    std::fs::write(
        root.join("vendor/wire-client/src/lib.rs"),
        "pub const TAMPERED: bool = true;",
    )?;
    std::fs::write(
        root.join("vendor/wire-client/Cargo.toml"),
        "[package]\nname = 'wire-client'\nversion = '0.1.0'\nedition = '2021'\n",
    )?;
    std::fs::write(
        root.join("vendor/wire-client/.cargo-checksum.json"),
        "{\"files\":{},\"package\":null}",
    )?;
    std::fs::write(root.join("Cargo.toml"), format!("[workspace]\n[package]\nname = 'replacement-test'\nversion = '0.1.0'\nedition = '2021'\n[dev-dependencies]\nwire-client = {{ git = 'https://github.com/LyraCoreProject/wire-harness', rev = '{revision}' }}\n"))?;
    std::fs::write(root.join("Cargo.lock"), format!("version = 4\n[[package]]\nname = 'replacement-test'\nversion = '0.1.0'\ndependencies = ['wire-client']\n[[package]]\nname = 'wire-client'\nversion = '0.1.0'\nsource = 'git+https://github.com/LyraCoreProject/wire-harness?rev={revision}#{revision}'\n"))?;
    std::fs::write(root.join(".cargo/config.toml"), format!("[source.pinned]\ngit = 'https://github.com/LyraCoreProject/wire-harness'\nrev = '{revision}'\nreplace-with = 'vend'\n[source.vend]\ndirectory = 'vendor'\n"))?;
    let output = Command::new("cargo")
        .current_dir(&*root)
        .args(["metadata", "--locked", "--offline", "--format-version", "1"])
        .output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    workspace_consumers_match_pin(&metadata, &revision)?;
    let harness = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "wire-client")
        .unwrap();
    assert_eq!(
        harness["source"],
        format!("git+https://github.com/LyraCoreProject/wire-harness?rev={revision}#{revision}")
    );
    let home = cargo_home()?;
    assert!(no_workspace_overrides(&metadata, &home)
        .unwrap_err()
        .to_string()
        .contains("source replacement"));
    assert!(resolved_harness_matches_pin(&metadata, &revision, &home)
        .unwrap_err()
        .to_string()
        .contains("outside Cargo git checkouts"));
    Ok(())
}

#[test]
fn checkout_head_and_canonical_manifest_path_must_match_the_pin() -> Result<()> {
    let home = scratch::Scratch::new("checkout-test")?;
    let checkout = home.join("git/checkouts/harness/revision");
    std::fs::create_dir_all(&checkout)?;
    let git = |args: &[&str]| -> Result<String> {
        let output = Command::new("git")
            .current_dir(&checkout)
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .args(args)
            .output()?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(String::from_utf8(output.stdout)?.trim().into())
    };
    git(&["init", "--quiet"])?;
    std::fs::write(checkout.join("Cargo.toml"), "")?;
    git(&["add", "."])?;
    git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "-m",
        "fixture",
    ])?;
    let revision = git(&["rev-parse", "HEAD"])?;
    let mut metadata = serde_json::json!({"resolve":{"nodes":[{"id":"harness"}]}, "packages":[{
        "id":"harness", "name":"wire-client", "manifest_path":checkout.join("Cargo.toml"),
        "source":format!("git+https://github.com/LyraCoreProject/wire-harness?rev={revision}#{revision}")} ]});
    resolved_harness_matches_pin(&metadata, &revision, &home)?;
    git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "--allow-empty",
        "-m",
        "changed",
    ])?;
    assert!(resolved_harness_matches_pin(&metadata, &revision, &home)
        .unwrap_err()
        .to_string()
        .contains("HEAD differs"));
    std::fs::write(home.join("Cargo.toml"), "")?;
    metadata["packages"][0]["manifest_path"] = serde_json::json!(home.join("Cargo.toml"));
    assert!(resolved_harness_matches_pin(&metadata, &revision, &home)
        .unwrap_err()
        .to_string()
        .contains("outside Cargo git checkouts"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(home.join("Cargo.toml"), checkout.join("symlink.toml"))?;
        metadata["packages"][0]["manifest_path"] = serde_json::json!(checkout.join("symlink.toml"));
        assert!(resolved_harness_matches_pin(&metadata, &revision, &home)
            .unwrap_err()
            .to_string()
            .contains("outside Cargo git checkouts"));
    }
    Ok(())
}
