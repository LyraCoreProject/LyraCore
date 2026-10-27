use std::collections::{BTreeSet, HashSet};

use super::*;

pub(super) fn plan(source: &impl TableRows) -> Result<DumpPlan> {
    let catalogue = spell::source_catalogue(source)?;
    let known = |id: u32| id == 0 || catalogue.ids.contains(&u64::from(id));
    let mut chains = Vec::new();
    let mut explicit = BTreeSet::new();
    for row in source.rows("spell_chain")?.iter() {
        let id: u32 = row.number("spell_id")?;
        let previous: u32 = row.number("prev_spell")?;
        let first: u32 = row.number("first_spell")?;
        let rank: u8 = row.number("rank")?;
        let required: u32 = row.number("req_spell")?;
        anyhow::ensure!(
            id != 0 && [id, previous, first, required].into_iter().all(known),
            "spell_chain {id}: missing spell"
        );
        explicit.insert(id);
        chains.push(format!("({id},{previous},{first},{rank},{required})"));
    }
    let mut links = Vec::new();
    let mut missing_ranks = Vec::new();
    for row in source
        .columns("skill_line_ability", &["spell_id", "superseded_by_spell"])?
        .iter()
    {
        let id: u32 = row.number("spell_id")?;
        let next: u32 = row.number("superseded_by_spell")?;
        if next != 0 {
            if !known(id) || !known(next) {
                missing_ranks.push(format!("{id}:{next}"));
                continue;
            }
            links.push((id, next));
        }
    }
    chains.extend(derived_spell_rank_rows(&links, &explicit)?);

    let mut learns = Vec::new();
    let mut parents = HashSet::new();
    let mut inactive = Vec::new();
    for row in source.rows("spell_learn_spell")?.iter() {
        let parent: u32 = row.number("entry")?;
        let learn: u32 = row.number("SpellID")?;
        let active: u8 = row.number("Active")?;
        anyhow::ensure!(
            known(parent) && known(learn) && parent != 0 && learn != 0,
            "spell_learn_spell {parent}: missing spell {learn}"
        );
        anyhow::ensure!(
            active <= 1,
            "spell_learn_spell {parent}: unknown Active {active}"
        );
        if active == 0 {
            inactive.push(format!("{parent}:{learn}"));
        }
        parents.insert(parent);
        learns.push(format!("({},{parent},{learn})", learns.len() + 1));
    }
    use lyracore_shared::constants::armor_proficiency as armor;
    for (parent, learn) in [
        (armor::PLATE_TRAINER_SPELL_ID, armor::PLATE_PASSIVE_SPELL_ID),
        (armor::MAIL_TRAINER_SPELL_ID, armor::MAIL_PASSIVE_SPELL_ID),
    ] {
        if !parents.contains(&parent) && known(parent) && known(learn) {
            learns.push(format!("({},{parent},{learn})", learns.len() + 1));
        }
    }

    let (procs, unsupported) = procs(source, &catalogue)?;
    let mut statements = Vec::new();
    for (table,key,columns,rows) in [
        ("game_spell_chain","spell_id","spell_id,prev_spell,first_spell,rank,req_spell",chains),
        ("game_spell_learn","id","id,parent_spell,learn_spell",learns),
        ("game_spell_proc_event","spell_id","spell_id,proc_flags,proc_ex,school_mask,family_name,family_flags,ppm_rate,custom_chance,icd_ms",procs),
    ] {
        statements.push(if table == "game_spell_proc_event" {format!("DELETE FROM {table} WHERE spell_id < {SYNTHETIC_PROC_EVENT_FLOOR}")}else{format!("DELETE FROM {table} WHERE {key} > 0")});
        push_insert(&mut statements,table,columns,&rows);
    }
    Ok(DumpPlan {
        stamps: vec![("spellmeta", insert_row_count(&statements))],
        stmts: statements,
        world_source_manifest: Some(serde_json::to_string_pretty(
            &serde_json::json!({"format_version":1,"source":"vmangos","family":"spellmeta","patch":10,"client_build":5875,"findings":[
                {"code":"spellmeta.inactive-learn","classification":"unsupported","occurrences":inactive.len(),"source_ids":inactive},
            {"code":"spellmeta.proc-mask","classification":"unsupported","occurrences":unsupported.len(),"source_ids":unsupported},
            {"code":"spellmeta.missing-rank-spell","classification":"unsupported","occurrences":missing_ranks.len(),"source_ids":missing_ranks}
            ]}),
        )?),
        ..Default::default()
    })
}

fn procs(
    source: &impl TableRows,
    catalogue: &spell::SpellCatalogue,
) -> Result<(Vec<String>, Vec<u32>)> {
    let mut rows = Vec::new();
    let mut unsupported = Vec::new();
    for row in source.rows("spell_proc_event")?.iter() {
        let id: u32 = row.number("entry")?;
        anyhow::ensure!(
            id != 0 && id < SYNTHETIC_PROC_EVENT_FLOOR && catalogue.ids.contains(&u64::from(id)),
            "spell_proc_event {id}: invalid source spell"
        );
        let school: u8 = row.number("SchoolMask")?;
        let family: u8 = row.number("SpellFamilyName")?;
        let masks = [
            row.number::<u64>("SpellFamilyMask0")?,
            row.number("SpellFamilyMask1")?,
            row.number("SpellFamilyMask2")?,
        ];
        let masks: BTreeSet<_> = if masks.iter().all(|mask| *mask == masks[0]) {
            BTreeSet::from([masks[0]])
        } else {
            catalogue
                .proc_effects
                .get(&id)
                .into_iter()
                .flatten()
                .map(|&index| masks[index])
                .collect()
        };
        let flags: u32 = row.number("procFlags")?;
        let extra: u32 = row.number("procEx")?;
        let ppm: f32 = row.number("ppmRate")?;
        let chance: f32 = row.number("CustomChance")?;
        let cooldown: u32 = row.number("Cooldown")?;
        anyhow::ensure!(
            ppm.is_finite() && ppm >= 0.0 && chance.is_finite(),
            "spell_proc_event {id}: invalid chance or PPM"
        );
        let cooldown = cooldown
            .checked_mul(1000)
            .context("spell_proc_event cooldown exceeds milliseconds")?;
        if masks.len() != 1 {
            unsupported.push(id);
            continue;
        }
        let mask = masks.first().unwrap();
        let chance = chance.round().clamp(0.0, 100.0) as u8;
        rows.push(format!(
            "({id},{flags},{extra},{school},{family},{mask},{ppm},{chance},{cooldown})"
        ));
    }
    Ok((rows, unsupported))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> WorldSource {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE spell_template (entry INTEGER,build INTEGER,effect1 INTEGER,effect2 INTEGER,effect3 INTEGER,effectApplyAuraName1 INTEGER,effectApplyAuraName2 INTEGER,effectApplyAuraName3 INTEGER,PRIMARY KEY(entry,build));
            INSERT INTO spell_template VALUES (10,5875,6,0,0,42,0,0),(20,5875,2,0,0,0,0,0),(30,5875,2,0,0,0,0,0),(40,5875,2,0,0,0,0,0);
            CREATE TABLE spell_chain (spell_id INTEGER,prev_spell INTEGER,first_spell INTEGER,rank INTEGER,req_spell INTEGER,build_min INTEGER,build_max INTEGER);
            INSERT INTO spell_chain VALUES (10,0,10,1,0,0,5875),(20,0,20,1,0,0,5000),(20,10,10,2,0,5001,5875);
            CREATE TABLE skill_line_ability (id INTEGER,build INTEGER,spell_id INTEGER,superseded_by_spell INTEGER,PRIMARY KEY(id,build));
            INSERT INTO skill_line_ability VALUES (1,5875,30,40),(1,6000,40,30);
            CREATE TABLE spell_learn_spell (entry INTEGER,SpellID INTEGER,Active INTEGER,build_min INTEGER,build_max INTEGER);
            INSERT INTO spell_learn_spell VALUES (10,20,0,0,5875);
            CREATE TABLE spell_proc_event (entry INTEGER,SchoolMask INTEGER,SpellFamilyName INTEGER,SpellFamilyMask0 INTEGER,SpellFamilyMask1 INTEGER,SpellFamilyMask2 INTEGER,procFlags INTEGER,procEx INTEGER,ppmRate REAL,CustomChance REAL,Cooldown INTEGER,build_min INTEGER,build_max INTEGER);
            INSERT INTO spell_proc_event VALUES (10,4,3,5,99,88,64,2,1.5,12.5,2,0,5875);").unwrap();
        WorldSource::Vmangos(connection)
    }

    #[test]
    fn source_metadata_selects_builds_derives_missing_ranks_and_uses_the_active_proc_mask() {
        let plan = plan(&fixture()).unwrap();
        let sql = plan.stmts.join("\n");
        assert!(sql.contains("(20,10,10,2,0)"));
        assert!(sql.contains("(30,0,30,1,0),(40,30,30,2,0)"));
        assert!(sql.contains("(10,64,2,4,3,5,1.5,13,2000)"));
        assert!(sql.contains("(1,10,20)"));
        let manifest: serde_json::Value =
            serde_json::from_str(plan.world_source_manifest.as_ref().unwrap()).unwrap();
        assert_eq!(
            manifest["findings"][0]["source_ids"],
            serde_json::json!(["10:20"])
        );
    }

    #[test]
    fn distinct_active_proc_masks_are_reported_without_loading_a_broader_mask() {
        let source = fixture();
        let WorldSource::Vmangos(connection) = &source else {
            unreachable!()
        };
        connection
            .execute(
                "UPDATE spell_template SET effect2=6,effectApplyAuraName2=43 WHERE entry=10",
                [],
            )
            .unwrap();
        let plan = plan(&source).unwrap();
        assert!(!plan
            .stmts
            .iter()
            .any(|s| s.starts_with("INSERT INTO game_spell_proc_event")));
        assert!(plan
            .world_source_manifest
            .unwrap()
            .contains("spellmeta.proc-mask"));
    }

    #[test]
    fn rank_links_to_absent_spells_are_reported_without_phantom_ranks() {
        let source = fixture();
        let WorldSource::Vmangos(connection) = &source else {
            unreachable!()
        };
        connection
            .execute("INSERT INTO skill_line_ability VALUES (2,5875,40,999)", [])
            .unwrap();
        let plan = plan(&source).unwrap();
        assert!(!plan.stmts.join("\n").contains("999"));
        let manifest: serde_json::Value =
            serde_json::from_str(plan.world_source_manifest.as_ref().unwrap()).unwrap();
        assert_eq!(
            manifest["findings"][2]["source_ids"],
            serde_json::json!(["40:999"])
        );
    }

    #[test]
    fn invalid_proc_numbers_fail_instead_of_dropping_the_overlay() {
        let source = fixture();
        let WorldSource::Vmangos(connection) = &source else {
            unreachable!()
        };
        connection
            .execute("UPDATE spell_proc_event SET CustomChance='invalid'", [])
            .unwrap();
        let error = plan(&source).err().unwrap().to_string();
        assert!(error.contains("spell_proc_event.CustomChance"));
    }
}
