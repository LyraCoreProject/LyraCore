use std::path::Path;
use std::process::Command;

use anyhow::{ensure, Result};
use lyracore_importer::spell_kind::{supported_effect, EffectSlot};
use serde::Deserialize;

#[derive(Deserialize)]
struct Spell {
    name: String,
    slots: Vec<EffectSlot>,
}

#[derive(Deserialize)]
struct Class {
    class: String,
    spells: Vec<Spell>,
}

#[derive(Deserialize)]
struct Catalogue {
    missing: Option<String>,
    path: Option<String>,
    classes: Option<Vec<Class>>,
}

pub fn print(root: &Path, path: Option<&Path>) -> Result<()> {
    let mut command = Command::new("python3");
    command.arg(root.join("conformance/spell_counts.py"));
    if let Some(path) = path {
        command.arg("--vmangos").arg(path);
    }
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "spell counts: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let catalogue: Catalogue = serde_json::from_slice(&output.stdout)?;
    if let Some(expected) = catalogue.missing {
        println!("\nTrainable spells: no data; expected {expected}");
        return Ok(());
    }
    println!(
        "\nTrainable spells: build 5875; {}",
        catalogue.path.unwrap_or_default()
    );
    println!("Active class-trainer offers plus automatic class-skill learning; teaching spells resolved, ranks counted separately.");
    println!("Supported means every nonempty effect and aura kind maps to a supported importer kind; this does not establish spell conformance.");
    println!("\n| Class | Trainable | Supported kinds | Unsupported kinds |\n| --- | ---: | ---: | ---: |");
    for class in catalogue.classes.unwrap_or_default() {
        let supported = class
            .spells
            .iter()
            .filter(|spell| {
                !spell.slots.is_empty()
                    && spell
                        .slots
                        .iter()
                        .all(|slot| supported_effect(&spell.name, slot))
            })
            .count();
        println!(
            "| {} | {} | {supported} | {} |",
            class.class,
            class.spells.len(),
            class.spells.len() - supported
        );
    }
    Ok(())
}

#[test]
fn sqlite_counts_follow_learning_data_and_build_boundaries() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let output = Command::new("python3")
        .current_dir(root)
        .args([
            "-B",
            "-m",
            "unittest",
            "discover",
            "-s",
            "conformance/tests",
            "-p",
            "test_spell_counts.py",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
