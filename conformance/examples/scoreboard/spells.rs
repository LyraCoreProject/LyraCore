use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{ensure, Context, Result};
use lyracore_shared::spell::taxonomy::*;
use wow_world_base::vanilla::AuraMod;

// Re-derive these mappings after the vmangos port; see conformance/README.md#trainable-spells.
fn instant_effect_to_kind(effect_id: i32) -> u8 {
    match effect_id {
        2 | 17 | 58 | 121 | 31 => E_DAMAGE,
        10 => E_HEAL,
        67 => E_HEAL_MAX_HEALTH,
        30 => E_ENERGIZE,
        38 => E_DISPEL,
        64 => E_TRIGGER,
        63 | 91 => E_TAUNT,
        114 => E_TAUNT,
        24 => E_CREATE_ITEM,
        68 => E_INTERRUPT,
        55 => E_TAME_CREATURE,
        101 => E_FEED_PET,
        56 => E_SUMMON_PET,
        62 => E_POWER_BURN,
        83 => E_DUEL,
        69 => E_DISTRACT,
        33 | 59 => E_OPEN_LOCK,
        99 => E_DISENCHANT,
        80 => E_ADD_COMBO,
        94 => E_SELF_RESURRECT,
        18 | 113 => E_RESURRECT,
        _ => E_SCRIPTED,
    }
}

fn aura_mod_to_kind(aura: AuraMod) -> u8 {
    use AuraMod::*;
    match aura {
        PeriodicDamage | PeriodicLeech | PeriodicDamagePercent => A_PERIODIC_DAMAGE,
        PeriodicHeal => A_PERIODIC_HEAL,
        PeriodicEnergize | PeriodicManaLeech | PowerBurnMana => A_PERIODIC_ENERGIZE,
        PeriodicTriggerSpell => E_TRIGGER,
        ProcTriggerSpell => A_PROC_TRIGGER,
        ProcTriggerDamage => A_PROC_DAMAGE,

        AddFlatModifier => A_SPELLMOD_FLAT,
        AddPctModifier => A_SPELLMOD_PCT,

        ModStat => A_MOD_STAT,
        ModPercentStat | ModTotalStatPercentage => A_MOD_STAT_PCT,

        ModResistance
        | ModBaseResistance
        | ModResistancePct
        | ModBaseResistancePct
        | ModResistanceOfStatPercent => A_MOD_RESISTANCE,

        SchoolAbsorb | DamageShield | ManaShield => A_ABSORB,

        ModAttackPower
        | ModRangedAttackPower
        | ModMeleeAttackPowerVersus
        | ModRangedAttackPowerVersus
        | ModAttackPowerPct
        | ModRangedAttackPowerPct => A_MOD_COMBAT,
        ModCritPercent | ModSpellCritChance | ModSpellCritChanceSchool | ModCritPercentVersus => {
            A_MOD_COMBAT
        }
        ModHitChance | ModSpellHitChance => A_MOD_COMBAT,
        ModDamageDone | ModDamagePercentDone | ModDamageDoneCreature | ModDamageDoneVersus => {
            A_MOD_COMBAT
        }
        ModHealing | ModHealingDone => A_MOD_COMBAT,
        ModDodgePercent | ModTotalThreat | ModThreat => A_MOD_COMBAT,

        ModIncreaseSpeed | ModDecreaseSpeed | ModSpeedAlways | ModSpeedNotStack => A_MOD_SPEED,
        ModIncreaseMountedSpeed | ModMountedSpeedAlways | ModMountedSpeedNotStack => A_MOD_SPEED,
        ModIncreaseSwimSpeed => A_MOD_SPEED,
        ModAttackspeed
        | ModMeleeHaste
        | ModRangedHaste
        | ModRangedAmmoHaste
        | ModCastingSpeedNotStack => A_MOD_SPEED,

        ModDamagePercentTaken => A_MOD_DAMAGE_TAKEN,

        ModIncreaseHealth | ModIncreaseEnergy | ObsModHealth | ObsModMana => A_MOD_HEALTH_POWER,

        ModStun => A_CONTROL,
        ModRoot => A_CONTROL,
        ModFear => A_CONTROL,
        ModConfuse | ModCharm | ModPossess | ModPacify | ModPacifySilence | AoeCharm => A_CONTROL,

        SchoolImmunity | DamageImmunity | EffectImmunity | StateImmunity | MechanicImmunity
        | MechanicImmunityMask => A_IMMUNITY,

        ModRegenDuringCombat | ModHealthRegenInCombat => A_COMBAT_HEALTH_REGEN_PCT,

        ModRegen => A_PERIODIC_HEAL,
        ModPowerRegen => A_PERIODIC_ENERGIZE,
        ModPowerRegenPercent => A_MOD_POWER_REGEN_PCT,

        ModDisarm => A_DISARM,
        Mounted => A_MOUNTED,
        Transform => A_TRANSFORM,
        ModDetectRange => A_MOD_DETECT_RANGE,

        Dummy
        | BindSight
        | ModTaunt
        | ModStealth
        | ModStealthDetect
        | ModInvisibility
        | ModInvisibilityDetection
        | ModShapeshift
        | DispelImmunity
        | TrackCreatures
        | TrackResources
        | ModScale
        | ModStalked
        | ModLanguage
        | FarSight
        | WaterBreathing
        | ModHealthRegenPercent
        | PreventsFleeing
        | ModUnattackable
        | InterruptRegen
        | Ghost
        | AurasVisible
        | WaterWalk
        | FeatherFall
        | Hover
        | SharePetTracking
        | Untrackable
        | SafeFall
        | Persuaded
        | RetainComboPoints
        | ResistPushback
        | TrackStealthed
        | ModWaterBreathing
        | NoPvpCredit
        | ModAoeAvoidance
        | FeignDeath
        | ModSilence
        | SpiritOfRedemption
        | AllowChampionSpells
        | UseNormalMovementSpeed => A_FLAG,

        _ => E_SCRIPTED,
    }
}

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
        "effects": (0..256).filter(|raw| instant_effect_to_kind(*raw) != E_SCRIPTED).collect::<Vec<_>>(),
        "auras": (0..256).filter(|raw| AuraMod::from_int(*raw).is_ok_and(|aura| aura_mod_to_kind(aura) != E_SCRIPTED)).collect::<Vec<_>>(),
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

#[test]
fn supported_mappings_match_the_importer() {
    let importer = syn::parse_file(include_str!("../../../importer/src/spell.rs")).unwrap();
    let scoreboard = syn::parse_file(include_str!("spells.rs")).unwrap();
    for name in ["instant_effect_to_kind", "aura_mod_to_kind"] {
        let find = |file: syn::File| {
            file.items
                .into_iter()
                .find_map(|item| match item {
                    syn::Item::Fn(function) if function.sig.ident == name => Some(function),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(
            find(importer.clone()),
            find(scoreboard.clone()),
            "re-derive spell support after importer mapping changes: {name}"
        );
    }
}
