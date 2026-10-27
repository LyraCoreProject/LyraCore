use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{ensure, Context, Result};
use lyracore_shared::spell::taxonomy::EffectKind;

// Read conformance/README.md#trainable-spells before changing these raw-kind mappings.
const EFFECTS: &[(u32, EffectKind)] = &[
    (2, EffectKind::Damage),
    (5, EffectKind::Blink),
    (6, EffectKind::AuraFlag),
    (10, EffectKind::Heal),
    (17, EffectKind::WeaponStrike),
    (18, EffectKind::Resurrect),
    (24, EffectKind::CreateItem),
    (27, EffectKind::PersistentArea),
    (29, EffectKind::Blink),
    (30, EffectKind::Energize),
    (31, EffectKind::WeaponStrike),
    (33, EffectKind::OpenLock),
    (35, EffectKind::AuraFlag),
    (38, EffectKind::Dispel),
    (53, EffectKind::EnchantItem),
    (54, EffectKind::EnchantItem),
    (55, EffectKind::TameCreature),
    (56, EffectKind::SummonPet),
    (58, EffectKind::WeaponStrike),
    (62, EffectKind::PowerBurn),
    (64, EffectKind::Trigger),
    (67, EffectKind::HealMaxHealth),
    (68, EffectKind::Interrupt),
    (69, EffectKind::Distract),
    (71, EffectKind::Pickpocket),
    (80, EffectKind::AddCombo),
    (83, EffectKind::Duel),
    (94, EffectKind::SelfResurrect),
    (96, EffectKind::Charge),
    (99, EffectKind::Disenchant),
    (101, EffectKind::FeedPet),
    (112, EffectKind::SummonHostile),
    (113, EffectKind::Resurrect),
    (114, EffectKind::Taunt),
    (121, EffectKind::WeaponStrike),
    (125, EffectKind::ReduceThreat),
];

const AURAS: &[(u32, EffectKind)] = &[
    (3, EffectKind::AuraPeriodicDamage),
    (7, EffectKind::AuraControl),
    (8, EffectKind::AuraPeriodicHeal),
    (9, EffectKind::AuraModSpeed),
    (10, EffectKind::AuraModCombat),
    (12, EffectKind::AuraControl),
    (13, EffectKind::AuraModCombat),
    (14, EffectKind::AuraModDamageTaken),
    (15, EffectKind::AuraRetaliate),
    (16, EffectKind::AuraStealth),
    (22, EffectKind::AuraModResistance),
    (23, EffectKind::AuraPeriodicTrigger),
    (24, EffectKind::AuraPeriodicEnergize),
    (26, EffectKind::AuraControl),
    (27, EffectKind::AuraFlag),
    (29, EffectKind::AuraModStat),
    (31, EffectKind::AuraModSpeed),
    (32, EffectKind::AuraModSpeed),
    (33, EffectKind::AuraModSpeed),
    (34, EffectKind::AuraModHealthPower),
    (35, EffectKind::AuraModHealthPower),
    (36, EffectKind::SetStance),
    (39, EffectKind::AuraImmunity),
    (42, EffectKind::AuraProcTrigger),
    (43, EffectKind::AuraProcDamage),
    (47, EffectKind::AuraModCombat),
    (49, EffectKind::AuraModCombat),
    (51, EffectKind::AuraModCombat),
    (52, EffectKind::AuraModCombat),
    (54, EffectKind::AuraModCombat),
    (55, EffectKind::AuraModCombat),
    (56, EffectKind::AuraTransform),
    (57, EffectKind::AuraModCombat),
    (58, EffectKind::AuraModSpeed),
    (65, EffectKind::AuraModSpeed),
    (67, EffectKind::AuraDisarm),
    (69, EffectKind::AuraAbsorb),
    (77, EffectKind::AuraImmunity),
    (78, EffectKind::AuraMounted),
    (80, EffectKind::AuraModStatPct),
    (87, EffectKind::AuraModDamageTaken),
    (91, EffectKind::AuraModDetectRange),
    (99, EffectKind::AuraModCombat),
    (104, EffectKind::AuraFlag),
    (105, EffectKind::AuraFlag),
    (106, EffectKind::AuraFlag),
    (107, EffectKind::AuraSpellmodFlat),
    (108, EffectKind::AuraSpellmodPct),
    (110, EffectKind::AuraModPowerRegenPct),
    (124, EffectKind::AuraModCombat),
    (129, EffectKind::AuraModSpeed),
    (130, EffectKind::AuraModSpeed),
    (140, EffectKind::AuraModSpeed),
    (141, EffectKind::AuraModSpeed),
    (161, EffectKind::AuraCombatHealthRegenPct),
];

pub fn print(root: &Path, path: Option<&Path>) -> Result<()> {
    let mut command = Command::new("python3");
    command.arg(root.join("conformance/spell_counts.py"));
    if let Some(path) = path {
        command.arg("--vmangos").arg(path);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let capabilities = serde_json::json!({
        "effects": EFFECTS.iter().map(|(raw, _)| raw).collect::<Vec<_>>(),
        "auras": AURAS.iter().map(|(raw, _)| raw).collect::<Vec<_>>(),
    });
    child
        .stdin
        .take()
        .context("spell counter stdin")?
        .write_all(&serde_json::to_vec(&capabilities)?)?;
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "spell counts: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8(output.stdout)?);
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
