use spacetimedb::{reducer, table, Identity, ReducerContext, Table};

#[table(accessor = game_character_shard)]
pub struct CharacterShard {
    #[primary_key]
    pub character_guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub updated_micros: i64,
    // World Shard revisions are independent and cannot certify a remote party partition.
    #[default(1u64)]
    pub revision: u64,
    #[default(Identity::ZERO)]
    pub bot_source_identity: Identity,
    #[default(0u64)]
    pub bot_transfer_intent_id: u64,
    #[default(0u64)]
    pub bot_controller_generation: u64,
    #[default(false)]
    pub transfer_pending: bool,
    #[default(0u32)]
    pub pending_destination_map: u32,
    #[default(0u64)]
    pub pending_destination_instance: u64,
}

crate::character_owned!(delete, fn sweep_delete_game_character_shard(ctx, character_guid) {
    ctx.db.game_character_shard().character_guid().delete(character_guid);
});

// Routing hints do not travel; the source snapshot still names the partition being left.
crate::character_owned!(not_transported, fn sweep_transfer_game_character_shard());

pub(crate) fn record_shard(
    ctx: &ReducerContext,
    character_guid: u64,
    map_id: u32,
    instance_id: u64,
) {
    let idx = ctx.db.game_character_shard();
    let current = idx.character_guid().find(character_guid);
    let revision = next_shard_revision(current.as_ref(), map_id, instance_id);
    let same_partition = current
        .as_ref()
        .is_some_and(|row| (row.map_id, row.instance_id) == (map_id, instance_id));
    let row = CharacterShard {
        character_guid,
        map_id,
        instance_id,
        updated_micros: crate::time::now(ctx).to_micros_since_unix_epoch(),
        revision,
        bot_source_identity: current.as_ref().map_or(Identity::ZERO, |row| {
            if same_partition {
                row.bot_source_identity
            } else {
                Identity::ZERO
            }
        }),
        bot_transfer_intent_id: current
            .as_ref()
            .filter(|_| same_partition)
            .map_or(0, |row| row.bot_transfer_intent_id),
        bot_controller_generation: current
            .as_ref()
            .filter(|_| same_partition)
            .map_or(0, |row| row.bot_controller_generation),
        transfer_pending: false,
        pending_destination_map: 0,
        pending_destination_instance: 0,
    };
    if current.is_some() {
        idx.character_guid().update(row);
    } else {
        idx.insert(row);
    }
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn begin_character_shard_transfer(
    ctx: &ReducerContext,
    character_guid: u64,
    source_map_id: u32,
    source_instance_id: u64,
    source_revision: u64,
    destination_map_id: u32,
    destination_instance_id: u64,
    source_module_identity: Identity,
    transfer_intent_id: u64,
    controller_generation: u64,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::account_ownership::require_actor_for(ctx, request_actor, character_guid)?;
    if (source_module_identity == Identity::ZERO) != (transfer_intent_id == 0)
        || (source_revision == 0
            && (source_module_identity != Identity::ZERO
                || transfer_intent_id != 0
                || controller_generation != 0))
    {
        return Err("Transfer locator identity is incomplete".to_string());
    }
    let table = ctx.db.game_character_shard();
    let Some(current) = table.character_guid().find(character_guid) else {
        if source_revision != 0 {
            return Err("Transfer source has no Realm locator".to_string());
        }
        table.insert(CharacterShard {
            character_guid,
            map_id: source_map_id,
            instance_id: source_instance_id,
            updated_micros: crate::time::now(ctx).to_micros_since_unix_epoch(),
            revision: 1,
            bot_source_identity: source_module_identity,
            bot_transfer_intent_id: transfer_intent_id,
            bot_controller_generation: controller_generation,
            transfer_pending: true,
            pending_destination_map: destination_map_id,
            pending_destination_instance: destination_instance_id,
        });
        return Ok(());
    };
    let crossing = (
        source_module_identity,
        transfer_intent_id,
        controller_generation,
    );
    if current.transfer_pending
        && (current.map_id, current.instance_id, current.revision)
            == (source_map_id, source_instance_id, source_revision.max(1))
        && (
            current.pending_destination_map,
            current.pending_destination_instance,
        ) == (destination_map_id, destination_instance_id)
        && (
            current.bot_source_identity,
            current.bot_transfer_intent_id,
            current.bot_controller_generation,
        ) == crossing
    {
        return Ok(());
    }
    if source_revision == 0
        || current.transfer_pending
        || (current.map_id, current.instance_id, current.revision)
            != (source_map_id, source_instance_id, source_revision)
    {
        return Err("Transfer Realm locator changed".to_string());
    }
    table.character_guid().update(CharacterShard {
        updated_micros: crate::time::now(ctx).to_micros_since_unix_epoch(),
        bot_source_identity: source_module_identity,
        bot_transfer_intent_id: transfer_intent_id,
        bot_controller_generation: controller_generation,
        transfer_pending: true,
        pending_destination_map: destination_map_id,
        pending_destination_instance: destination_instance_id,
        ..current
    });
    Ok(())
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn finish_character_shard_transfer(
    ctx: &ReducerContext,
    character_guid: u64,
    source_map_id: u32,
    source_instance_id: u64,
    source_revision: u64,
    destination_map_id: u32,
    destination_instance_id: u64,
    source_module_identity: Identity,
    transfer_intent_id: u64,
    controller_generation: u64,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::account_ownership::require_actor_for(ctx, request_actor, character_guid)?;
    if source_revision == 0
        || (source_module_identity == Identity::ZERO) != (transfer_intent_id == 0)
    {
        return Err("Transfer locator identity is incomplete".to_string());
    }
    let table = ctx.db.game_character_shard();
    let current = table
        .character_guid()
        .find(character_guid)
        .ok_or_else(|| "Transfer source has no Realm locator".to_string())?;
    let crossing = (
        source_module_identity,
        transfer_intent_id,
        controller_generation,
    );
    if !current.transfer_pending
        && (current.map_id, current.instance_id, current.revision)
            == (
                destination_map_id,
                destination_instance_id,
                source_revision.saturating_add(1),
            )
        && (
            current.bot_source_identity,
            current.bot_transfer_intent_id,
            current.bot_controller_generation,
        ) == crossing
    {
        return Ok(());
    }
    if !current.transfer_pending
        || (current.map_id, current.instance_id, current.revision)
            != (source_map_id, source_instance_id, source_revision)
        || (
            current.pending_destination_map,
            current.pending_destination_instance,
        ) != (destination_map_id, destination_instance_id)
        || (
            current.bot_source_identity,
            current.bot_transfer_intent_id,
            current.bot_controller_generation,
        ) != crossing
    {
        return Err("Transfer Realm locator phase changed".to_string());
    }
    table.character_guid().update(CharacterShard {
        character_guid,
        map_id: destination_map_id,
        instance_id: destination_instance_id,
        updated_micros: crate::time::now(ctx).to_micros_since_unix_epoch(),
        revision: source_revision
            .checked_add(1)
            .ok_or_else(|| "Transfer Realm locator revision exhausted".to_string())?,
        bot_source_identity: source_module_identity,
        bot_transfer_intent_id: transfer_intent_id,
        bot_controller_generation: controller_generation,
        transfer_pending: false,
        pending_destination_map: 0,
        pending_destination_instance: 0,
    });
    Ok(())
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn finish_pending_character_shard_transfer(
    ctx: &ReducerContext,
    character_guid: u64,
    source_map_id: u32,
    source_instance_id: u64,
    source_revision: u64,
    destination_map_id: u32,
    destination_instance_id: u64,
    source_module_identity: Identity,
    transfer_intent_id: u64,
    controller_generation: u64,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::account_ownership::require_actor_for(ctx, request_actor, character_guid)?;
    if source_revision == 0
        || (source_module_identity == Identity::ZERO) != (transfer_intent_id == 0)
    {
        return Err("Transfer locator identity is incomplete".to_string());
    }
    let table = ctx.db.game_character_shard();
    let current = table
        .character_guid()
        .find(character_guid)
        .ok_or_else(|| "Transfer destination has no Realm locator".to_string())?;
    if !current.transfer_pending
        || (current.map_id, current.instance_id, current.revision)
            != (source_map_id, source_instance_id, source_revision)
        || (
            current.pending_destination_map,
            current.pending_destination_instance,
        ) != (destination_map_id, destination_instance_id)
        || (
            current.bot_source_identity,
            current.bot_transfer_intent_id,
            current.bot_controller_generation,
        ) != (
            source_module_identity,
            transfer_intent_id,
            controller_generation,
        )
    {
        return Err("pending Realm Transfer phase changed".to_string());
    }
    table.character_guid().update(CharacterShard {
        character_guid,
        map_id: destination_map_id,
        instance_id: destination_instance_id,
        updated_micros: crate::time::now(ctx).to_micros_since_unix_epoch(),
        revision: source_revision
            .checked_add(1)
            .ok_or_else(|| "Transfer Realm locator revision exhausted".to_string())?,
        bot_source_identity: current.bot_source_identity,
        bot_transfer_intent_id: current.bot_transfer_intent_id,
        bot_controller_generation: current.bot_controller_generation,
        transfer_pending: false,
        pending_destination_map: 0,
        pending_destination_instance: 0,
    });
    Ok(())
}

fn next_shard_revision(current: Option<&CharacterShard>, map_id: u32, instance_id: u64) -> u64 {
    match current {
        Some(row) if (row.map_id, row.instance_id) == (map_id, instance_id) => row.revision.max(1),
        Some(row) => row.revision.max(1).saturating_add(1),
        None => 1,
    }
}

#[reducer]
pub fn set_character_shard(
    ctx: &ReducerContext,
    character_guid: u64,
    map_id: u32,
    instance_id: u64,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::account_ownership::require_actor_for(ctx, request_actor, character_guid)?;
    if ctx
        .db
        .game_character_shard()
        .character_guid()
        .find(character_guid)
        .is_some_and(|row| row.transfer_pending)
    {
        return Err("Character has a pending Realm Transfer phase".to_string());
    }
    record_shard(ctx, character_guid, map_id, instance_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realm_locator_revision_advances_only_when_the_partition_changes() {
        let current = CharacterShard {
            character_guid: 100,
            map_id: 0,
            instance_id: 0,
            updated_micros: 20,
            revision: 7,
            bot_source_identity: Identity::ZERO,
            bot_transfer_intent_id: 0,
            bot_controller_generation: 0,
            transfer_pending: false,
            pending_destination_map: 0,
            pending_destination_instance: 0,
        };
        assert_eq!(next_shard_revision(None, 0, 0), 1);
        assert_eq!(next_shard_revision(Some(&current), 0, 0), 7);
        assert_eq!(next_shard_revision(Some(&current), 36, 1), 8);
    }
}
