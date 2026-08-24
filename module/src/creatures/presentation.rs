//! Reversible EventAI creature projection.

use spacetimedb::{table, ReducerContext, Table};

use super::eventai::RelayFlagOperation;
use super::eventai::{CreaturePresentationInstruction, CreaturePresentationMount, FlagOverride};
use crate::{game_creature_ai_state, game_creature_template, game_world_entity};

const NOT_ATTACKABLE: u32 = 0x0000_0002;
const IMMUNE_TO_PLAYERS: u32 = 0x0000_0100;
const IMMUNE_TO_CREATURES: u32 = 0x0000_0200;
const RAJAXX_CLIENT_PROJECTION: u32 = 0x0000_0040;
const NOT_SELECTABLE: u32 = 0x0200_0000;
const PRESENTATION_UNIT_FLAGS: u32 = NOT_ATTACKABLE
    | IMMUNE_TO_PLAYERS
    | IMMUNE_TO_CREATURES
    | RAJAXX_CLIENT_PROJECTION
    | NOT_SELECTABLE;

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum NpcFlagsProjection {
    Base,
    Clear,
    GossipAndQuest,
}

#[table(accessor = game_creature_presentation)]
pub struct CreaturePresentation {
    #[primary_key]
    pub creature_guid: u64,
    pub lifecycle_id: u64,
    pub definition_revision: u64,
    pub faction_template_override: u32,
    pub has_faction_template_override: bool,
    pub display_template_override: u32,
    pub creature_mount: CreaturePresentationMount,
    pub npc_flags: NpcFlagsProjection,
    pub not_attackable: FlagOverride,
    pub immune_to_players: FlagOverride,
    pub immune_to_creatures: FlagOverride,
    pub not_selectable: FlagOverride,
    pub rajaxx_client_projection: bool,
    pub mana_emptied: bool,
    pub virtual_main_hand_cleared: bool,
}

#[table(accessor = game_creature_relay_temporary_faction)]
pub struct CreatureRelayTemporaryFaction {
    #[primary_key]
    pub creature_guid: u64,
}

/// Apply one named EventAI instruction and refresh the WorldEntity projection.
///
/// State survives evade because that resets Engagement rather than creature lifecycle. A canonical
/// lifecycle reset clears it before the next spawn rule, and definition adoption clears the state
/// owned by the previous revision. Respawn starts from the creature template.
pub(crate) fn apply_eventai_instruction(
    ctx: &ReducerContext,
    creature_guid: u64,
    lifecycle_id: u64,
    definition_revision: u64,
    instruction: CreaturePresentationInstruction,
) -> bool {
    let entities = ctx.db.game_world_entity();
    let Some(entity) = entities.guid().find(creature_guid) else {
        return false;
    };
    if entity.is_player() || entity.owner_guid != 0 || entity.dead {
        return false;
    }
    let templates = ctx.db.game_creature_template();
    let Some(base) = templates.entry().find(entity.entry) else {
        return false;
    };
    let table = ctx.db.game_creature_presentation();
    let mut state = table
        .creature_guid()
        .find(creature_guid)
        .filter(|state| {
            state.lifecycle_id == lifecycle_id && state.definition_revision == definition_revision
        })
        .unwrap_or(CreaturePresentation {
            creature_guid,
            lifecycle_id,
            definition_revision,
            faction_template_override: 0,
            has_faction_template_override: false,
            display_template_override: 0,
            creature_mount: CreaturePresentationMount::Clear,
            npc_flags: NpcFlagsProjection::Base,
            not_attackable: FlagOverride::Base,
            immune_to_players: FlagOverride::Base,
            immune_to_creatures: FlagOverride::Base,
            not_selectable: FlagOverride::Base,
            rajaxx_client_projection: false,
            mana_emptied: false,
            virtual_main_hand_cleared: false,
        });
    match instruction {
        CreaturePresentationInstruction::SetFaction { faction_template } => {
            state.faction_template_override = faction_template;
            state.has_faction_template_override = true;
        }
        CreaturePresentationInstruction::ShowTemplateDisplay { template_entry } => {
            if templates.entry().find(template_entry).is_none() {
                return false;
            }
            state.display_template_override = template_entry;
        }
        CreaturePresentationInstruction::SetCreatureMount { mount } => state.creature_mount = mount,
        CreaturePresentationInstruction::SetNpcFlags { flags } => state.npc_flags = flags,
        CreaturePresentationInstruction::EmptyMana => state.mana_emptied = true,
        CreaturePresentationInstruction::ClearVirtualMainHand => {
            state.virtual_main_hand_cleared = true;
        }
        CreaturePresentationInstruction::SetNotAttackable => {
            state.not_attackable = FlagOverride::Set;
        }
        CreaturePresentationInstruction::ClearNotAttackable => {
            state.not_attackable = FlagOverride::Clear;
        }
        CreaturePresentationInstruction::SetImmuneToPlayers => {
            state.immune_to_players = FlagOverride::Set;
        }
        CreaturePresentationInstruction::ClearImmuneToPlayers => {
            state.immune_to_players = FlagOverride::Clear;
        }
        CreaturePresentationInstruction::SetImmuneToCreatures => {
            state.immune_to_creatures = FlagOverride::Set;
        }
        CreaturePresentationInstruction::ClearImmuneToCreatures => {
            state.immune_to_creatures = FlagOverride::Clear;
        }
        CreaturePresentationInstruction::SetImmuneToPlayersAndCreatures => {
            state.immune_to_players = FlagOverride::Set;
            state.immune_to_creatures = FlagOverride::Set;
        }
        CreaturePresentationInstruction::ClearImmuneToPlayersAndCreatures => {
            state.immune_to_players = FlagOverride::Clear;
            state.immune_to_creatures = FlagOverride::Clear;
        }
        CreaturePresentationInstruction::SetNotSelectable => {
            state.not_selectable = FlagOverride::Set;
        }
        CreaturePresentationInstruction::SetRajaxxSpawnProtection(_) => {
            state.immune_to_players = FlagOverride::Set;
            state.immune_to_creatures = FlagOverride::Set;
            state.rajaxx_client_projection = true;
        }
    }
    let Some(mut entity) = entities.guid().find(creature_guid) else {
        return false;
    };
    project(ctx, &mut entity, &base, &state);
    entities.guid().update(entity);
    if table.creature_guid().find(creature_guid).is_some() {
        table.creature_guid().update(state);
    } else {
        table.insert(state);
    }
    true
}

/// Clear all effects at the lifecycle boundary and restore the static creature projection.
pub(crate) fn clear_eventai_presentation(ctx: &ReducerContext, creature_guid: u64) {
    ctx.db
        .game_creature_relay_temporary_faction()
        .creature_guid()
        .delete(creature_guid);
    clear_if(ctx, creature_guid, |_| true);
}

/// A definition replacement removes only presentation that belongs to the old definition.
pub(crate) fn clear_for_definition_revision(
    ctx: &ReducerContext,
    creature_guid: u64,
    definition_revision: u64,
) {
    clear_if(ctx, creature_guid, |state| {
        state.definition_revision != definition_revision
    });
}

pub(crate) fn apply_relay_faction(
    ctx: &ReducerContext,
    creature_guid: u64,
    faction_template: u32,
    restore_on_combat_stop: bool,
) -> Result<(), String> {
    let lifecycle_id = ctx
        .db
        .game_creature_ai_state()
        .creature_guid()
        .find(creature_guid)
        .map_or(1, |state| state.lifecycle_id);
    let revision = super::eventai::current_definition_revision(ctx, creature_guid).value;
    if !apply_eventai_instruction(
        ctx,
        creature_guid,
        lifecycle_id,
        revision,
        CreaturePresentationInstruction::SetFaction { faction_template },
    ) {
        return Err(format!(
            "relay faction subject {creature_guid} refused faction change"
        ));
    }
    let temporary = ctx.db.game_creature_relay_temporary_faction();
    if restore_on_combat_stop {
        if temporary.creature_guid().find(creature_guid).is_none() {
            temporary.insert(CreatureRelayTemporaryFaction { creature_guid });
        }
    } else {
        temporary.creature_guid().delete(creature_guid);
    }
    Ok(())
}

pub(crate) fn clear_relay_temporary_faction(ctx: &ReducerContext, creature_guid: u64) {
    if !ctx
        .db
        .game_creature_relay_temporary_faction()
        .creature_guid()
        .delete(creature_guid)
    {
        return;
    }
    let table = ctx.db.game_creature_presentation();
    let Some(mut state) = table.creature_guid().find(creature_guid) else {
        return;
    };
    state.has_faction_template_override = false;
    state.faction_template_override = 0;
    let entities = ctx.db.game_world_entity();
    let Some(mut entity) = entities.guid().find(creature_guid) else {
        return;
    };
    let Some(base) = ctx.db.game_creature_template().entry().find(entity.entry) else {
        return;
    };
    project(ctx, &mut entity, &base, &state);
    entities.guid().update(entity);
    table.creature_guid().update(state);
}

pub(crate) fn apply_relay_stand_state(
    ctx: &ReducerContext,
    creature_guid: u64,
    stand_state: u8,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut entity = entities
        .guid()
        .find(creature_guid)
        .filter(|entity| !entity.is_player())
        .ok_or_else(|| format!("relay stand-state subject {creature_guid} is unavailable"))?;
    entity.unit_bytes_1 = (entity.unit_bytes_1 & !0xFF) | u32::from(stand_state);
    entities.guid().update(entity);
    Ok(())
}

pub(crate) fn apply_relay_npc_flags(
    ctx: &ReducerContext,
    creature_guid: u64,
    flags: u32,
    operation: RelayFlagOperation,
) -> Result<(), String> {
    if flags == 0 || flags & !0x3 != 0 {
        return Err(format!("relay NPC flags {flags:#x} are not named"));
    }
    let entities = ctx.db.game_world_entity();
    let mut entity = entities
        .guid()
        .find(creature_guid)
        .filter(|entity| !entity.is_player())
        .ok_or_else(|| format!("relay NPC-flag subject {creature_guid} is unavailable"))?;
    entity.npc_flags = apply_relay_flags(entity.npc_flags, flags, operation);
    entities.guid().update(entity);
    Ok(())
}

pub(crate) fn apply_relay_unit_flags(
    ctx: &ReducerContext,
    creature_guid: u64,
    flags: u32,
    operation: RelayFlagOperation,
) -> Result<(), String> {
    if flags == 0 || flags & !(IMMUNE_TO_PLAYERS | IMMUNE_TO_CREATURES) != 0 {
        return Err(format!("relay unit flags {flags:#x} are not named"));
    }
    let entity = ctx
        .db
        .game_world_entity()
        .guid()
        .find(creature_guid)
        .ok_or_else(|| format!("relay unit-flag subject {creature_guid} is unavailable"))?;
    let lifecycle_id = ctx
        .db
        .game_creature_ai_state()
        .creature_guid()
        .find(creature_guid)
        .map_or(1, |state| state.lifecycle_id);
    let revision = super::eventai::current_definition_revision(ctx, creature_guid).value;
    for (bit, set, clear) in [
        (
            IMMUNE_TO_PLAYERS,
            CreaturePresentationInstruction::SetImmuneToPlayers,
            CreaturePresentationInstruction::ClearImmuneToPlayers,
        ),
        (
            IMMUNE_TO_CREATURES,
            CreaturePresentationInstruction::SetImmuneToCreatures,
            CreaturePresentationInstruction::ClearImmuneToCreatures,
        ),
    ] {
        if flags & bit == 0 {
            continue;
        }
        let enabled = match operation {
            RelayFlagOperation::Add => true,
            RelayFlagOperation::Remove => false,
            RelayFlagOperation::Toggle => entity.unit_flags & bit == 0,
        };
        if !apply_eventai_instruction(
            ctx,
            creature_guid,
            lifecycle_id,
            revision,
            if enabled { set } else { clear },
        ) {
            return Err(format!(
                "relay unit-flag subject {creature_guid} refused flag change"
            ));
        }
    }
    Ok(())
}

fn apply_relay_flags(current: u32, flags: u32, operation: RelayFlagOperation) -> u32 {
    match operation {
        RelayFlagOperation::Add => current | flags,
        RelayFlagOperation::Remove => current & !flags,
        RelayFlagOperation::Toggle => current ^ flags,
    }
}

fn clear_if(
    ctx: &ReducerContext,
    creature_guid: u64,
    predicate: impl FnOnce(&CreaturePresentation) -> bool,
) {
    let table = ctx.db.game_creature_presentation();
    let Some(state) = table.creature_guid().find(creature_guid) else {
        return;
    };
    if !predicate(&state) {
        return;
    }
    table.creature_guid().delete(creature_guid);
    let entities = ctx.db.game_world_entity();
    let Some(mut entity) = entities.guid().find(creature_guid) else {
        return;
    };
    let Some(base) = ctx.db.game_creature_template().entry().find(entity.entry) else {
        return;
    };
    restore_base(&mut entity, &base);
    entities.guid().update(entity);
}

fn project(
    ctx: &ReducerContext,
    entity: &mut crate::WorldEntity,
    base: &crate::CreatureTemplate,
    state: &CreaturePresentation,
) {
    entity.faction_template = if state.has_faction_template_override {
        state.faction_template_override
    } else {
        base.faction_template
    };
    let display = if state.display_template_override == 0 {
        base.display_id
    } else {
        ctx.db
            .game_creature_template()
            .entry()
            .find(state.display_template_override)
            .map(|template| template.display_id)
            .unwrap_or(base.display_id)
    };
    entity.display_id = display;
    entity.native_display_id = display;
    entity.mount_display_id = state.creature_mount.display_id();
    entity.npc_flags = match state.npc_flags {
        NpcFlagsProjection::Base => base.npc_flags,
        NpcFlagsProjection::Clear => 0,
        NpcFlagsProjection::GossipAndQuest => 0x0000_0003,
    };
    if state.mana_emptied {
        entity.power = 0;
    }
    entity.unit_flags = project_unit_flags(entity.unit_flags, base.unit_flags, state);
}

fn restore_base(entity: &mut crate::WorldEntity, base: &crate::CreatureTemplate) {
    entity.faction_template = base.faction_template;
    entity.display_id = base.display_id;
    entity.native_display_id = base.display_id;
    entity.mount_display_id = 0;
    entity.npc_flags = base.npc_flags;
    entity.unit_flags = (entity.unit_flags & !PRESENTATION_UNIT_FLAGS)
        | (base.unit_flags & PRESENTATION_UNIT_FLAGS);
}

fn project_unit_flags(current: u32, base: u32, state: &CreaturePresentation) -> u32 {
    let mut flags = (current & !PRESENTATION_UNIT_FLAGS) | (base & PRESENTATION_UNIT_FLAGS);
    flags = apply_flag_override(flags, NOT_ATTACKABLE, state.not_attackable);
    flags = apply_flag_override(flags, IMMUNE_TO_PLAYERS, state.immune_to_players);
    flags = apply_flag_override(flags, IMMUNE_TO_CREATURES, state.immune_to_creatures);
    flags = apply_flag_override(flags, NOT_SELECTABLE, state.not_selectable);
    if state.rajaxx_client_projection {
        flags |= RAJAXX_CLIENT_PROJECTION;
    }
    flags
}

fn apply_flag_override(flags: u32, bit: u32, override_state: FlagOverride) -> u32 {
    match override_state {
        FlagOverride::Base => flags,
        FlagOverride::Set => flags | bit,
        FlagOverride::Clear => flags & !bit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_overrides_keep_unrelated_dynamic_flags() {
        let state = CreaturePresentation {
            creature_guid: 1,
            lifecycle_id: 1,
            definition_revision: 1,
            faction_template_override: 0,
            has_faction_template_override: false,
            display_template_override: 0,
            creature_mount: CreaturePresentationMount::Clear,
            npc_flags: NpcFlagsProjection::Base,
            not_attackable: FlagOverride::Clear,
            immune_to_players: FlagOverride::Set,
            immune_to_creatures: FlagOverride::Base,
            not_selectable: FlagOverride::Base,
            rajaxx_client_projection: false,
            mana_emptied: false,
            virtual_main_hand_cleared: false,
        };
        let combat_flag = lyracore_shared::constants::unit_flags::IN_COMBAT;
        let projected = project_unit_flags(NOT_ATTACKABLE | combat_flag, NOT_ATTACKABLE, &state);
        assert_eq!(projected & NOT_ATTACKABLE, 0);
        assert_ne!(projected & IMMUNE_TO_PLAYERS, 0);
        assert_ne!(projected & combat_flag, 0);
    }
}
