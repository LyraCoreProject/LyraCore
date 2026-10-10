//! `Coordinator`'s [`GuildActionStore`] adapter.

use anyhow::Result;
use lyracore_shared::guild::GuildRefusal;

use crate::codec;
use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, reducer_refusal_reason};
use crate::stdb::reads::petition_view;
use crate::stdb::Coordinator;
use crate::world::{
    presence, whisper, Actor, CharacterFacts, GuildActionStore, GuildOutcome, GuildRequest,
};

impl GuildActionStore for Coordinator {
    /// The membership row of `character_guid` on Realm-core.
    fn guild_member(&self, character_guid: u64) -> Result<Option<codec::GuildMemberView>> {
        let realm = self.realm_core()?;
        let member = realm
            .0
            .coord()
            .conn
            .db
            .game_guild_member()
            .character_guid()
            .find(&character_guid)
            .map(member_view);
        Ok(member)
    }

    /// One Guild and its Guild Ranks on Realm-core, ranks ordered by `rank_id`.
    fn guild(&self, guild_id: u32) -> Result<Option<codec::GuildView>> {
        let realm = self.realm_core()?;
        let guard = realm.0.coord();
        let db = &guard.conn.db;
        let Some(guild) = db.game_guild().guild_id().find(&guild_id) else {
            return Ok(None);
        };
        let rank_ids = guard.guilds.read().unwrap().rank_ids(guild_id);
        let ranks_table = db.game_guild_rank();
        let mut ranks: Vec<codec::GuildRankView> = rank_ids
            .into_iter()
            .filter_map(|id| ranks_table.id().find(&id))
            .map(|rank| codec::GuildRankView {
                rank_id: rank.rank_id,
                name: rank.name,
                rights: rank.rights,
            })
            .collect();
        ranks.sort_by_key(|rank| rank.rank_id);
        Ok(Some(codec::GuildView {
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
        }))
    }

    /// Every member row of one Guild on Realm-core, ordered by guid.
    fn guild_members(&self, guild_id: u32) -> Result<Vec<codec::GuildMemberView>> {
        let realm = self.realm_core()?;
        let guard = realm.0.coord();
        let guids = guard.guilds.read().unwrap().member_guids(guild_id);
        let members = guard.conn.db.game_guild_member();
        Ok(guids
            .into_iter()
            .filter_map(|guid| members.character_guid().find(&guid))
            .filter(|member| member.guild_id == guild_id)
            .map(member_view)
            .collect())
    }

    fn guild_character_facts(&self, character_guid: u64) -> Result<Option<CharacterFacts>> {
        crate::stdb::Coordinator::guild_character_facts(self, character_guid)
    }

    fn guild_characters_named(&self, name: &str) -> Result<Vec<u64>> {
        presence::resolve_all_by_name(self, name)
    }

    fn guild_gm_level(&self, actor_guid: u64) -> Result<u8> {
        Ok(self.home_gm_level(actor_guid))
    }

    /// The unit `actor_guid` has selected, from this Home Shard's live entity. 0 for none.
    fn guild_selected_target(&self, actor_guid: u64) -> u64 {
        self.0
            .coord()
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&actor_guid)
            .map_or(0, |entity| entity.target_guid)
    }

    fn guild_ignored_by(&self, owner_guid: u64, other_guid: u64) -> Result<bool> {
        whisper::ignored_anywhere(self, owner_guid, other_guid)
    }

    /// `realm_guild_op`: one guild op on Realm-core, or on the one database of an unsharded
    /// Realm.
    fn guild_op(&self, actor: Actor, request: GuildRequest) -> Result<GuildOutcome> {
        let realm = self.realm_core()?;
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
        guild_outcome(call_reducer!(
            realm.0.call_pipe().conn.reducers,
            "realm_guild_op",
            realm_guild_op_then(realm.session_actor(actor), op)
        ))
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

    /// The open Petition `owner_guid` owns on Realm-core.
    fn guild_petition_of_owner(&self, owner_guid: u64) -> Result<Option<codec::PetitionView>> {
        let realm = self.realm_core()?;
        let guard = realm.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_guild_petition()
            .owner_guid()
            .find(&owner_guid)
            .map(|row| petition_view(db, row)))
    }

    /// Does a Guild on Realm-core hold `name`, without regard to case?
    fn guild_name_taken(&self, name: &str) -> Result<bool> {
        let realm = self.realm_core()?;
        let taken = realm
            .0
            .coord()
            .conn
            .db
            .game_guild()
            .name_key()
            .find(&lyracore_shared::guild::name_key(name))
            .is_some();
        Ok(taken)
    }

    /// Does `actor_guid` hold the Guild Charter `charter_item_guid` on this Home Shard?
    fn guild_holds_charter(&self, actor_guid: u64, charter_item_guid: u64) -> Result<bool> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_item_instance()
            .guid()
            .find(&charter_item_guid)
            .is_some_and(|item| {
                item.owner_guid == actor_guid
                    && item.entry == lyracore_shared::guild::GUILD_CHARTER_ENTRY
            }))
    }

    /// `gw_destroy_guild_charter` on this Home Shard. A Charter already gone is Ok.
    fn guild_destroy_charter(&self, actor: Actor, charter_item_guid: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_destroy_guild_charter",
            gw_destroy_guild_charter_then(self.session_actor(actor), charter_item_guid)
        )
    }

    /// Every Character that a Guild, a Petition or a Signature on Realm-core names: the
    /// Characters the deleted-Character reconciliation checks. It fails while the subscription
    /// is unhealthy, so a stale cache never hides a Character.
    fn guild_character_guids(&self) -> Result<Vec<u64>> {
        let realm = self.realm_core()?;
        let guard = realm.0.coord();
        if !guard.is_healthy() {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for guild cleanup",
                realm.shard_name()
            );
        }
        let named = guard.guilds.read().unwrap().named_characters();
        Ok(named.into_iter().collect())
    }

    /// Does a Guild, a Petition or a Signature on Realm-core name `character_guid`? Keyed reads
    /// only. Fails while the subscription is unhealthy.
    fn guild_names_character(&self, character_guid: u64) -> Result<bool> {
        let realm = self.realm_core()?;
        let guard = realm.0.coord();
        if !guard.is_healthy() {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for guild cleanup",
                realm.shard_name()
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

    /// The name of the Guild `character_guid` belongs to on Realm-core, if any. Two keyed reads,
    /// cheap enough for every `/who` row.
    fn guild_name_of_member(&self, character_guid: u64) -> Result<Option<String>> {
        let realm = self.realm_core()?;
        let guard = realm.0.coord();
        let db = &guard.conn.db;
        let Some(member) = db
            .game_guild_member()
            .character_guid()
            .find(&character_guid)
        else {
            return Ok(None);
        };
        Ok(db
            .game_guild()
            .guild_id()
            .find(&member.guild_id)
            .map(|guild| guild.name))
    }
}

/// The Module's typed guild Refusal. Only a reducer the Module rejected carries a tag; a timeout,
/// transport, or SDK failure stays an error with an unknown outcome.
fn guild_outcome(result: Result<()>) -> Result<GuildOutcome> {
    match result {
        Ok(()) => Ok(GuildOutcome::Ran),
        Err(error) => match reducer_refusal_reason(&error).and_then(GuildRefusal::parse_tag) {
            Some(refusal) => Ok(GuildOutcome::Refused(refusal)),
            None => Err(error),
        },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdb::{classify, DurableFailure, ReducerCallError};

    #[test]
    fn a_rejected_guild_tag_decodes_to_its_refusal() {
        let error = ReducerCallError::refused("realm_guild_op", GuildRefusal::NotLeader.as_tag());
        assert_eq!(
            guild_outcome(Err(error.into())).unwrap(),
            GuildOutcome::Refused(GuildRefusal::NotLeader)
        );
    }

    #[test]
    fn a_committed_op_decodes_to_ran() {
        assert_eq!(guild_outcome(Ok(())).unwrap(), GuildOutcome::Ran);
    }

    #[test]
    fn an_unknown_rejection_stays_a_refusal_for_the_handler() {
        let error = guild_outcome(Err(
            ReducerCallError::refused("realm_guild_op", "mystery").into()
        ))
        .unwrap_err();
        assert!(matches!(classify(&error), DurableFailure::Refusal { .. }));
    }

    #[test]
    fn a_lost_transport_stays_a_transport_loss() {
        let error = guild_outcome(Err(
            ReducerCallError::transport_lost("realm_guild_op").into()
        ))
        .unwrap_err();
        assert_eq!(classify(&error), DurableFailure::TransportLoss);
    }
}
