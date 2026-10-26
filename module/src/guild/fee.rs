use lyracore_shared::constants::npc_flags;
use lyracore_shared::guild::{
    event_kind, fee_kind, name_key, validate_guild_name, GuildRefusal, CHARTER_COST_COPPER,
    EMBLEM_COST_COPPER, GUILD_CHARTER_ENTRY,
};
use spacetimedb::{table, ReducerContext, SpacetimeType, Table};

use super::petition::game_guild_petition;
use super::{game_guild, member, petition, push_event};
use crate::items::{game_item_instance, game_item_template};

#[derive(SpacetimeType, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuildEmblem {
    pub emblem_style: u32,
    pub emblem_color: u32,
    pub border_style: u32,
    pub border_color: u32,
    pub background_color: u32,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuildEmblemPurchase {
    pub npc_guid: u64,
    pub emblem: GuildEmblem,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildCharterPurchase {
    pub npc_guid: u64,
    pub name: String,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum GuildFeeRequest {
    Emblem(GuildEmblemPurchase),
    Charter(GuildCharterPurchase),
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildCharterTerms {
    pub charter_item_guid: u64,
    pub name: String,
    pub payer_name: String,
    pub payer_team: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fee {
    Emblem,
    Charter,
}

impl Fee {
    fn kind(self) -> u8 {
        match self {
            Self::Emblem => fee_kind::EMBLEM,
            Self::Charter => fee_kind::CHARTER,
        }
    }

    fn copper(self) -> u32 {
        match self {
            Self::Emblem => EMBLEM_COST_COPPER,
            Self::Charter => CHARTER_COST_COPPER,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FeeError {
    Refused(GuildRefusal),
    Conflict(&'static str),
}

impl FeeError {
    fn into_text(self) -> String {
        match self {
            Self::Refused(refusal) => refusal.as_tag().to_string(),
            Self::Conflict(reason) => format!("guild fee: {reason}"),
        }
    }
}

fn payment_gate(
    purse: &CtxPurse,
    payer_guid: u64,
    npc_guid: u64,
    fee: Fee,
) -> Result<(), FeeError> {
    if !purse.npc_serves(payer_guid, npc_guid, fee.kind()) {
        return Err(FeeError::Refused(GuildRefusal::NpcRefused));
    }
    if fee == Fee::Charter && !purse.charter_known() {
        return Err(FeeError::Refused(GuildRefusal::CharterUnavailable));
    }
    if purse
        .purse(payer_guid)
        .is_none_or(|copper| copper < fee.copper())
    {
        return Err(FeeError::Refused(GuildRefusal::NotEnoughMoney));
    }
    if fee == Fee::Charter {
        purse.charter_room(payer_guid).map_err(FeeError::Refused)?;
    }
    Ok(())
}

// The caller must roll back on error, including item creation after the copper debit.
pub(crate) fn purchase(
    ctx: &ReducerContext,
    payer_guid: u64,
    request: GuildFeeRequest,
) -> Result<(), String> {
    let apply = || -> Result<(), FeeError> {
        let purse = CtxPurse { ctx };
        let ledger = CtxLedger { ctx };
        match request {
            GuildFeeRequest::Emblem(request) => {
                let guild_id = emblem_gate(&ledger, payer_guid).map_err(FeeError::Refused)?;
                payment_gate(&purse, payer_guid, request.npc_guid, Fee::Emblem)?;
                debit(ctx, payer_guid, EMBLEM_COST_COPPER)?;
                ledger.apply_emblem(guild_id, &request.emblem);
            }
            GuildFeeRequest::Charter(request) => {
                if let Some(petition) = ctx.db.game_guild_petition().owner_guid().find(payer_guid) {
                    if !petition::holds_charter(ctx, payer_guid, petition.charter_item_guid) {
                        petition::close(ctx, payer_guid, petition.petition_id)
                            .map_err(FeeError::Refused)?;
                    }
                }
                let character = crate::helpers::character_by_guid(ctx, payer_guid)
                    .ok_or(FeeError::Refused(GuildRefusal::NoSuchCharacter))?;
                let mut charter = GuildCharterTerms {
                    charter_item_guid: 0,
                    name: request.name.clone(),
                    payer_name: character.name,
                    payer_team: lyracore_shared::faction::team_for_race(character.race),
                };
                charter_gate(&ledger, payer_guid, &charter).map_err(FeeError::Refused)?;
                payment_gate(&purse, payer_guid, request.npc_guid, Fee::Charter)?;
                let identity = debit(ctx, payer_guid, CHARTER_COST_COPPER)?;
                charter.charter_item_guid = purse.create_charter(payer_guid, identity)?;
                ledger.open_petition(payer_guid, &charter);
            }
        }
        Ok(())
    };
    apply().map_err(FeeError::into_text)
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_guild_fee_purchase(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    request: GuildFeeRequest,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let payer_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    purchase(ctx, payer_guid, request)
}

fn debit(ctx: &ReducerContext, guid: u64, copper: u32) -> Result<spacetimedb::Identity, FeeError> {
    let mut payer = crate::helpers::acting_entity_by_guid(ctx, guid)
        .ok_or(FeeError::Refused(GuildRefusal::NotEnoughMoney))?;
    payer.money = payer
        .money
        .checked_sub(copper)
        .ok_or(FeeError::Refused(GuildRefusal::NotEnoughMoney))?;
    let identity = payer.owner_identity;
    payer.update(ctx);
    Ok(identity)
}

fn charter_fits(max_count: u32, held: usize) -> bool {
    max_count == 0 || held < max_count as usize
}

fn emblem_gate(ledger: &CtxLedger, payer_guid: u64) -> Result<u32, GuildRefusal> {
    let (guild_id, leader_guid) = ledger
        .guild_of(payer_guid)
        .ok_or(GuildRefusal::NotInGuild)?;
    if leader_guid != payer_guid {
        return Err(GuildRefusal::NotLeader);
    }
    Ok(guild_id)
}

fn charter_gate(
    ledger: &CtxLedger,
    payer_guid: u64,
    terms: &GuildCharterTerms,
) -> Result<(), GuildRefusal> {
    if ledger.guild_of(payer_guid).is_some() {
        return Err(GuildRefusal::AlreadyInGuild);
    }
    if ledger.owns_petition(payer_guid) {
        return Err(GuildRefusal::AlreadyHasPetition);
    }
    validate_guild_name(&terms.name)?;
    if ledger.guild_name_taken(&terms.name) {
        return Err(GuildRefusal::NameExists);
    }
    Ok(())
}

pub(crate) fn npc_serves(ctx: &ReducerContext, payer_guid: u64, npc_guid: u64, kind: u8) -> bool {
    let required_flags = match kind {
        fee_kind::EMBLEM => npc_flags::TABARDDESIGNER,
        fee_kind::CHARTER => npc_flags::PETITIONER | npc_flags::TABARDDESIGNER,
        _ => return false,
    };
    let Ok((payer, npc)) = crate::items::npc_interaction_gate(
        ctx,
        payer_guid,
        npc_guid,
        required_flags,
        "guild NPC",
        "target does not serve guild fees",
        "pay a guild fee",
    ) else {
        return false;
    };
    npc.npc_flags & required_flags == required_flags
        && !npc.dead
        && npc.health != 0
        && !crate::reputation::npc_refuses_interaction(ctx, &npc, &payer)
}

struct CtxPurse<'a> {
    ctx: &'a ReducerContext,
}

impl CtxPurse<'_> {
    fn purse(&self, payer_guid: u64) -> Option<u32> {
        crate::helpers::acting_entity_by_guid(self.ctx, payer_guid).map(|payer| payer.money)
    }

    fn npc_serves(&self, payer_guid: u64, npc_guid: u64, kind: u8) -> bool {
        npc_serves(self.ctx, payer_guid, npc_guid, kind)
    }

    fn charter_known(&self) -> bool {
        self.ctx
            .db
            .game_item_template()
            .entry()
            .find(GUILD_CHARTER_ENTRY)
            .is_some()
    }

    fn charter_room(&self, payer_guid: u64) -> Result<(), GuildRefusal> {
        let max_count = self
            .ctx
            .db
            .game_item_template()
            .entry()
            .find(GUILD_CHARTER_ENTRY)
            .map_or(0, |template| template.max_count);
        let held = self
            .ctx
            .db
            .game_item_instance()
            .by_owner_guid()
            .filter(&payer_guid)
            .filter(|item| item.entry == GUILD_CHARTER_ENTRY)
            .count();
        if !charter_fits(max_count, held) {
            return Err(GuildRefusal::CharterLimit);
        }
        if !crate::items::has_free_slot(self.ctx, payer_guid) {
            return Err(GuildRefusal::BagsFull);
        }
        Ok(())
    }

    fn create_charter(
        &self,
        payer_guid: u64,
        owner_identity: spacetimedb::Identity,
    ) -> Result<u64, FeeError> {
        let template = self
            .ctx
            .db
            .game_item_template()
            .entry()
            .find(GUILD_CHARTER_ENTRY)
            .ok_or(FeeError::Refused(GuildRefusal::CharterUnavailable))?;
        let guid = crate::items::next_item_guid(self.ctx)
            .map_err(|_| FeeError::Conflict("no item guid left"))?;
        crate::items::store_instance_state(
            self.ctx,
            payer_guid,
            owner_identity,
            &template,
            Some(guid),
            &crate::items::ItemSnapshot {
                entry: GUILD_CHARTER_ENTRY,
                stack_count: 1,
                durability: template.max_durability,
                ..crate::items::ItemSnapshot::default()
            },
        )
        .map_err(|_| FeeError::Refused(GuildRefusal::BagsFull))?;
        Ok(guid)
    }
}

struct CtxLedger<'a> {
    ctx: &'a ReducerContext,
}

impl CtxLedger<'_> {
    fn guild_of(&self, payer_guid: u64) -> Option<(u32, u64)> {
        let guild_id = member(self.ctx, payer_guid)?.guild_id;
        let guild = self.ctx.db.game_guild().guild_id().find(guild_id)?;
        Some((guild_id, guild.leader_guid))
    }

    fn owns_petition(&self, payer_guid: u64) -> bool {
        petition::owns_petition(self.ctx, payer_guid)
    }

    fn guild_name_taken(&self, name: &str) -> bool {
        self.ctx
            .db
            .game_guild()
            .name_key()
            .find(name_key(name))
            .is_some()
    }

    fn apply_emblem(&self, guild_id: u32, emblem: &GuildEmblem) {
        let Some(mut guild) = self.ctx.db.game_guild().guild_id().find(guild_id) else {
            return;
        };
        guild.emblem_style = emblem.emblem_style;
        guild.emblem_color = emblem.emblem_color;
        guild.border_style = emblem.border_style;
        guild.border_color = emblem.border_color;
        guild.background_color = emblem.background_color;
        self.ctx.db.game_guild().guild_id().update(guild);
        push_event(
            self.ctx,
            guild_id,
            0,
            event_kind::TABARD_CHANGED,
            0,
            0,
            Vec::new(),
        );
    }

    fn open_petition(&self, payer_guid: u64, terms: &GuildCharterTerms) -> u32 {
        petition::open(
            self.ctx,
            payer_guid,
            &terms.payer_name,
            terms.payer_team,
            &terms.name,
            terms.charter_item_guid,
        )
    }
}

// Retired legacy Fee Hold tables keep their declarations; see docs/danger-zones.md.
#[table(accessor = game_guild_fee_hold)]
pub struct GuildFeeHold {
    #[primary_key]
    pub payer_guid: u64,
    pub operation_id: u64,
    pub kind: u8,
    pub copper: u32,
    pub npc_guid: u64,
    pub emblem_style: u32,
    pub emblem_color: u32,
    pub border_style: u32,
    pub border_color: u32,
    pub background_color: u32,
    pub charter_item_guid: u64,
    pub charter_name: String,
    pub created_micros: i64,
}
crate::character_owned!(delete, fn sweep_delete_game_guild_fee_hold(ctx, character_guid) {
    ctx.db.game_guild_fee_hold().payer_guid().delete(character_guid);
});
crate::character_owned!(transfer, fn sweep_transfer_game_guild_fee_hold(ctx, character_guid, io) {
    table = game_guild_fee_hold,
    primary_key = payer_guid,
});

#[table(accessor = game_guild_fee_decision)]
pub struct GuildFeeDecision {
    #[primary_key]
    pub operation_id: u64,
    pub payer_guid: u64,
    pub kind: u8,
    pub accepted: bool,
    pub refusal: String,
    pub petition_id: u32,
    pub decided_micros: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unique_charter_fits_only_under_its_max_count() {
        assert!(charter_fits(1, 0));
        assert!(!charter_fits(1, 1));
        assert!(charter_fits(0, 7), "max_count 0 sets no limit");
    }
}
