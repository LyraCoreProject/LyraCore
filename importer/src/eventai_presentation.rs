//! Pinned CMaNGOS presentation-action mapping.

use std::collections::HashSet;

use super::eventai::{Dependency, MappingFailure, SourceNormalization};

pub(super) const ACTION_SET_FACTION: u32 = 2;
pub(super) const ACTION_MORPH_TO_ENTRY_OR_MODEL: u32 = 3;
pub(super) const ACTION_SET_UNIT_FIELD: u32 = 17;
pub(super) const ACTION_SET_UNIT_FLAG: u32 = 18;
pub(super) const ACTION_REMOVE_UNIT_FLAG: u32 = 19;
pub(super) const ACTION_UPDATE_TEMPLATE: u32 = 36;
pub(super) const ACTION_MOUNT_TO_ENTRY_OR_MODEL: u32 = 43;

const RAJAXX_RULE_ID: u64 = 1_534_108;
const RAJAXX_SOURCE_GUID: i32 = -155_940;

const DISPLAY_TEMPLATE_ENTRIES: &[u32] = &[
    5_357, 5_358, 5_359, 5_360, 5_361, 9_621, 10_296, 11_284, 13_279, 13_738, 13_739, 13_740,
    13_741, 13_742, 14_603, 14_604, 14_638, 14_639, 14_640,
];

pub(super) struct PresentationAction {
    pub(super) encoded: String,
    pub(super) raw_target: Option<u32>,
    pub(super) dependencies: Vec<Dependency>,
    pub(super) normalizations: Vec<SourceNormalization>,
}

pub(super) fn map_action(
    action: [u32; 4],
    rule_id: u64,
    subject: i32,
    slot: usize,
    importable_templates: &HashSet<u64>,
) -> Option<Result<PresentationAction, Vec<MappingFailure>>> {
    let mapped = match action[0] {
        ACTION_SET_FACTION => map_faction(action),
        ACTION_MORPH_TO_ENTRY_OR_MODEL | ACTION_UPDATE_TEMPLATE => {
            map_template_display(action, rule_id, slot, importable_templates)
        }
        ACTION_SET_UNIT_FIELD => map_unit_field(action),
        ACTION_SET_UNIT_FLAG => map_set_unit_flags(action, rule_id, subject),
        ACTION_REMOVE_UNIT_FLAG => map_remove_unit_flags(action),
        ACTION_MOUNT_TO_ENTRY_OR_MODEL => map_creature_mount(action),
        _ => return None,
    };
    Some(mapped)
}

pub(super) fn source_target_parameter(action: u32) -> Option<usize> {
    match action {
        ACTION_SET_UNIT_FIELD => Some(3),
        ACTION_SET_UNIT_FLAG | ACTION_REMOVE_UNIT_FLAG => Some(2),
        _ => None,
    }
}

fn map_faction(action: [u32; 4]) -> Result<PresentationAction, Vec<MappingFailure>> {
    let faction_template = action[1];
    if action[2] != 0
        || action[3] != 0
        || !matches!(faction_template, 14 | 17 | 35 | 54 | 84 | 104 | 777)
    {
        return Err(vec![unsupported(action)]);
    }
    Ok(PresentationAction {
        encoded: format!("faction:{faction_template}"),
        raw_target: None,
        dependencies: Vec::new(),
        normalizations: Vec::new(),
    })
}

fn map_template_display(
    action: [u32; 4],
    rule_id: u64,
    slot: usize,
    importable_templates: &HashSet<u64>,
) -> Result<PresentationAction, Vec<MappingFailure>> {
    let template_entry = action[1];
    if action[2] != 0 || action[3] != 0 || !DISPLAY_TEMPLATE_ENTRIES.contains(&template_entry) {
        return Err(vec![unsupported(action)]);
    }
    if !importable_templates.contains(&u64::from(template_entry)) {
        return Err(vec![MappingFailure::dependency(
            "presentation_template",
            u64::from(template_entry),
            "missing",
            format!("rule:{rule_id} -> action:{slot} -> creature_template:{template_entry}"),
        )]);
    }
    Ok(PresentationAction {
        encoded: format!("display-template:{template_entry}"),
        raw_target: None,
        dependencies: vec![Dependency {
            kind: "presentation_template",
            path: format!("rule:{rule_id} -> action:{slot} -> creature_template:{template_entry}"),
        }],
        normalizations: Vec::new(),
    })
}

fn map_unit_field(action: [u32; 4]) -> Result<PresentationAction, Vec<MappingFailure>> {
    let encoded = match action {
        [ACTION_SET_UNIT_FIELD, 147, 0, 0] => "npc-flags:clear",
        [ACTION_SET_UNIT_FIELD, 147, 3, 0] => "npc-flags:gossip-and-quest",
        [ACTION_SET_UNIT_FIELD, 37, 0, 0] => "virtual-main-hand:clear",
        [ACTION_SET_UNIT_FIELD, 23, 0, 0] => "mana:empty",
        _ => return Err(vec![unsupported(action)]),
    };
    Ok(PresentationAction {
        encoded: encoded.to_string(),
        raw_target: Some(action[3]),
        dependencies: Vec::new(),
        normalizations: Vec::new(),
    })
}

fn map_set_unit_flags(
    action: [u32; 4],
    rule_id: u64,
    subject: i32,
) -> Result<PresentationAction, Vec<MappingFailure>> {
    let (encoded, normalizations) = match action {
        [ACTION_SET_UNIT_FLAG, 0x0000_0100, 0, 0] => ("unit-flags:set:immune-to-players", vec![]),
        [ACTION_SET_UNIT_FLAG, 0x0000_0200, 0, 0] => ("unit-flags:set:immune-to-creatures", vec![]),
        [ACTION_SET_UNIT_FLAG, 0x0000_0300, 0, 0] => {
            ("unit-flags:set:immune-to-players-and-creatures", vec![])
        }
        [ACTION_SET_UNIT_FLAG, 0x0200_0000, 0, 0] => ("unit-flags:set:not-selectable", vec![]),
        [ACTION_SET_UNIT_FLAG, 0x0000_0340, 0, 0]
            if rule_id == RAJAXX_RULE_ID && subject == RAJAXX_SOURCE_GUID =>
        {
            (
                "unit-flags:set:rajaxx-spawn-protection",
                vec![SourceNormalization {
                    dimension: "unit_flag",
                    raw_value: 0x0000_0340,
                    reason: "quarantined_rajaxx_client_projection",
                }],
            )
        }
        _ => return Err(vec![unsupported(action)]),
    };
    Ok(PresentationAction {
        encoded: encoded.to_string(),
        raw_target: Some(action[2]),
        dependencies: Vec::new(),
        normalizations,
    })
}

fn map_remove_unit_flags(action: [u32; 4]) -> Result<PresentationAction, Vec<MappingFailure>> {
    let encoded = match action {
        [ACTION_REMOVE_UNIT_FLAG, 0x0000_0002, 0, 0] => "unit-flags:clear:not-attackable",
        [ACTION_REMOVE_UNIT_FLAG, 0x0000_0100, 0, 0] => "unit-flags:clear:immune-to-players",
        [ACTION_REMOVE_UNIT_FLAG, 0x0000_0200, 0, 0] => "unit-flags:clear:immune-to-creatures",
        [ACTION_REMOVE_UNIT_FLAG, 0x0000_0300, 0, 0] => {
            "unit-flags:clear:immune-to-players-and-creatures"
        }
        _ => return Err(vec![unsupported(action)]),
    };
    Ok(PresentationAction {
        encoded: encoded.to_string(),
        raw_target: Some(action[2]),
        dependencies: Vec::new(),
        normalizations: Vec::new(),
    })
}

fn map_creature_mount(action: [u32; 4]) -> Result<PresentationAction, Vec<MappingFailure>> {
    let encoded = match action {
        [ACTION_MOUNT_TO_ENTRY_OR_MODEL, 0, 0, 0] => "creature-mount:clear",
        [ACTION_MOUNT_TO_ENTRY_OR_MODEL, 0, 207, 0] => "creature-mount:raider",
        [ACTION_MOUNT_TO_ENTRY_OR_MODEL, 0, 2_328, 0] => "creature-mount:kerr",
        [ACTION_MOUNT_TO_ENTRY_OR_MODEL, 0, 9_991, 0] => "creature-mount:huntress",
        [ACTION_MOUNT_TO_ENTRY_OR_MODEL, 0, 14_337, 0] => "creature-mount:twilight-marauder",
        _ => return Err(vec![unsupported(action)]),
    };
    Ok(PresentationAction {
        encoded: encoded.to_string(),
        raw_target: None,
        dependencies: Vec::new(),
        normalizations: Vec::new(),
    })
}

fn unsupported(action: [u32; 4]) -> MappingFailure {
    MappingFailure::source(
        "presentation_action",
        u64::from(action[0]),
        format!(
            "unsupported_presentation_action_parameters_{}_{}_{}",
            action[1], action[2], action[3]
        ),
    )
}
