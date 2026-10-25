use lyracore_protocol::codec::addon;
use spacetimedb::{ReducerContext, SpacetimeType};

use super::{game_native_player, game_native_world_connection, NativeWorldConnection};
use crate::{bridge::AddonMessage, game_world_entity};

#[derive(SpacetimeType)]
pub struct AddonLimit {
    credit_micros: u64,
    updated_micros: i64,
}

impl AddonLimit {
    fn new(now: i64) -> Self {
        Self {
            credit_micros: 10_000_000,
            updated_micros: now,
        }
    }

    fn admit(&mut self, now: i64) -> bool {
        let elapsed = now.saturating_sub(self.updated_micros).max(0) as u64;
        self.credit_micros = self.credit_micros.saturating_add(elapsed).min(10_000_000);
        self.updated_micros = self.updated_micros.max(now);
        if self.credit_micros < 500_000 {
            return false;
        }
        self.credit_micros -= 500_000;
        true
    }
}

pub(super) fn receive(
    ctx: &ReducerContext,
    mut connection: NativeWorldConnection,
    guid: u64,
    text: &str,
) -> Result<(), String> {
    super::actor(ctx, guid).map_err(super::rollback_silent)?;
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let admitted = connection
        .addon_limit
        .get_or_insert_with(|| AddonLimit::new(now))
        .admit(now);
    ctx.db
        .game_native_world_connection()
        .connection_id()
        .update(connection);
    if admitted {
        if let Some((command, payload)) = addon::parse_bridge_envelope(text) {
            crate::bridge::apply_client_command(ctx, guid, &command, &payload)
                .map_err(super::rollback_silent)?;
        }
    }
    Ok(())
}

pub(crate) fn appeared(ctx: &ReducerContext, row: &AddonMessage) {
    let text = addon::build_bridge_envelope(&row.cmd, &row.payload);
    if text.contains('\0') || text.len() > usize::from(u16::MAX) - 21 {
        spacetimedb::log::warn!("Addon reply {} does not fit a world frame", row.id);
        return;
    }
    for entity in ctx
        .db
        .game_world_entity()
        .by_owner()
        .filter(row.recipient_identity)
        .filter(|entity| entity.is_player())
    {
        if let Some(player) = ctx
            .db
            .game_native_player()
            .character_guid()
            .find(entity.guid)
        {
            let (opcode, body) = addon::build_addon_smsg_raw(entity.guid, &text);
            super::send_raw(ctx, player.connection_id, opcode, body, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_allows_twenty_commands_then_two_per_second() {
        let mut limit = AddonLimit::new(1_000_000);
        for _ in 0..20 {
            assert!(limit.admit(1_000_000));
        }
        assert!(!limit.admit(1_000_000));
        assert!(!limit.admit(1_499_999));
        assert!(limit.admit(1_500_000));
        assert!(!limit.admit(1_500_000));
        assert!(!limit.admit(0));
        assert!(!limit.admit(1_500_000));
        assert!(limit.admit(2_000_000));
        for _ in 0..20 {
            assert!(limit.admit(100_000_000));
        }
        assert!(!limit.admit(100_000_000));
    }
}
