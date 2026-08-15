//! Guild cache-accessor methods. Which database the handle points at is the whole meaning of the
//! answer — on the realm-core handle these are the authority, and on a single-database gateway that
//! one database is the authority. Nothing here knows which; routing is `world::guild`'s job.

use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;
use crate::world::guild::{GuildRosterMember, GuildRosterView, GuildView};

impl Coordinator {
    /// One guild, with its member count and its rank names already folded in. A cache read, so it
    /// is cheap enough to run inside an SDK callback: no reducer call, no round trip.
    ///
    /// Rank names come back in rank order, and there are exactly
    /// `lyracore_shared::guild::GUILD_RANK_COUNT` of them for any guild the module wrote.
    pub fn guild_view(&self, guild_id: u64) -> Option<GuildView> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let guild = db.game_guild().guild_id().find(&guild_id)?;
        let member_count = db
            .game_guild_member()
            .iter()
            .filter(|m| m.guild_id == guild_id)
            .count() as u32;
        let mut ranks: Vec<(u32, String)> = db
            .game_guild_rank()
            .iter()
            .filter(|r| r.guild_id == guild_id)
            .map(|r| (r.rank_index, r.name))
            .collect();
        ranks.sort_by_key(|(index, _)| *index);
        Some(GuildView {
            guild_id,
            name: guild.name,
            master_guid: guild.master_guid,
            motd: guild.motd,
            info_text: guild.info_text,
            created_micros: guild.created_at.to_micros_since_unix_epoch(),
            member_count,
            rank_names: ranks.into_iter().map(|(_, name)| name).collect(),
        })
    }

    /// `character_guid`'s `(guild_id, rank_index)`, or `None` when it is in no guild.
    pub fn guild_membership(&self, character_guid: u64) -> Option<(u64, u32)> {
        let guard = self.0.coord();
        let found = guard
            .conn
            .db
            .game_guild_member()
            .iter()
            .find(|m| m.character_guid == character_guid)?;
        Some((found.guild_id, found.rank_index))
    }

    /// One guild's roster: the guild text, the per-rank rights in rank order, and every member row
    /// in join order. Another cache read, so the guild panel costs no round trip.
    ///
    /// Members carry guids, ranks and notes only. On realm-core there is no `game_character` row to
    /// join a name, level, class or area from — `world::guild::render_roster` fans out to the shards
    /// for those.
    pub fn guild_roster_view(&self, guild_id: u64) -> Option<GuildRosterView> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let guild = db.game_guild().guild_id().find(&guild_id)?;
        let mut ranks: Vec<(u32, u32)> = db
            .game_guild_rank()
            .iter()
            .filter(|r| r.guild_id == guild_id)
            .map(|r| (r.rank_index, r.rights))
            .collect();
        ranks.sort_by_key(|(index, _)| *index);
        let mut members: Vec<(u64, GuildRosterMember)> = db
            .game_guild_member()
            .iter()
            .filter(|m| m.guild_id == guild_id)
            .map(|m| {
                (
                    m.id,
                    GuildRosterMember {
                        guid: m.character_guid,
                        rank: m.rank_index,
                        public_note: m.public_note,
                        officer_note: m.officer_note,
                    },
                )
            })
            .collect();
        // Join order, the order the module's own `members_of` reads them in.
        members.sort_by_key(|(id, _)| *id);
        Some(GuildRosterView {
            guild_id,
            motd: guild.motd,
            info_text: guild.info_text,
            rank_rights: ranks.into_iter().map(|(_, rights)| rights).collect(),
            members: members.into_iter().map(|(_, m)| m).collect(),
        })
    }

    /// Every member guid of `guild_id`, in join order (member-row id) — the order the module's own
    /// `members_of` answers in. The disband fan-out's input.
    pub fn guild_member_guids(&self, guild_id: u64) -> Vec<u64> {
        let guard = self.0.coord();
        let mut members: Vec<(u64, u64)> = guard
            .conn
            .db
            .game_guild_member()
            .iter()
            .filter(|m| m.guild_id == guild_id)
            .map(|m| (m.id, m.character_guid))
            .collect();
        members.sort_by_key(|(id, _)| *id);
        members.into_iter().map(|(_, guid)| guid).collect()
    }
}
