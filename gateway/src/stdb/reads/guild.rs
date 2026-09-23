//! Guild cache reads. The guild tables are read on the Realm-core handle; Character facts are read
//! across every World Shard. A Fee Hold is read on the payer's Home Shard.

use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;
use crate::world::{guild_fee, CharacterFacts};

fn member_view(row: GuildMember) -> crate::codec::GuildMemberView {
    crate::codec::GuildMemberView {
        character_guid: row.character_guid,
        guild_id: row.guild_id,
        rank_id: row.rank_id,
        name: row.name,
        public_note: row.public_note,
        officer_note: row.officer_note,
        realm_account_id: row.realm_account_id,
    }
}

impl Coordinator {
    /// The membership row of `character_guid` in THIS handle's cache. Call it on the Realm-core
    /// handle.
    pub(crate) fn guild_member_row(
        &self,
        character_guid: u64,
    ) -> Option<crate::codec::GuildMemberView> {
        self.0
            .coord()
            .conn
            .db
            .game_guild_member()
            .character_guid()
            .find(&character_guid)
            .map(member_view)
    }

    /// One Guild and its Guild Ranks in THIS handle's cache, ranks ordered by `rank_id`.
    pub(crate) fn guild_row(&self, guild_id: u32) -> Option<crate::codec::GuildView> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let guild = db.game_guild().guild_id().find(&guild_id)?;
        let mut ranks: Vec<crate::codec::GuildRankView> = db
            .game_guild_rank()
            .iter()
            .filter(|rank| rank.guild_id == guild_id)
            .map(|rank| crate::codec::GuildRankView {
                rank_id: rank.rank_id,
                name: rank.name,
                rights: rank.rights,
            })
            .collect();
        ranks.sort_by_key(|rank| rank.rank_id);
        Some(crate::codec::GuildView {
            guild_id: guild.guild_id,
            name: guild.name,
            leader_guid: guild.leader_guid,
            team: guild.team,
            motd: guild.motd,
            info: guild.info,
            emblem_style: guild.emblem_style,
            emblem_color: guild.emblem_color,
            border_style: guild.border_style,
            border_color: guild.border_color,
            background_color: guild.background_color,
            created_micros: guild.created_micros,
            ranks,
        })
    }

    /// Every member row of one Guild in THIS handle's cache, ordered by guid.
    pub(crate) fn guild_member_rows(&self, guild_id: u32) -> Vec<crate::codec::GuildMemberView> {
        let mut members: Vec<_> = self
            .0
            .coord()
            .conn
            .db
            .game_guild_member()
            .iter()
            .filter(|member| member.guild_id == guild_id)
            .map(member_view)
            .collect();
        members.sort_by_key(|member| member.character_guid);
        members
    }

    /// The Guild Projection `(guild_id, rank_id)` of `character_guid` in THIS handle's cache.
    /// `(0, 0)` outside a Guild. Cheap enough for a relay job.
    pub(crate) fn guild_projection(&self, character_guid: u64) -> (u32, u32) {
        self.0
            .coord()
            .conn
            .db
            .game_guild_member()
            .character_guid()
            .find(&character_guid)
            .map_or((0, 0), |member| (member.guild_id, member.rank_id))
    }

    /// Facts for one Character from whichever World Shard holds its row. Level and zone come from
    /// the live entity when there is one, on any World Shard, else from the durable row.
    pub(crate) fn guild_character_facts(&self, character_guid: u64) -> Option<CharacterFacts> {
        let shards = self.all_shards();
        let (character, realm_account_id) = shards.iter().find_map(|shard| {
            let guard = shard.0.coord();
            let db = &guard.conn.db;
            let character = db.game_character().guid().find(&character_guid)?;
            let realm_account_id = db
                .game_account_character_owner()
                .character_guid()
                .find(&character_guid)
                .map_or(0, |owner| owner.account_id);
            Some((character, realm_account_id))
        })?;
        let live = shards.iter().find_map(|shard| {
            shard
                .0
                .coord()
                .conn
                .db
                .game_world_entity()
                .guid()
                .find(&character_guid)
        });
        let (level, zone_id) =
            live.as_ref()
                .map_or((character.level, character.zone_id), |entity| {
                    (
                        u8::try_from(entity.level).unwrap_or(u8::MAX),
                        entity.zone_id,
                    )
                });
        Some(CharacterFacts {
            guid: character.guid,
            name: character.name,
            race: character.race,
            class: character.class,
            level,
            zone_id,
            last_logout_micros: character.last_logout_micros,
            online: live.is_some(),
            realm_account_id,
        })
    }

    /// The GM level on THIS handle's own Character row, 0 when it holds none. The guild Gates call
    /// it on the actor's Home Shard, so a copy an interrupted Transfer left elsewhere never counts.
    pub(crate) fn home_gm_level(&self, character_guid: u64) -> u8 {
        self.0
            .coord()
            .conn
            .db
            .game_character()
            .guid()
            .find(&character_guid)
            .map_or(0, |character| character.gm_level)
    }

    /// The unit `actor_guid` has selected, from THIS handle's live entity. 0 for none.
    pub(crate) fn selected_target(&self, actor_guid: u64) -> u64 {
        self.0
            .coord()
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&actor_guid)
            .map_or(0, |entity| entity.target_guid)
    }

    /// The Fee Hold of `payer_guid` in THIS handle's cache. Call it on the payer's Home Shard. A
    /// kind this Gateway does not know reads as none.
    pub(crate) fn guild_fee_hold_row(&self, payer_guid: u64) -> Option<guild_fee::FeeHold> {
        let hold = self
            .0
            .coord()
            .conn
            .db
            .game_guild_fee_hold()
            .payer_guid()
            .find(&payer_guid)?;
        let terms = match hold.kind {
            lyracore_shared::guild::fee_kind::EMBLEM => {
                guild_fee::FeeTerms::Emblem(guild_fee::Emblem {
                    emblem_style: hold.emblem_style,
                    emblem_color: hold.emblem_color,
                    border_style: hold.border_style,
                    border_color: hold.border_color,
                    background_color: hold.background_color,
                })
            }
            _ => return None,
        };
        Some(guild_fee::FeeHold {
            operation_id: hold.operation_id,
            terms,
        })
    }

    /// Realm-core's fee decision for `operation_id` in THIS handle's cache. Call it on the
    /// Realm-core handle.
    pub(crate) fn guild_fee_decision_row(&self, operation_id: u64) -> Option<GuildFeeDecision> {
        self.0
            .coord()
            .conn
            .db
            .game_guild_fee_decision()
            .operation_id()
            .find(&operation_id)
    }
}

#[cfg(test)]
mod tests {
    /// The GM Gate's read must stay on the handle it is called on, the Home Shard. A realm-wide
    /// union takes the first shard that holds the Character, default shard first, and a frozen copy
    /// there would pass the Gate.
    #[test]
    fn the_gm_level_is_read_from_the_home_shard_only() {
        let body = crate::test_scan::code_of(include_str!("guild.rs"), "fn home_gm_level(");
        let flat: String = body.split_whitespace().collect();
        assert!(
            flat.contains("self.0.coord().conn.db.game_character()"),
            "{body}"
        );
        for union in ["all_shards", "world_shards", "realm_core", "world_stores"] {
            assert!(
                !flat.contains(union),
                "`home_gm_level` reads `{union}`:\n{body}"
            );
        }
        let forward = crate::test_scan::code_of(
            include_str!("../../world/handlers/guild.rs"),
            "fn guild_gm_level(&self, actor_guid: u64) -> Result<u8> {",
        );
        let forward: String = forward.split_whitespace().collect();
        assert_eq!(
            forward,
            "{Ok(crate::stdb::Coordinator::home_gm_level(self,actor_guid))}"
        );
    }
}
