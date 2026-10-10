//! `Coordinator`'s [`GuildActionStore`] adapter.

use anyhow::Result;
use lyracore_shared::guild::GuildRefusal;

use crate::codec;
use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, reducer_refusal_reason};
use crate::stdb::reads::petition_view;
use crate::stdb::Coordinator;
use crate::world::{
    presence, whisper, CharacterFacts, GuildActionStore, GuildOutcome, GuildRequest,
};

impl GuildActionStore for crate::stdb::Coordinator {
    fn guild_member(&self, character_guid: u64) -> Result<Option<codec::GuildMemberView>> {
        Ok(self.realm_core()?.guild_member_row(character_guid))
    }

    fn guild(&self, guild_id: u32) -> Result<Option<codec::GuildView>> {
        Ok(self.realm_core()?.guild_row(guild_id))
    }

    fn guild_members(&self, guild_id: u32) -> Result<Vec<codec::GuildMemberView>> {
        Ok(self.realm_core()?.guild_member_rows(guild_id))
    }

    fn guild_character_facts(&self, character_guid: u64) -> Result<Option<CharacterFacts>> {
        crate::stdb::Coordinator::guild_character_facts(self, character_guid)
    }

    fn guild_characters_named(&self, name: &str) -> Result<Vec<u64>> {
        presence::resolve_all_by_name(self, name)
    }

    fn guild_gm_level(&self, actor_guid: u64) -> Result<u8> {
        Ok(crate::stdb::Coordinator::home_gm_level(self, actor_guid))
    }

    fn guild_selected_target(&self, actor_guid: u64) -> u64 {
        crate::stdb::Coordinator::selected_target(self, actor_guid)
    }

    fn guild_ignored_by(&self, owner_guid: u64, other_guid: u64) -> Result<bool> {
        whisper::ignored_anywhere(self, owner_guid, other_guid)
    }

    fn guild_op(&self, actor_guid: u64, request: GuildRequest) -> Result<GuildOutcome> {
        self.realm_core()?.realm_guild_op(actor_guid, request)
    }

    fn guild_npc_refuses(&self, npc_guid: u64, actor_guid: u64) -> Result<bool> {
        crate::stdb::Coordinator::npc_refuses_interaction(self, npc_guid, actor_guid)
    }

    fn guild_petition_of_charter(
        &self,
        charter_item_guid: u64,
    ) -> Result<Option<codec::PetitionView>> {
        Ok(self
            .realm_core()?
            .guild_petition_of_charter(charter_item_guid))
    }

    fn guild_petition_of_owner(&self, owner_guid: u64) -> Result<Option<codec::PetitionView>> {
        Ok(self.realm_core()?.guild_petition_of_owner(owner_guid))
    }

    fn guild_name_taken(&self, name: &str) -> Result<bool> {
        Ok(self.realm_core()?.guild_name_taken(name))
    }

    fn guild_holds_charter(&self, actor_guid: u64, charter_item_guid: u64) -> Result<bool> {
        Ok(self.holds_guild_charter(actor_guid, charter_item_guid))
    }

    fn guild_destroy_charter(&self, actor_guid: u64, charter_item_guid: u64) -> Result<()> {
        self.destroy_guild_charter(actor_guid, charter_item_guid)
    }

    fn guild_character_guids(&self) -> Result<Vec<u64>> {
        self.realm_core()?.guild_character_guids()
    }

    fn guild_names_character(&self, character_guid: u64) -> Result<bool> {
        self.realm_core()?.guild_names_character(character_guid)
    }

    fn guild_name_of_member(&self, character_guid: u64) -> Result<Option<String>> {
        Ok(self.realm_core()?.guild_name_of_member(character_guid))
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
        let rank_ids = guard.guilds.read().unwrap().rank_ids(guild_id);
        let ranks_table = db.game_guild_rank();
        let mut ranks: Vec<crate::codec::GuildRankView> = rank_ids
            .into_iter()
            .filter_map(|id| ranks_table.id().find(&id))
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
        let guard = self.0.coord();
        let guids = guard.guilds.read().unwrap().member_guids(guild_id);
        let members = guard.conn.db.game_guild_member();
        guids
            .into_iter()
            .filter_map(|guid| members.character_guid().find(&guid))
            .filter(|member| member.guild_id == guild_id)
            .map(member_view)
            .collect()
    }

    /// Every Character that a Guild, a Petition or a Signature in THIS handle's cache names: the
    /// Characters the deleted-Character reconciliation checks. Call it on the Realm-core handle.
    /// It fails while the subscription is unhealthy, so a stale cache never hides a Character.
    pub(crate) fn guild_character_guids(&self) -> Result<Vec<u64>> {
        let guard = self.0.coord();
        if !guard.is_healthy() {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for guild cleanup",
                self.shard_name()
            );
        }
        let named = guard.guilds.read().unwrap().named_characters();
        Ok(named.into_iter().collect())
    }

    /// Does a Guild, a Petition or a Signature in THIS handle's cache name `character_guid`? Keyed
    /// reads only. Call it on the Realm-core handle. Fails while the subscription is unhealthy.
    pub(crate) fn guild_names_character(&self, character_guid: u64) -> Result<bool> {
        let guard = self.0.coord();
        if !guard.is_healthy() {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for guild cleanup",
                self.shard_name()
            );
        }
        let member = guard
            .conn
            .db
            .game_guild_member()
            .character_guid()
            .find(&character_guid)
            .is_some();
        Ok(member
            || guard
                .guilds
                .read()
                .unwrap()
                .petition_characters
                .contains_key(&character_guid))
    }

    /// The name of the Guild `character_guid` belongs to in THIS handle's cache, if any. Two keyed
    /// reads, cheap enough for every `/who` row.
    pub(crate) fn guild_name_of_member(&self, character_guid: u64) -> Option<String> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let member = db
            .game_guild_member()
            .character_guid()
            .find(&character_guid)?;
        db.game_guild()
            .guild_id()
            .find(&member.guild_id)
            .map(|guild| guild.name)
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

    /// The open Petition `owner_guid` owns in THIS handle's cache. Call it on the Realm-core
    /// handle.
    pub(crate) fn guild_petition_of_owner(
        &self,
        owner_guid: u64,
    ) -> Option<crate::codec::PetitionView> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let row = db.game_guild_petition().owner_guid().find(&owner_guid)?;
        Some(petition_view(db, row))
    }

    /// Does a Guild in THIS handle's cache hold `name`, without regard to case? Call it on the
    /// Realm-core handle.
    pub(crate) fn guild_name_taken(&self, name: &str) -> bool {
        self.0
            .coord()
            .conn
            .db
            .game_guild()
            .name_key()
            .find(&lyracore_shared::guild::name_key(name))
            .is_some()
    }

    /// Does `actor_guid` hold the Guild Charter `charter_item_guid` in THIS handle's cache? Call it
    /// on the actor's Home Shard.
    pub(crate) fn holds_guild_charter(&self, actor_guid: u64, charter_item_guid: u64) -> bool {
        self.0
            .coord()
            .conn
            .db
            .game_item_instance()
            .guid()
            .find(&charter_item_guid)
            .is_some_and(|item| {
                item.owner_guid == actor_guid
                    && item.entry == lyracore_shared::guild::GUILD_CHARTER_ENTRY
            })
    }

    /// `realm_guild_op`: one guild op against the database THIS handle points at. Callers hold the
    /// Realm-core handle. The actor is the guid this World Session entered the world with.
    pub fn realm_guild_op(
        &self,
        actor_guid: u64,
        request: crate::world::GuildRequest,
    ) -> Result<crate::world::GuildOutcome> {
        use crate::world::GuildRequest;
        let op = match request {
            GuildRequest::GmCreate {
                leader_guid,
                leader_name,
                leader_team,
                leader_realm_account,
                gm_level,
                name,
            } => GuildOp::GmCreate(GuildGmCreate {
                leader_guid,
                leader_name,
                leader_team,
                leader_realm_account,
                gm_level,
                name,
            }),
            GuildRequest::SignOn { actor_name } => GuildOp::SignOn(actor_name),
            GuildRequest::SignOff => GuildOp::SignOff,
            GuildRequest::Invite {
                target_guid,
                actor_team,
                target_team,
                target_ignores_actor,
            } => GuildOp::Invite(GuildInviteRequest {
                target_guid,
                actor_team,
                target_team,
                target_ignores_actor,
            }),
            GuildRequest::Accept {
                actor_name,
                actor_team,
            } => GuildOp::Accept(GuildAcceptRequest {
                actor_name,
                actor_team,
            }),
            GuildRequest::Decline { actor_name } => GuildOp::Decline(actor_name),
            GuildRequest::Leave => GuildOp::Leave,
            GuildRequest::Remove { target_guid } => GuildOp::Remove(target_guid),
            GuildRequest::Promote { target_guid } => GuildOp::Promote(target_guid),
            GuildRequest::Demote { target_guid } => GuildOp::Demote(target_guid),
            GuildRequest::SetLeader { target_guid } => GuildOp::SetLeader(target_guid),
            GuildRequest::Disband => GuildOp::Disband,
            GuildRequest::SetMotd { text } => GuildOp::SetMotd(text),
            GuildRequest::SetInfo { text } => GuildOp::SetInfo(text),
            GuildRequest::SetPublicNote { target_guid, text } => {
                GuildOp::SetPublicNote(GuildNoteEdit { target_guid, text })
            }
            GuildRequest::SetOfficerNote { target_guid, text } => {
                GuildOp::SetOfficerNote(GuildNoteEdit { target_guid, text })
            }
            GuildRequest::EditRank {
                rank_id,
                rights,
                name,
            } => GuildOp::EditRank(GuildRankEdit {
                rank_id,
                rights,
                name,
            }),
            GuildRequest::AddRank { name } => GuildOp::AddRank(name),
            GuildRequest::DeleteRank => GuildOp::DeleteRank,
            GuildRequest::SignPetition {
                charter_item_guid,
                actor_name,
                actor_team,
            } => GuildOp::SignPetition(GuildPetitionSign {
                charter_item_guid,
                actor_name,
                actor_team,
            }),
            GuildRequest::OfferPetition {
                charter_item_guid,
                target_guid,
                target_team,
            } => GuildOp::OfferPetition(GuildPetitionOffer {
                charter_item_guid,
                target_guid,
                target_team,
            }),
            GuildRequest::DeclinePetition { charter_item_guid } => {
                GuildOp::DeclinePetition(charter_item_guid)
            }
            GuildRequest::RenamePetition {
                charter_item_guid,
                name,
            } => GuildOp::RenamePetition(GuildPetitionRename {
                charter_item_guid,
                name,
            }),
            GuildRequest::TurnInPetition { charter_item_guid } => {
                GuildOp::TurnInPetition(charter_item_guid)
            }
            GuildRequest::ClosePetition { petition_id } => GuildOp::ClosePetition(petition_id),
            GuildRequest::ForgetDeletedCharacter => GuildOp::ForgetDeletedCharacter,
        };
        let result = call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_guild_op",
            realm_guild_op_then(self.actor_or_owner(actor_guid), op)
        );
        match result {
            Ok(()) => Ok(crate::world::GuildOutcome::Ran),
            Err(error) => match reducer_refusal_reason(&error).and_then(GuildRefusal::parse_tag) {
                Some(refusal) => Ok(crate::world::GuildOutcome::Refused(refusal)),
                None => Err(error),
            },
        }
    }

    /// `gw_destroy_guild_charter` on THIS handle, the actor's Home Shard. A Charter already gone
    /// is Ok.
    pub(crate) fn destroy_guild_charter(
        &self,
        actor_guid: u64,
        charter_item_guid: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_destroy_guild_charter",
            gw_destroy_guild_charter_then(self.actor_or_owner(actor_guid), charter_item_guid)
        )
    }
}

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
