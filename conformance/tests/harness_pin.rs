use anyhow::{ensure, Context, Result};
use std::path::Path;
use std::process::Command;
use toml::Value;

fn agrees(pin: &str, workspace: &str) -> Result<()> {
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
    Ok(())
}

#[test]
fn harness_dependency_and_smoke_pin_name_the_same_commit() {
    agrees(
        include_str!("../../.wire-harness-rev"),
        include_str!("../../Cargo.toml"),
    )
    .unwrap();
    for source in [
        include_str!("../Cargo.toml"),
        include_str!("../../module/Cargo.toml"),
    ] {
        let manifest: Value = toml::from_str(source).unwrap();
        assert_eq!(
            manifest["dev-dependencies"]["wire-client"]["workspace"].as_bool(),
            Some(true)
        );
        assert!(manifest["dependencies"].get("wire-client").is_none());
    }
    let output = Command::new("cargo")
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
        .args(["metadata", "--locked", "--no-deps", "--format-version", "1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    workspace_consumers_are_dev_only(&serde_json::from_slice(&output.stdout).unwrap()).unwrap();
}

fn workspace_consumers_are_dev_only(metadata: &serde_json::Value) -> Result<()> {
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
            if dependency["name"] == "wire-client" {
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

#[test]
fn every_workspace_member_rejects_non_dev_harness_dependencies() {
    use serde_json::json;
    let mut metadata = json!({
        "workspace_members": ["module", "conformance", "another_crate"],
        "packages": [
            {"id":"module", "name":"module", "dependencies":[]},
            {"id":"conformance", "name":"conformance", "dependencies":[]},
            {"id":"another_crate", "name":"another_crate", "dependencies":[
                {"name":"wire-client", "rename":"renamed_harness", "kind":"dev", "target":"cfg(windows)", "optional":true}
            ]}
        ]
    });
    assert!(workspace_consumers_are_dev_only(&metadata).is_ok());
    for kind in [serde_json::Value::Null, json!("build")] {
        metadata["packages"][2]["dependencies"][0]["kind"] = kind;
        assert!(workspace_consumers_are_dev_only(&metadata)
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
