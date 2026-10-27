use anyhow::{ensure, Result};

use crate::dump::TableRows;
use crate::spell_dbc::Spell;
use crate::world_source::Row;

fn array<T: std::str::FromStr + Copy + Default, const N: usize>(
    row: &Row,
    prefix: &str,
) -> Result<[T; N]> {
    let mut values = [T::default(); N];
    for (index, value) in values.iter_mut().enumerate() {
        *value = row.number(&format!("{prefix}{}", index + 1))?;
    }
    Ok(values)
}

fn word(row: &Row, column: &str) -> Result<u32> {
    let value: i64 = row.number(column)?;
    ensure!(
        (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(&value),
        "spell_template.{column}: value exceeds 32 bits"
    );
    Ok(value as u32)
}

fn source_spell(row: &Row) -> Result<Spell> {
    let family: u8 = row.number("spellFamilyName")?;
    let icon: u32 = row.number("spellIconId")?;
    let family_flags: u64 = row.number("spellFamilyFlags")?;
    let category = if family == 3
        && family_flags & 0x12000000 == 0
        && row.number::<u32>("effectApplyAuraName1")? == 5
        && row.number::<u32>("preventionType")? == 1
    {
        2
    } else if family == 10
        && family_flags & (0x0a000200 | 0x10000100) == 0
        && ((family_flags & 0x20180400 != 0 && row.number::<u32>("baseLevel")? != 0)
            || (icon == 561 && row.number::<u32>("spellVisual1")? == 5652))
    {
        1
    } else {
        0
    };

    let spell = Spell {
        single_target_key: lyracore_shared::spell_custom::single_target_key(family, icon, category),
        custom_flags: row.number("customFlags")?,
        min_target_level: row.number("minTargetLevel")?,
        bonus_coefficient: Some(array(row, "effectBonusCoefficient")?),
        id: row.number("entry")?,
        school: row.number("school")?,
        dispel_type: row.number("dispel")?,
        mechanic: row.number("mechanic")?,
        attributes: row.number("attributes")?,
        attributes_ex1: row.number("attributesEx")?,
        attributes_ex2: row.number("attributesEx2")?,
        shapeshift_mask: row.number("stances")?,
        targets: row.number("targets")?,
        requires_spell_focus: row.number("requiresSpellFocus")?,
        casting_time_index: row.number("castingTimeIndex")?,
        recovery_time: row.number("recoveryTime")?,
        category_recovery_time: row.number("categoryRecoveryTime")?,
        aura_interrupt_flags: row.number("auraInterruptFlags")?,
        proc_flags: row.number("procFlags")?,
        proc_chance: row.number("procChance")?,
        proc_charges: row.number("procCharges")?,
        max_level: row.number("maxLevel")?,
        spell_level: row.number("spellLevel")?,
        duration: row.number("durationIndex")?,
        power_type: word(row, "powerType")?,
        mana_cost: row.number("manaCost")?,
        range: row.number("rangeIndex")?,
        stack_amount: row.number("stackAmount")?,
        reagent: array(row, "reagent")?,
        reagent_count: array(row, "reagentCount")?,
        equipment: [
            word(row, "equippedItemClass")?,
            word(row, "equippedItemSubClassMask")?,
            word(row, "equippedItemInventoryTypeMask")?,
        ],
        effect: array(row, "effect")?,
        effect_die_sides: array(row, "effectDieSides")?,
        effect_base_dice: array(row, "effectBaseDice")?,
        effect_dice_per_level: array(row, "effectDicePerLevel")?,
        effect_real_points_per_level: array(row, "effectRealPointsPerLevel")?,
        effect_base_points: array(row, "effectBasePoints")?,
        effect_mechanic: array(row, "effectMechanic")?,
        implicit_target_a: array(row, "effectImplicitTargetA")?,
        implicit_target_b: array(row, "effectImplicitTargetB")?,
        effect_radius: array(row, "effectRadiusIndex")?,
        effect_aura: array(row, "effectApplyAuraName")?,
        effect_amplitude: array(row, "effectAmplitude")?,
        effect_multiple_values: array(row, "effectMultipleValue")?,
        effect_chain_target: array(row, "effectChainTarget")?,
        effect_item_type: array(row, "effectItemType")?,
        effect_misc_value: array::<i32, 3>(row, "effectMiscValue")?.map(|n| n as u32),
        effect_trigger_spell: array(row, "effectTriggerSpell")?,
        effect_points_per_combo: array(row, "effectPointsPerComboPoint")?,
        name: row.text("name")?.into(),
        family_name: row.number("spellFamilyName")?,
        family_flags: row.number("spellFamilyFlags")?,
    };
    ensure!(
        spell
            .effect_dice_per_level
            .iter()
            .chain(&spell.effect_real_points_per_level)
            .chain(&spell.effect_multiple_values)
            .chain(&spell.effect_points_per_combo)
            .chain(spell.bonus_coefficient.iter().flatten())
            .all(|v| v.is_finite()),
        "spell {}: non-finite effect magnitude",
        spell.id
    );
    for (index, value) in spell.effect_item_type.iter().enumerate() {
        ensure!(
            spell.effect[index] != 24 || *value <= i32::MAX as u64,
            "spell {} effect {index}: item entry exceeds the Module's signed item parameter",
            spell.id
        );
    }
    Ok(spell)
}

pub(crate) fn read(source: &impl TableRows) -> Result<(Vec<Spell>, serde_json::Value)> {
    let mut scripts = Vec::new();
    let mut masks = Vec::new();
    let mut spells = Vec::new();
    let mut flags = Vec::new();
    let columns = [
        "spellIconId",
        "baseLevel",
        "spellVisual1",
        "preventionType",
        "customFlags",
        "minTargetLevel",
        "effectBonusCoefficient1",
        "effectBonusCoefficient2",
        "effectBonusCoefficient3",
        "attributes",
        "attributesEx",
        "attributesEx2",
        "auraInterruptFlags",
        "castingTimeIndex",
        "categoryRecoveryTime",
        "dispel",
        "durationIndex",
        "effect1",
        "effect2",
        "effect3",
        "effectAmplitude1",
        "effectAmplitude2",
        "effectAmplitude3",
        "effectApplyAuraName1",
        "effectApplyAuraName2",
        "effectApplyAuraName3",
        "effectBaseDice1",
        "effectBaseDice2",
        "effectBaseDice3",
        "effectBasePoints1",
        "effectBasePoints2",
        "effectBasePoints3",
        "effectChainTarget1",
        "effectChainTarget2",
        "effectChainTarget3",
        "effectDicePerLevel1",
        "effectDicePerLevel2",
        "effectDicePerLevel3",
        "effectDieSides1",
        "effectDieSides2",
        "effectDieSides3",
        "effectImplicitTargetA1",
        "effectImplicitTargetA2",
        "effectImplicitTargetA3",
        "effectImplicitTargetB1",
        "effectImplicitTargetB2",
        "effectImplicitTargetB3",
        "effectItemType1",
        "effectItemType2",
        "effectItemType3",
        "effectMechanic1",
        "effectMechanic2",
        "effectMechanic3",
        "effectMiscValue1",
        "effectMiscValue2",
        "effectMiscValue3",
        "effectMultipleValue1",
        "effectMultipleValue2",
        "effectMultipleValue3",
        "effectPointsPerComboPoint1",
        "effectPointsPerComboPoint2",
        "effectPointsPerComboPoint3",
        "effectRadiusIndex1",
        "effectRadiusIndex2",
        "effectRadiusIndex3",
        "effectRealPointsPerLevel1",
        "effectRealPointsPerLevel2",
        "effectRealPointsPerLevel3",
        "effectTriggerSpell1",
        "effectTriggerSpell2",
        "effectTriggerSpell3",
        "entry",
        "equippedItemClass",
        "equippedItemInventoryTypeMask",
        "equippedItemSubClassMask",
        "manaCost",
        "maxLevel",
        "mechanic",
        "name",
        "powerType",
        "procChance",
        "procCharges",
        "procFlags",
        "rangeIndex",
        "reagent1",
        "reagent2",
        "reagent3",
        "reagent4",
        "reagent5",
        "reagent6",
        "reagent7",
        "reagent8",
        "reagentCount1",
        "reagentCount2",
        "reagentCount3",
        "reagentCount4",
        "reagentCount5",
        "reagentCount6",
        "reagentCount7",
        "reagentCount8",
        "recoveryTime",
        "requiresSpellFocus",
        "school",
        "script_name",
        "spellFamilyFlags",
        "spellFamilyName",
        "spellLevel",
        "stackAmount",
        "stances",
        "targets",
    ];
    for row in source.columns("spell_template", &columns)?.iter() {
        let spell = source_spell(row)?;
        ensure!(
            spell.custom_flags & !lyracore_shared::spell_custom::KNOWN == 0,
            "spell {}: unknown customFlags bits",
            spell.id
        );
        if spell.custom_flags & lyracore_shared::spell_custom::UNSUPPORTED != 0 {
            flags.push(format!(
                "{}:0x{:x}",
                spell.id,
                spell.custom_flags & lyracore_shared::spell_custom::UNSUPPORTED
            ));
        }
        if !row
            .optional_text("script_name")?
            .unwrap_or_default()
            .is_empty()
        {
            scripts.push(spell.id);
        }
        for (index, value) in spell.effect_item_type.iter().enumerate() {
            if *value > u64::from(u32::MAX) {
                masks.push(format!("{}:{index}", spell.id));
            }
        }
        spells.push(spell);
    }
    let manifest = serde_json::json!({"format_version":1,"source":"vmangos","family":"spell","patch":crate::source_selection::PATCH,"client_build":crate::source_selection::CLIENT_BUILD,"findings":[
        {"code":"spells.script","classification":"unsupported","occurrences":scripts.len(),"source_ids":scripts},
        {"code":"spells.modifier-high-mask","classification":"unsupported","occurrences":masks.len(),"source_ids":masks},
        {"code":"spells.custom-flags","classification":"unsupported","occurrences":flags.len(),"source_ids":flags}
    ]});
    Ok((spells, manifest))
}

#[cfg(test)]
pub(crate) fn fixture() -> crate::world_source::WorldSource {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    let mut columns = "customFlags minTargetLevel spellIconId baseLevel spellVisual1 preventionType school dispel mechanic attributes attributesEx attributesEx2 stances targets requiresSpellFocus castingTimeIndex recoveryTime categoryRecoveryTime auraInterruptFlags procFlags procChance procCharges maxLevel spellLevel durationIndex powerType manaCost rangeIndex stackAmount equippedItemClass equippedItemSubClassMask equippedItemInventoryTypeMask spellFamilyName spellFamilyFlags".split_whitespace().map(str::to_owned).collect::<Vec<_>>();
    for prefix in [
        "effectBonusCoefficient",
        "effect",
        "effectDieSides",
        "effectBaseDice",
        "effectDicePerLevel",
        "effectRealPointsPerLevel",
        "effectBasePoints",
        "effectMechanic",
        "effectImplicitTargetA",
        "effectImplicitTargetB",
        "effectRadiusIndex",
        "effectApplyAuraName",
        "effectAmplitude",
        "effectMultipleValue",
        "effectChainTarget",
        "effectItemType",
        "effectMiscValue",
        "effectTriggerSpell",
        "effectPointsPerComboPoint",
    ] {
        for n in 1..=3 {
            columns.push(format!("{prefix}{n}"));
        }
    }
    for n in 1..=8 {
        columns.push(format!("reagent{n}"));
        columns.push(format!("reagentCount{n}"));
    }
    let declarations = columns
        .iter()
        .map(|c| format!("{c} NUMERIC NOT NULL DEFAULT 0"))
        .collect::<Vec<_>>()
        .join(",");
    connection.execute_batch(&format!("CREATE TABLE spell_template (entry INTEGER,build INTEGER,name TEXT,script_name TEXT DEFAULT '',{declarations},PRIMARY KEY(entry,build));
        INSERT INTO spell_template (entry,build,name,effect1,effectBasePoints1) VALUES (10,4222,'Old',2,1),(10,5875,'Selected',2,19),(10,6000,'Future',2,99);
        UPDATE spell_template SET school=4,spellLevel=7,castingTimeIndex=1,rangeIndex=1,durationIndex=1,effectDieSides1=1,effectRealPointsPerLevel1=1.25,reagent1=123,reagentCount1=2,powerType=-2,script_name='fixture_script',effect2=6,effectApplyAuraName2=107,effectItemType2=4294967297 WHERE build=5875;")).unwrap();
    crate::world_source::WorldSource::Vmangos(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seals_and_blessings_do_not_displace_judgements_on_another_target() {
        use lyracore_shared::spell_custom::single_target_conflicts;
        let source = fixture();
        let crate::world_source::WorldSource::Vmangos(connection) = &source else {
            unreachable!();
        };
        connection.execute_batch("UPDATE spell_template SET spellFamilyName=10, spellFamilyFlags=1024, baseLevel=1, spellIconId=10").unwrap();
        let judgement = read(&source).unwrap().0[0].single_target_key;
        for earlier_category in [0x0a000200u64, 0x10000100] {
            connection
                .execute_batch(&format!(
                    "UPDATE spell_template SET spellFamilyFlags={}, spellIconId=11",
                    earlier_category | 1024
                ))
                .unwrap();
            let key = read(&source).unwrap().0[0].single_target_key;
            assert!(!single_target_conflicts(judgement, key));
        }
        connection
            .execute_batch("UPDATE spell_template SET spellFamilyFlags=1024, spellIconId=12")
            .unwrap();
        assert!(single_target_conflicts(
            judgement,
            read(&source).unwrap().0[0].single_target_key
        ));
    }

    #[test]
    fn selected_named_spell_rows_keep_signed_values_and_report_unsupported_scripts_and_masks() {
        let (spells, manifest) = read(&fixture()).unwrap();
        assert_eq!(spells.len(), 1);
        let spell = &spells[0];
        assert_eq!(spell.name, "Selected");
        assert_eq!(spell.effect_base_points, [19, 0, 0]);
        assert_eq!(spell.power_type, u32::MAX - 1);
        assert_eq!(spell.reagent[0], 123);
        assert_eq!(spell.effect_item_type[1], 4_294_967_297);
        assert_eq!(
            manifest["findings"][0]["source_ids"],
            serde_json::json!([10])
        );
        assert_eq!(
            manifest["findings"][1]["source_ids"],
            serde_json::json!(["10:1"])
        );
    }

    #[test]
    fn source_behavior_fields_are_checked_and_unknown_bits_are_refused() {
        let source = fixture();
        let crate::world_source::WorldSource::Vmangos(connection) = &source else {
            unreachable!()
        };
        connection.execute("UPDATE spell_template SET customFlags=1024,minTargetLevel=8,effectBonusCoefficient1=0.5,effectBonusCoefficient2=-1,effectBonusCoefficient3=0 WHERE build=5875", []).unwrap();
        let (spells, _) = read(&source).unwrap();
        assert_eq!(spells[0].custom_flags, 1024);
        assert_eq!(spells[0].min_target_level, 8);
        assert_eq!(spells[0].bonus_coefficient, Some([0.5, -1.0, 0.0]));
        connection
            .execute(
                "UPDATE spell_template SET customFlags=16384 WHERE build=5875",
                [],
            )
            .unwrap();
        assert!(read(&source)
            .unwrap_err()
            .to_string()
            .contains("unknown customFlags"));
        connection.execute("UPDATE spell_template SET customFlags=0,effectBonusCoefficient1=1e40 WHERE build=5875", []).unwrap();
        assert!(read(&source)
            .unwrap_err()
            .to_string()
            .contains("non-finite"));
    }

    #[test]
    fn invalid_or_missing_source_fields_fail_with_the_column_name() {
        let source = fixture();
        let crate::world_source::WorldSource::Vmangos(connection) = &source else {
            unreachable!()
        };
        connection
            .execute(
                "UPDATE spell_template SET manaCost='bad' WHERE build=5875",
                [],
            )
            .unwrap();
        assert!(read(&source)
            .unwrap_err()
            .to_string()
            .contains("spell_template.manaCost"));
        connection
            .execute(
                "ALTER TABLE spell_template RENAME COLUMN manaCost TO missing",
                [],
            )
            .unwrap();
        assert!(read(&source)
            .unwrap_err()
            .to_string()
            .contains("spell_template.manaCost"));
    }
}
