use std::collections::{BTreeMap, BTreeSet};

use lyracore_shared::constants::{gossip_condition, gossip_option};
use lyracore_shared::gossip::{evaluate_reference, CharacterFacts, QuestState};

use super::*;
use crate::dump::sql_str;
use crate::world_source::Row;

const DEFAULT_GREETING: u64 = 0x00ff_ffff;

type Findings = BTreeMap<&'static str, BTreeSet<String>>;
struct OptionRow {
    key: String,
    npc_flags: u32,
    fields: String,
}

type Broadcast = BTreeMap<u64, (String, String)>;

fn finding(findings: &mut Findings, code: &'static str, key: impl ToString) {
    findings.entry(code).or_default().insert(key.to_string());
}

fn broadcast(dump: &impl TableRows, findings: &mut Findings) -> Result<Broadcast> {
    let mut output = BTreeMap::new();
    for row in dump.rows("broadcast_text")?.iter() {
        let id = row.number::<u64>("entry")?;
        output.insert(
            id,
            (
                row.optional_text("male_text")?
                    .unwrap_or_default()
                    .to_string(),
                row.optional_text("female_text")?
                    .unwrap_or_default()
                    .to_string(),
            ),
        );
        for column in [
            "chat_type",
            "sound_id",
            "language_id",
            "emote_id1",
            "emote_id2",
            "emote_id3",
            "emote_delay1",
            "emote_delay2",
            "emote_delay3",
        ] {
            if row.number::<u32>(column)? != 0 {
                finding(findings, "gossip.broadcast-metadata", id);
            }
        }
    }
    Ok(output)
}

fn slots(
    dump: &impl TableRows,
    broadcast: &Broadcast,
    findings: &mut Findings,
) -> Result<BTreeMap<u64, [NpcTextSlot; 8]>> {
    let mut output = BTreeMap::new();
    for row in dump.rows("npc_text")?.iter() {
        let id = row.number::<u64>("ID")?;
        let mut slots = core::array::from_fn(|_| (String::new(), String::new(), 0.0));
        for (index, slot) in slots.iter_mut().enumerate() {
            let reference = row.number::<u64>(&format!("BroadcastTextID{index}"))?;
            let probability = row.number::<f32>(&format!("Probability{index}"))?;
            anyhow::ensure!(
                probability.is_finite() && probability >= 0.0,
                "npc_text {id}: invalid probability"
            );
            if let Some((male, female)) = broadcast.get(&reference) {
                *slot = (male.clone(), female.clone(), probability);
            } else if reference != 0 {
                finding(
                    findings,
                    "gossip.missing-broadcast",
                    format!("{id}:{index}"),
                );
            }
        }
        output.insert(id, slots);
    }
    Ok(output)
}

fn option_text(row: &Row, broadcast: &Broadcast, findings: &mut Findings) -> Result<String> {
    let reference = row.number::<u64>("option_broadcast_text")?;
    if reference != 0 {
        if let Some((male, female)) = broadcast.get(&reference) {
            if !male.is_empty() && !female.is_empty() && male != female {
                finding(findings, "gossip.option-gender", reference);
            }
            return Ok(if male.is_empty() { female } else { male }.clone());
        }
        finding(findings, "gossip.missing-option-broadcast", reference);
    }
    Ok(row
        .optional_text("option_text")?
        .unwrap_or_default()
        .to_string())
}

fn options(
    dump: &impl TableRows,
    broadcast: &Broadcast,
    conditions: &gossip_conditions::Conditions,
    findings: &mut Findings,
) -> Result<BTreeMap<u64, Vec<OptionRow>>> {
    let mut menus: BTreeMap<u64, Vec<OptionRow>> = BTreeMap::new();
    let rows = dump.rows("gossip_menu_option")?;
    let mut ordered = rows
        .iter()
        .map(|row| {
            Ok((
                (row.number::<u64>("menu_id")?, row.number::<u32>("id")?),
                row,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    ordered.sort_by_key(|(key, _)| *key);
    for ((menu, id), row) in ordered {
        menus.entry(menu).or_default();
        let action = row.number::<u32>("option_id")?;
        if action == gossip_option::QUESTGIVER {
            continue;
        }
        let key = format!("{menu}:{id}");
        let text = option_text(row, broadcast, findings)?;
        let icon = row.number::<u32>("option_icon")?;
        let target = row.number::<i64>("action_menu_id")?;
        let (mut kind, mut a, mut b) = gossip_conditions::resolve(row, conditions)?;
        for (column, code) in [
            ("action_script_id", "gossip.option-script"),
            ("action_poi_id", "gossip.option-poi"),
            ("box_coded", "gossip.option-input"),
            ("box_money", "gossip.option-payment"),
        ] {
            if row.number::<u64>(column)? != 0 {
                finding(findings, code, &key);
                if matches!(column, "box_money" | "box_coded") {
                    (kind, a, b) = (gossip_condition::NEVER, 0, 0);
                }
            }
        }
        if row.number::<u64>("box_broadcast_text")? != 0
            || !row
                .optional_text("box_text")?
                .unwrap_or_default()
                .is_empty()
        {
            finding(findings, "gossip.option-confirmation", &key);
        }
        if !matches!(action, 1 | 3 | 4 | 5 | 6 | 8 | 9 | 10 | 11 | 16) {
            finding(findings, "gossip.option-action", &key);
            (kind, a, b) = (gossip_condition::NEVER, 0, 0);
        }
        let condition = row.number::<u32>("condition_id")?;
        if condition != 0
            && evaluate_reference(
                condition,
                CharacterFacts {
                    race: 1,
                    class: 1,
                    level: 1,
                },
                |_| QuestState::Absent,
                |id| conditions.get(&id).copied(),
            )
            .is_err()
        {
            finding(findings, "gossip.option-condition", &key);
        }
        let target = if target == -1 {
            0
        } else {
            u32::try_from(target).context("gossip action menu outside u32 or close sentinel -1")?
        };
        let npc_flags = row.number::<u32>("npc_option_npcflag")?;
        menus.entry(menu).or_default().push(OptionRow {
            key,
            npc_flags,
            fields: format!("{icon},{},{action},{target},{kind},{a},{b}", sql_str(&text)),
        });
    }
    Ok(menus)
}

pub(super) fn plan(dump: &impl TableRows, args: &WorldImportArgs) -> Result<DumpPlan> {
    let entries = vmangos_creature_entries(dump, args)?;
    let mut findings = Findings::new();
    for row in dump.rows("npc_gossip")?.iter() {
        let guid = row.number::<u32>("npc_guid")?;
        row.number::<u32>("textid")?;
        finding(&mut findings, "gossip.spawn-greeting", guid);
    }
    let broadcast = broadcast(dump, &mut findings)?;
    let texts = slots(dump, &broadcast, &mut findings)?;
    let conditions = gossip_conditions::read(dump)?;
    for (&id, condition) in &conditions {
        if matches!(
            condition.kind,
            lyracore_shared::gossip::ConditionKind::Unsupported(_)
        ) {
            finding(&mut findings, "gossip.condition-kind", id);
        }
        if condition.flags & !1 != 0 {
            finding(&mut findings, "gossip.condition-flags", id);
        }
    }
    let options = options(dump, &broadcast, &conditions, &mut findings)?;
    let mut menus = BTreeMap::new();
    let rows = dump.rows("gossip_menu")?;
    let mut greetings = rows
        .iter()
        .map(|row| {
            Ok((
                (row.number::<u64>("entry")?, row.number::<u64>("text_id")?),
                row,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    greetings.sort_by_key(|(key, _)| *key);
    for ((menu, text), row) in greetings {
        menus.entry(menu).or_insert(DEFAULT_GREETING);
        if row.number::<u32>("condition_id")? != 0 {
            finding(
                &mut findings,
                "gossip.conditional-greeting",
                format!("{menu}:{text}"),
            );
        } else {
            menus.insert(menu, text);
        }
        if row.number::<u32>("script_id")? != 0 {
            finding(
                &mut findings,
                "gossip.menu-script",
                format!("{menu}:{text}"),
            );
        }
    }
    let mut gossip = GossipEtl {
        menu_rows: Vec::new(),
        npc_text_rows: Vec::new(),
        npc_text_slot_rows: Vec::new(),
        option_rows: Vec::new(),
        profile_rows: Vec::new(),
        profile_option_rows: Vec::new(),
        condition_rows: gossip_conditions::sql_rows(&conditions),
    };
    for creature in dump
        .columns(
            "creature_template",
            &["entry", "gossip_menu_id", "npc_flags"],
        )?
        .iter()
    {
        let entry = creature.number::<u64>("entry")?;
        if !entries.contains(&entry) {
            continue;
        }
        let menu = creature.number::<u64>("gossip_menu_id")?;
        if let Some(&text) = menus.get(&menu).filter(|text| **text != 0) {
            gossip.menu_rows.push(format!("({entry},{text})"));
        } else if menu != 0 && !menus.contains_key(&menu) {
            finding(&mut findings, "gossip.missing-menu", entry);
        }
        if let Some(options) = options.get(&menu).or_else(|| options.get(&0)) {
            let npc_flags = creature.number::<u32>("npc_flags")?;
            for (index, option) in options
                .iter()
                .filter(|option| option.npc_flags & npc_flags != 0)
                .enumerate()
            {
                gossip.option_rows.push(format!(
                    "({},{entry},{index},{})",
                    gossip.option_rows.len() + 1,
                    option.fields
                ));
            }
        }
    }
    for (&menu, &text) in &menus {
        gossip.profile_rows.push(format!("({menu},{text})"));
        if let Some(options) = options.get(&menu) {
            for (index, option) in options.iter().enumerate() {
                if option.npc_flags != 0 {
                    finding(&mut findings, "gossip.profile-npc-flags", &option.key);
                }
                gossip.profile_option_rows.push(format!(
                    "({},{menu},{index},{})",
                    gossip.profile_option_rows.len() + 1,
                    option.fields
                ));
            }
        }
    }
    let needed = menus
        .values()
        .copied()
        .filter(|id| *id != 0 && *id != DEFAULT_GREETING)
        .collect::<BTreeSet<_>>();
    for id in needed {
        let Some(slots) = texts.get(&id) else {
            finding(&mut findings, "gossip.missing-npc-text", id);
            continue;
        };
        gossip
            .npc_text_rows
            .push(format!("({id},{})", sql_str(&slots_first_nonempty(slots))));
        for (index, (male, female, probability)) in slots.iter().enumerate() {
            if male.is_empty() && female.is_empty() && *probability == 0.0 {
                continue;
            }
            gossip.npc_text_slot_rows.push(format!(
                "({},{id},{index},{},{},{probability})",
                gossip.npc_text_slot_rows.len() + 1,
                sql_str(male),
                sql_str(female)
            ));
        }
    }
    let mut stmts = Vec::new();
    gossip_statements(&mut stmts, &gossip);
    let count = insert_row_count(&stmts);
    let findings = findings.into_iter().map(|(code, keys)| serde_json::json!({"code":code,"classification":"unsupported","occurrences":keys.len()})).collect::<Vec<_>>();
    Ok(DumpPlan {
        stmts,
        stamps: vec![("gossip", count)],
        world_source_manifest: Some(serde_json::to_string_pretty(
            &serde_json::json!({"format_version":1,"source":"vmangos","family":"gossip","patch":crate::source_selection::PATCH,"client_build":crate::source_selection::CLIENT_BUILD,"findings":findings}),
        )?),
        ..Default::default()
    })
}

#[cfg(test)]
#[path = "world_import_gossip_tests.rs"]
mod tests;
