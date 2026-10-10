use super::super::*;

#[derive(Default)]
pub(crate) struct GuildState {
    /// Realm-core Guilds the guild query answers from.
    pub(crate) guilds: Vec<codec::GuildView>,
    /// Realm-core member rows. Guild ops are recorded in `calls` as `guild_op:<request>`.
    pub(crate) guild_memberships: Vec<codec::GuildMemberView>,
    /// A Fee Hold left on this Home Shard. Fee steps are recorded in `calls` as `guild_fee_<step>`.
    pub(crate) guild_fee_hold: std::sync::Mutex<Option<crate::world::guild_fee::FeeHold>>,
    /// Realm-core Petitions.
    pub(crate) guild_petitions: Vec<codec::PetitionView>,
    /// A Character whose guild lookup fails, as when one Realm-core read errors.
    pub(crate) guild_lookup_error_for: Option<u64>,
}

impl GuildActionStore for WorldFake {
    fn guild_member(&self, character_guid: u64) -> Result<Option<codec::GuildMemberView>> {
        Ok(self
            .guild
            .guild_memberships
            .iter()
            .find(|member| member.character_guid == character_guid)
            .cloned())
    }

    fn guild(&self, guild_id: u32) -> Result<Option<codec::GuildView>> {
        Ok(self
            .guild
            .guilds
            .iter()
            .find(|guild| guild.guild_id == guild_id)
            .cloned())
    }

    fn guild_members(&self, _guild_id: u32) -> Result<Vec<codec::GuildMemberView>> {
        Ok(Vec::new())
    }

    fn guild_character_facts(&self, character_guid: u64) -> Result<Option<CharacterFacts>> {
        Ok(self
            .characters
            .iter()
            .find(|c| c.guid == character_guid)
            .map(|c| CharacterFacts {
                guid: c.guid,
                name: c.name.clone(),
                race: c.race,
                class: c.class,
                level: c.level,
                zone_id: c.zone_id,
                last_logout_micros: 0,
                online: !self.social.offline_guids.contains(&c.guid),
                in_transit: false,
                realm_account_id: 0,
            }))
    }

    fn guild_characters_named(&self, name: &str) -> Result<Vec<u64>> {
        Ok(self
            .characters
            .iter()
            .filter(|c| c.name.eq_ignore_ascii_case(name))
            .map(|c| c.guid)
            .collect())
    }

    fn guild_gm_level(&self, _actor_guid: u64) -> Result<u8> {
        Ok(0)
    }

    fn guild_selected_target(&self, _actor_guid: u64) -> u64 {
        0
    }

    fn guild_ignored_by(&self, _owner_guid: u64, _other_guid: u64) -> Result<bool> {
        Ok(false)
    }

    fn guild_op(&self, actor: Actor, request: GuildRequest) -> Result<GuildOutcome> {
        if request == GuildRequest::ForgetDeletedCharacter {
            self.rec(&format!("guild_op:ForgetDeletedCharacter:{}", actor.guid()));
            return Ok(GuildOutcome::Ran);
        }
        self.rec(match request {
            GuildRequest::GmCreate { .. } => "guild_op:GmCreate",
            GuildRequest::SignOn { .. } => "guild_op:SignOn",
            GuildRequest::SignOff => "guild_op:SignOff",
            GuildRequest::Invite { .. } => "guild_op:Invite",
            GuildRequest::Accept { .. } => "guild_op:Accept",
            GuildRequest::Decline { .. } => "guild_op:Decline",
            GuildRequest::Leave => "guild_op:Leave",
            GuildRequest::Remove { .. } => "guild_op:Remove",
            GuildRequest::Promote { .. } => "guild_op:Promote",
            GuildRequest::Demote { .. } => "guild_op:Demote",
            GuildRequest::SetLeader { .. } => "guild_op:SetLeader",
            GuildRequest::Disband => "guild_op:Disband",
            GuildRequest::SetMotd { .. } => "guild_op:SetMotd",
            GuildRequest::SetInfo { .. } => "guild_op:SetInfo",
            GuildRequest::SetPublicNote { .. } => "guild_op:SetPublicNote",
            GuildRequest::SetOfficerNote { .. } => "guild_op:SetOfficerNote",
            GuildRequest::EditRank { .. } => "guild_op:EditRank",
            GuildRequest::AddRank { .. } => "guild_op:AddRank",
            GuildRequest::DeleteRank => "guild_op:DeleteRank",
            GuildRequest::SignPetition { .. } => "guild_op:SignPetition",
            GuildRequest::OfferPetition { .. } => "guild_op:OfferPetition",
            GuildRequest::DeclinePetition { .. } => "guild_op:DeclinePetition",
            GuildRequest::RenamePetition { .. } => "guild_op:RenamePetition",
            GuildRequest::TurnInPetition { .. } => "guild_op:TurnInPetition",
            GuildRequest::ClosePetition { .. } => "guild_op:ClosePetition",
            GuildRequest::ForgetDeletedCharacter => unreachable!("recorded with its actor above"),
        });
        Ok(GuildOutcome::Ran)
    }

    fn guild_npc_refuses(&self, _npc_guid: u64, _actor_guid: u64) -> Result<bool> {
        Ok(false)
    }

    fn guild_petition_of_charter(
        &self,
        charter_item_guid: u64,
    ) -> Result<Option<codec::PetitionView>> {
        Ok(self
            .guild
            .guild_petitions
            .iter()
            .find(|petition| petition.charter_item_guid == charter_item_guid)
            .cloned())
    }

    fn guild_petition_of_owner(&self, owner_guid: u64) -> Result<Option<codec::PetitionView>> {
        Ok(self
            .guild
            .guild_petitions
            .iter()
            .find(|petition| petition.owner_guid == owner_guid)
            .cloned())
    }

    fn guild_name_taken(&self, name: &str) -> Result<bool> {
        Ok(self
            .guild
            .guilds
            .iter()
            .any(|guild| guild.name.eq_ignore_ascii_case(name)))
    }

    fn guild_holds_charter(&self, _actor_guid: u64, _charter_item_guid: u64) -> Result<bool> {
        Ok(false)
    }

    fn guild_destroy_charter(&self, _actor: Actor, _charter_item_guid: u64) -> Result<()> {
        Ok(())
    }

    fn guild_character_guids(&self) -> Result<Vec<u64>> {
        let mut guids: Vec<u64> = self
            .guild
            .guild_memberships
            .iter()
            .map(|member| member.character_guid)
            .chain(self.guild.guild_petitions.iter().flat_map(|petition| {
                std::iter::once(petition.owner_guid).chain(petition.signers.iter().copied())
            }))
            .collect();
        guids.sort_unstable();
        guids.dedup();
        Ok(guids)
    }

    fn guild_names_character(&self, character_guid: u64) -> Result<bool> {
        if self.guild.guild_lookup_error_for == Some(character_guid) {
            return Err(crate::stdb::ReducerCallError::transport_lost(&format!(
                "guild lookup for {character_guid}"
            ))
            .into());
        }
        Ok(self.guild_character_guids()?.contains(&character_guid))
    }

    fn guild_name_of_member(&self, character_guid: u64) -> Result<Option<String>> {
        let Some(member) = self.guild_member(character_guid)? else {
            return Ok(None);
        };
        Ok(self.guild(member.guild_id)?.map(|guild| guild.name))
    }
}

/// Realm-core refuses every fee: the socket tests only watch a leftover hold finish.
impl crate::world::guild_fee::GuildFeeStore for WorldFake {
    fn guild_fee_held(&self, _actor: Actor) -> Result<Option<crate::world::guild_fee::FeeHold>> {
        Ok(self.guild.guild_fee_hold.lock().unwrap().clone())
    }

    fn guild_fee_hold(
        &self,
        _actor: Actor,
        _request: crate::world::guild_fee::FeeRequest,
    ) -> Result<Result<crate::world::guild_fee::FeeHold, lyracore_shared::guild::GuildRefusal>>
    {
        Ok(Err(lyracore_shared::guild::GuildRefusal::NotEnoughMoney))
    }

    fn guild_fee_decide(
        &self,
        _actor: Actor,
        _hold: crate::world::guild_fee::FeeHold,
    ) -> Result<crate::world::guild_fee::FeeOutcome> {
        self.rec("guild_fee_decide");
        Ok(crate::world::guild_fee::FeeOutcome::Refused(
            lyracore_shared::guild::GuildRefusal::NotLeader,
        ))
    }

    fn guild_fee_finish(&self, _actor: Actor, _operation_id: u64, _accepted: bool) -> Result<()> {
        self.rec("guild_fee_finish");
        *self.guild.guild_fee_hold.lock().unwrap() = None;
        Ok(())
    }
}
