//! Legacy fees must hold before deciding; a reused decision id can debit twice.

use lyracore_shared::constants::npc_flags;
use lyracore_shared::guild::{
    event_kind, fee_kind, name_key, validate_guild_name, GuildRefusal, CHARTER_COST_COPPER,
    EMBLEM_COST_COPPER, GUILD_CHARTER_ENTRY,
};
use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table};

use super::petition::game_guild_petition;
use super::{game_guild, member, petition, push_event};
use crate::game_world_entity;
use crate::items::{game_item_instance, game_item_template};

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

const REFUND_LETTER_BODY: &str = "Your purse could not hold all of this guild fee refund.";

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

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum GuildFeeTerms {
    Emblem(GuildEmblem),
    Charter(GuildCharterTerms),
}

impl GuildFeeTerms {
    fn kind(&self) -> u8 {
        match self {
            Self::Emblem(_) => fee_kind::EMBLEM,
            Self::Charter(_) => fee_kind::CHARTER,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HeldTerms {
    Emblem(GuildEmblem),
    Charter {
        name: String,
        charter_item_guid: u64,
    },
}

impl HeldTerms {
    fn kind(&self) -> u8 {
        match self {
            Self::Emblem(_) => fee_kind::EMBLEM,
            Self::Charter { .. } => fee_kind::CHARTER,
        }
    }

    fn copper(&self) -> u32 {
        match self {
            Self::Emblem(_) => EMBLEM_COST_COPPER,
            Self::Charter { .. } => CHARTER_COST_COPPER,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HeldFee {
    operation_id: u64,
    payer_guid: u64,
    copper: u32,
    npc_guid: u64,
    terms: HeldTerms,
}

impl HeldFee {
    fn requested(operation_id: u64, payer_guid: u64, request: &GuildFeeRequest) -> Self {
        let (npc_guid, terms) = match request {
            GuildFeeRequest::Emblem(purchase) => {
                (purchase.npc_guid, HeldTerms::Emblem(purchase.emblem))
            }
            GuildFeeRequest::Charter(purchase) => (
                purchase.npc_guid,
                HeldTerms::Charter {
                    name: purchase.name.clone(),
                    charter_item_guid: 0,
                },
            ),
        };
        Self {
            operation_id,
            payer_guid,
            copper: terms.copper(),
            npc_guid,
            terms,
        }
    }

    fn answers(&self, request: &HeldFee) -> bool {
        let mut minted = request.clone();
        if let (
            HeldTerms::Charter {
                charter_item_guid, ..
            },
            HeldTerms::Charter {
                charter_item_guid: held,
                ..
            },
        ) = (&mut minted.terms, &self.terms)
        {
            *charter_item_guid = *held;
        }
        *self == minted
    }

    fn from_row(row: GuildFeeHold) -> Option<Self> {
        let terms = match row.kind {
            fee_kind::EMBLEM => HeldTerms::Emblem(GuildEmblem {
                emblem_style: row.emblem_style,
                emblem_color: row.emblem_color,
                border_style: row.border_style,
                border_color: row.border_color,
                background_color: row.background_color,
            }),
            fee_kind::CHARTER => HeldTerms::Charter {
                name: row.charter_name,
                charter_item_guid: row.charter_item_guid,
            },
            _ => return None,
        };
        Some(Self {
            operation_id: row.operation_id,
            payer_guid: row.payer_guid,
            copper: row.copper,
            npc_guid: row.npc_guid,
            terms,
        })
    }

    fn into_row(self, created_micros: i64) -> GuildFeeHold {
        let kind = self.terms.kind();
        let (emblem, charter_name, charter_item_guid) = match self.terms {
            HeldTerms::Emblem(emblem) => (emblem, String::new(), 0),
            HeldTerms::Charter {
                name,
                charter_item_guid,
            } => (GuildEmblem::default(), name, charter_item_guid),
        };
        GuildFeeHold {
            payer_guid: self.payer_guid,
            operation_id: self.operation_id,
            kind,
            copper: self.copper,
            npc_guid: self.npc_guid,
            emblem_style: emblem.emblem_style,
            emblem_color: emblem.emblem_color,
            border_style: emblem.border_style,
            border_color: emblem.border_color,
            background_color: emblem.background_color,
            charter_item_guid,
            charter_name,
            created_micros,
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

trait FeePurse {
    fn held(&self, payer_guid: u64) -> Option<HeldFee>;
    fn purse(&self, payer_guid: u64) -> Option<u32>;
    fn npc_serves(&self, payer_guid: u64, npc_guid: u64, kind: u8) -> bool;
    fn charter_known(&self) -> bool;
    fn charter_room(&self, payer_guid: u64) -> Result<(), GuildRefusal>;
    fn take(&mut self, hold: HeldFee) -> Result<(), FeeError>;
    fn take_back_charter(&mut self, payer_guid: u64, charter_item_guid: u64) -> bool;
    fn release(&mut self, hold: &HeldFee, credit: u32, mailed: u32) -> Result<(), FeeError>;
}

trait FeeLedger {
    fn decided_for(&self, operation_id: u64) -> Option<(u64, u8)>;
    fn guild_of(&self, payer_guid: u64) -> Option<(u32, u64)>;
    fn owns_petition(&self, payer_guid: u64) -> bool;
    fn guild_name_taken(&self, name: &str) -> bool;
    fn apply_emblem(&mut self, guild_id: u32, emblem: &GuildEmblem);
    fn open_petition(&mut self, payer_guid: u64, terms: &GuildCharterTerms) -> u32;
    fn record(
        &mut self,
        operation_id: u64,
        payer_guid: u64,
        kind: u8,
        refusal: Option<GuildRefusal>,
        petition_id: u32,
    );
}

fn hold_fee<P: FeePurse>(purse: &mut P, hold: HeldFee) -> Result<(), FeeError> {
    if hold.operation_id == 0 {
        return Err(FeeError::Conflict("operation id 0"));
    }
    if let Some(existing) = purse.held(hold.payer_guid) {
        return if existing.answers(&hold) {
            Ok(())
        } else {
            Err(FeeError::Conflict("another Fee Hold is pending"))
        };
    }
    payment_gate(purse, hold.payer_guid, hold.npc_guid, &hold.terms)?;
    purse.take(hold)
}

fn payment_gate<P: FeePurse>(
    purse: &P,
    payer_guid: u64,
    npc_guid: u64,
    terms: &HeldTerms,
) -> Result<(), FeeError> {
    let charter = matches!(terms, HeldTerms::Charter { .. });
    if !purse.npc_serves(payer_guid, npc_guid, terms.kind()) {
        return Err(FeeError::Refused(GuildRefusal::NpcRefused));
    }
    if charter && !purse.charter_known() {
        return Err(FeeError::Refused(GuildRefusal::CharterUnavailable));
    }
    if purse
        .purse(payer_guid)
        .is_none_or(|copper| copper < terms.copper())
    {
        return Err(FeeError::Refused(GuildRefusal::NotEnoughMoney));
    }
    if charter {
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
        if purse.held(payer_guid).is_some() {
            return Err(FeeError::Conflict(
                "a legacy Fee Hold must settle before a native purchase",
            ));
        }
        let mut ledger = CtxLedger { ctx };
        match request {
            GuildFeeRequest::Emblem(request) => {
                let guild_id = emblem_gate(&ledger, payer_guid).map_err(FeeError::Refused)?;
                payment_gate(
                    &purse,
                    payer_guid,
                    request.npc_guid,
                    &HeldTerms::Emblem(request.emblem),
                )?;
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
                payment_gate(
                    &purse,
                    payer_guid,
                    request.npc_guid,
                    &HeldTerms::Charter {
                        name: request.name,
                        charter_item_guid: 0,
                    },
                )?;
                let identity = debit(ctx, payer_guid, CHARTER_COST_COPPER)?;
                charter.charter_item_guid = purse.create_charter(payer_guid, identity)?;
                ledger.open_petition(payer_guid, &charter);
            }
        }
        Ok(())
    };
    apply().map_err(FeeError::into_text)
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

fn decide_fee<L: FeeLedger>(
    ledger: &mut L,
    operation_id: u64,
    payer_guid: u64,
    terms: &GuildFeeTerms,
) -> Result<(), FeeError> {
    if operation_id == 0 {
        return Err(FeeError::Conflict("operation id 0"));
    }
    if let Some(decided_for) = ledger.decided_for(operation_id) {
        return if decided_for == (payer_guid, terms.kind()) {
            Ok(())
        } else {
            Err(FeeError::Conflict("operation id reused"))
        };
    }
    let verdict = match terms {
        GuildFeeTerms::Emblem(emblem) => emblem_gate(ledger, payer_guid).map(|guild_id| {
            ledger.apply_emblem(guild_id, emblem);
            0
        }),
        GuildFeeTerms::Charter(charter) => {
            if charter.charter_item_guid == 0 {
                return Err(FeeError::Conflict("Guild Charter guid 0"));
            }
            charter_gate(ledger, payer_guid, charter)
                .map(|()| ledger.open_petition(payer_guid, charter))
        }
    };
    let (refusal, petition_id) = match verdict {
        Ok(petition_id) => (None, petition_id),
        Err(refusal) => (Some(refusal), 0),
    };
    ledger.record(operation_id, payer_guid, terms.kind(), refusal, petition_id);
    Ok(())
}

fn emblem_gate<L: FeeLedger>(ledger: &L, payer_guid: u64) -> Result<u32, GuildRefusal> {
    let (guild_id, leader_guid) = ledger
        .guild_of(payer_guid)
        .ok_or(GuildRefusal::NotInGuild)?;
    if leader_guid != payer_guid {
        return Err(GuildRefusal::NotLeader);
    }
    Ok(guild_id)
}

fn charter_gate<L: FeeLedger>(
    ledger: &L,
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

// An absent Character keeps its Fee Hold for the Transfer destination.
fn finish_fee<P: FeePurse>(
    purse: &mut P,
    operation_id: u64,
    payer_guid: u64,
    accepted: bool,
) -> Result<(), FeeError> {
    let Some(hold) = purse
        .held(payer_guid)
        .filter(|hold| hold.operation_id == operation_id)
    else {
        return Ok(());
    };
    let Some(copper) = purse.purse(payer_guid) else {
        return Err(FeeError::Conflict("payer is not in the world here"));
    };
    let refund = match &hold.terms {
        _ if accepted => 0,
        HeldTerms::Emblem(_) => hold.copper,
        HeldTerms::Charter {
            charter_item_guid, ..
        } => {
            if purse.take_back_charter(payer_guid, *charter_item_guid) {
                hold.copper
            } else {
                0
            }
        }
    };
    let (_, mailed) = crate::mail::split_refund(copper, refund);
    purse.release(&hold, refund - mailed, mailed)
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

impl FeePurse for CtxPurse<'_> {
    fn held(&self, payer_guid: u64) -> Option<HeldFee> {
        self.ctx
            .db
            .game_guild_fee_hold()
            .payer_guid()
            .find(payer_guid)
            .and_then(HeldFee::from_row)
    }

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

    fn take(&mut self, mut hold: HeldFee) -> Result<(), FeeError> {
        let owner_identity = debit(self.ctx, hold.payer_guid, hold.copper)?;
        if let HeldTerms::Charter {
            charter_item_guid, ..
        } = &mut hold.terms
        {
            *charter_item_guid = self.create_charter(hold.payer_guid, owner_identity)?;
        }
        let created_micros = self.ctx.timestamp.to_micros_since_unix_epoch();
        self.ctx
            .db
            .game_guild_fee_hold()
            .insert(hold.into_row(created_micros));
        Ok(())
    }

    fn take_back_charter(&mut self, payer_guid: u64, charter_item_guid: u64) -> bool {
        let items = self.ctx.db.game_item_instance();
        let held = items
            .guid()
            .find(charter_item_guid)
            .is_some_and(|item| item.owner_guid == payer_guid && item.entry == GUILD_CHARTER_ENTRY);
        if held {
            crate::ItemInstance::delete(self.ctx, charter_item_guid);
        }
        held
    }

    fn release(&mut self, hold: &HeldFee, credit: u32, mailed: u32) -> Result<(), FeeError> {
        if credit != 0 {
            let mut payer = crate::helpers::acting_entity_by_guid(self.ctx, hold.payer_guid)
                .ok_or(FeeError::Conflict("payer is not in the world here"))?;
            payer.money = payer
                .money
                .checked_add(credit)
                .ok_or(FeeError::Conflict("refund does not fit in the purse"))?;
            payer.update(self.ctx);
        }
        if mailed != 0 {
            self.mail_refund(hold, mailed)?;
        }
        self.ctx
            .db
            .game_guild_fee_hold()
            .payer_guid()
            .delete(hold.payer_guid);
        Ok(())
    }
}

impl CtxPurse<'_> {
    fn mail_refund(&self, hold: &HeldFee, mailed: u32) -> Result<(), FeeError> {
        let npc_entry = self
            .ctx
            .db
            .game_world_entity()
            .guid()
            .find(hold.npc_guid)
            .map_or(0, |npc| npc.entry);
        let escrow_id = *crate::auth::reserve_guids(self.ctx, 1, u64::MAX)
            .map_err(|_| FeeError::Conflict("no mail escrow id left"))?
            .start();
        let letter = crate::mail_reward::RewardLetter {
            header: lyracore_shared::mail::RewardHeader {
                giver: lyracore_shared::mail::QuestGiver::Creature(npc_entry),
                mail_template_id: 0,
            },
            body: REFUND_LETTER_BODY.to_string(),
            money: mailed,
            item: crate::items::ItemSnapshot::default(),
            delay_secs: 0,
        };
        crate::mail_escrow::file_reward(self.ctx, escrow_id, hold.payer_guid, &letter)
            .map_err(|_| FeeError::Conflict("refund letter not filed"))
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

impl FeeLedger for CtxLedger<'_> {
    fn decided_for(&self, operation_id: u64) -> Option<(u64, u8)> {
        self.ctx
            .db
            .game_guild_fee_decision()
            .operation_id()
            .find(operation_id)
            .map(|decision| (decision.payer_guid, decision.kind))
    }

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

    fn apply_emblem(&mut self, guild_id: u32, emblem: &GuildEmblem) {
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

    fn open_petition(&mut self, payer_guid: u64, terms: &GuildCharterTerms) -> u32 {
        petition::open(
            self.ctx,
            payer_guid,
            &terms.payer_name,
            terms.payer_team,
            &terms.name,
            terms.charter_item_guid,
        )
    }

    fn record(
        &mut self,
        operation_id: u64,
        payer_guid: u64,
        kind: u8,
        refusal: Option<GuildRefusal>,
        petition_id: u32,
    ) {
        self.ctx
            .db
            .game_guild_fee_decision()
            .insert(GuildFeeDecision {
                operation_id,
                payer_guid,
                kind,
                accepted: refusal.is_none(),
                refusal: refusal.map_or_else(String::new, |refusal| refusal.as_tag().to_string()),
                petition_id,
                decided_micros: self.ctx.timestamp.to_micros_since_unix_epoch(),
            });
    }
}

#[reducer]
pub fn gw_guild_fee_hold(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::account_ownership::SessionActor,
    request: GuildFeeRequest,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let payer_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    hold_fee(
        &mut CtxPurse { ctx },
        HeldFee::requested(operation_id, payer_guid, &request),
    )
    .map_err(FeeError::into_text)
}

#[reducer]
pub fn realm_guild_fee_decide(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::account_ownership::SessionActor,
    terms: GuildFeeTerms,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let payer_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    decide_fee(&mut CtxLedger { ctx }, operation_id, payer_guid, &terms)
        .map_err(FeeError::into_text)
}

#[reducer]
pub fn gw_guild_fee_finish(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::account_ownership::SessionActor,
    accepted: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let payer_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    finish_fee(&mut CtxPurse { ctx }, operation_id, payer_guid, accepted)
        .map_err(FeeError::into_text)
}

// Character deletion must not destroy copper held for a guild operation.
pub(crate) fn character_has_fee_hold(ctx: &ReducerContext, character_guid: u64) -> bool {
    ctx.db
        .game_guild_fee_hold()
        .payer_guid()
        .find(character_guid)
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const LEADER: u64 = 5_090_401;
    const MEMBER: u64 = 5_090_402;
    const DESIGNER: u64 = 5_090_410;
    const GUILD: u32 = 7;

    const EMBLEM: GuildEmblem = GuildEmblem {
        emblem_style: 11,
        emblem_color: 12,
        border_style: 3,
        border_color: 14,
        background_color: 15,
    };

    fn purchase() -> GuildFeeRequest {
        GuildFeeRequest::Emblem(GuildEmblemPurchase {
            npc_guid: DESIGNER,
            emblem: EMBLEM,
        })
    }

    #[derive(Default)]
    struct Shard {
        purses: BTreeMap<u64, u32>,
        mailed: BTreeMap<u64, u32>,
        serving_npcs: Vec<u64>,
        holds: BTreeMap<u64, HeldFee>,
        charters: BTreeMap<u64, u64>,
        charter_unknown: bool,
        charter_room: Option<GuildRefusal>,
    }

    impl FeePurse for Shard {
        fn held(&self, payer_guid: u64) -> Option<HeldFee> {
            self.holds.get(&payer_guid).cloned()
        }

        fn purse(&self, payer_guid: u64) -> Option<u32> {
            self.purses.get(&payer_guid).copied()
        }

        fn npc_serves(&self, _payer_guid: u64, npc_guid: u64, _kind: u8) -> bool {
            self.serving_npcs.contains(&npc_guid)
        }

        fn charter_known(&self) -> bool {
            !self.charter_unknown
        }

        fn charter_room(&self, _payer_guid: u64) -> Result<(), GuildRefusal> {
            self.charter_room.map_or(Ok(()), Err)
        }

        fn take(&mut self, mut hold: HeldFee) -> Result<(), FeeError> {
            let purse = self
                .purses
                .get_mut(&hold.payer_guid)
                .ok_or(FeeError::Refused(GuildRefusal::NotEnoughMoney))?;
            *purse = purse
                .checked_sub(hold.copper)
                .ok_or(FeeError::Refused(GuildRefusal::NotEnoughMoney))?;
            if let HeldTerms::Charter {
                charter_item_guid, ..
            } = &mut hold.terms
            {
                *charter_item_guid = CHARTER_GUID_BASE + self.charters.len() as u64;
                self.charters.insert(*charter_item_guid, hold.payer_guid);
            }
            self.holds.insert(hold.payer_guid, hold);
            Ok(())
        }

        fn take_back_charter(&mut self, payer_guid: u64, charter_item_guid: u64) -> bool {
            if self.charters.get(&charter_item_guid) == Some(&payer_guid) {
                self.charters.remove(&charter_item_guid);
                return true;
            }
            false
        }

        fn release(&mut self, hold: &HeldFee, credit: u32, mailed: u32) -> Result<(), FeeError> {
            let purse = self
                .purses
                .get_mut(&hold.payer_guid)
                .ok_or(FeeError::Conflict("payer is not in the world here"))?;
            *purse = purse
                .checked_add(credit)
                .ok_or(FeeError::Conflict("refund does not fit in the purse"))?;
            *self.mailed.entry(hold.payer_guid).or_default() += mailed;
            self.holds.remove(&hold.payer_guid);
            Ok(())
        }
    }

    #[derive(Default)]
    struct Realm {
        members: BTreeMap<u64, u32>,
        leaders: BTreeMap<u32, u64>,
        guild_names: Vec<String>,
        emblems: BTreeMap<u32, GuildEmblem>,
        petitions: BTreeMap<u64, (u32, GuildCharterTerms)>,
        decisions: BTreeMap<u64, (u64, u8, Option<GuildRefusal>)>,
        decided_petitions: BTreeMap<u64, u32>,
        tabard_events: usize,
    }

    impl FeeLedger for Realm {
        fn decided_for(&self, operation_id: u64) -> Option<(u64, u8)> {
            self.decisions
                .get(&operation_id)
                .map(|(payer, kind, _)| (*payer, *kind))
        }

        fn guild_of(&self, payer_guid: u64) -> Option<(u32, u64)> {
            let guild_id = *self.members.get(&payer_guid)?;
            Some((guild_id, *self.leaders.get(&guild_id)?))
        }

        fn owns_petition(&self, payer_guid: u64) -> bool {
            self.petitions.contains_key(&payer_guid)
        }

        fn guild_name_taken(&self, name: &str) -> bool {
            self.guild_names
                .iter()
                .any(|taken| name_key(taken) == name_key(name))
        }

        fn apply_emblem(&mut self, guild_id: u32, emblem: &GuildEmblem) {
            self.emblems.insert(guild_id, *emblem);
            self.tabard_events += 1;
        }

        fn open_petition(&mut self, payer_guid: u64, terms: &GuildCharterTerms) -> u32 {
            let petition_id = 40 + self.petitions.len() as u32;
            self.petitions
                .insert(payer_guid, (petition_id, terms.clone()));
            petition_id
        }

        fn record(
            &mut self,
            operation_id: u64,
            payer_guid: u64,
            kind: u8,
            refusal: Option<GuildRefusal>,
            petition_id: u32,
        ) {
            self.decisions
                .insert(operation_id, (payer_guid, kind, refusal));
            self.decided_petitions.insert(operation_id, petition_id);
        }
    }

    fn shard(purse: u32) -> Shard {
        Shard {
            purses: BTreeMap::from([(LEADER, purse), (MEMBER, purse)]),
            serving_npcs: vec![DESIGNER],
            ..Shard::default()
        }
    }

    fn realm() -> Realm {
        Realm {
            members: BTreeMap::from([(LEADER, GUILD), (MEMBER, GUILD)]),
            leaders: BTreeMap::from([(GUILD, LEADER)]),
            ..Realm::default()
        }
    }

    fn hold(shard: &mut Shard, operation_id: u64, payer_guid: u64) -> Result<(), FeeError> {
        hold_fee(
            shard,
            HeldFee::requested(operation_id, payer_guid, &purchase()),
        )
    }

    fn decide(realm: &mut Realm, operation_id: u64, payer_guid: u64) -> Result<(), FeeError> {
        decide_fee(
            realm,
            operation_id,
            payer_guid,
            &GuildFeeTerms::Emblem(EMBLEM),
        )
    }

    fn accepted(realm: &Realm, operation_id: u64) -> bool {
        matches!(realm.decisions.get(&operation_id), Some((_, _, None)))
    }

    fn pay(shard: &mut Shard, realm: &mut Realm, operation_id: u64, payer_guid: u64) {
        hold(shard, operation_id, payer_guid).unwrap();
        decide(realm, operation_id, payer_guid).unwrap();
        let accepted = accepted(realm, operation_id);
        finish_fee(shard, operation_id, payer_guid, accepted).unwrap();
    }

    #[test]
    fn the_guild_leader_buys_an_emblem_for_ten_gold() {
        let (mut shard, mut realm) = (shard(150_000), realm());
        pay(&mut shard, &mut realm, 1, LEADER);
        assert_eq!(shard.purses[&LEADER], 50_000);
        assert!(shard.holds.is_empty());
        assert_eq!(realm.emblems[&GUILD], EMBLEM);
        assert_eq!(realm.tabard_events, 1);
    }

    #[test]
    fn a_member_who_is_not_the_leader_is_refunded_every_copper() {
        let (mut shard, mut realm) = (shard(150_000), realm());
        pay(&mut shard, &mut realm, 1, MEMBER);
        assert_eq!(
            realm.decisions[&1],
            (MEMBER, fee_kind::EMBLEM, Some(GuildRefusal::NotLeader))
        );
        assert_eq!(shard.purses[&MEMBER], 150_000);
        assert!(shard.holds.is_empty());
        assert!(realm.emblems.is_empty());
    }

    #[test]
    fn a_refund_the_purse_cannot_hold_sends_the_rest_by_mail() {
        let (mut shard, mut realm) = (shard(150_000), realm());
        hold(&mut shard, 1, MEMBER).unwrap();
        shard.purses.insert(MEMBER, 4_294_937_295);
        decide(&mut realm, 1, MEMBER).unwrap();
        finish_fee(&mut shard, 1, MEMBER, accepted(&realm, 1)).unwrap();
        assert_eq!(shard.purses[&MEMBER], 4_294_967_295);
        assert_eq!(shard.mailed[&MEMBER], 70_000);
        assert!(shard.holds.is_empty());
    }

    #[test]
    fn leadership_that_moves_after_the_hold_refunds_the_old_leader() {
        let (mut shard, mut realm) = (shard(150_000), realm());
        hold(&mut shard, 1, LEADER).unwrap();
        realm.leaders.insert(GUILD, MEMBER);
        decide(&mut realm, 1, LEADER).unwrap();
        finish_fee(&mut shard, 1, LEADER, accepted(&realm, 1)).unwrap();
        assert_eq!(shard.purses[&LEADER], 150_000);
        assert!(realm.emblems.is_empty());
    }

    #[test]
    fn a_guild_that_disbands_after_the_hold_refunds_the_payer() {
        let (mut shard, mut realm) = (shard(150_000), realm());
        hold(&mut shard, 1, LEADER).unwrap();
        realm.members.clear();
        realm.leaders.clear();
        decide(&mut realm, 1, LEADER).unwrap();
        assert_eq!(
            realm.decisions[&1].2,
            Some(GuildRefusal::NotInGuild),
            "the vanished Guild is a refusal the finish can refund, not an error that strands the Hold"
        );
        finish_fee(&mut shard, 1, LEADER, accepted(&realm, 1)).unwrap();
        assert_eq!(shard.purses[&LEADER], 150_000);
        assert!(shard.holds.is_empty());
    }

    #[test]
    fn hold_refusals_change_nothing() {
        let mut broke = shard(99_999);
        assert_eq!(
            hold(&mut broke, 1, LEADER),
            Err(FeeError::Refused(GuildRefusal::NotEnoughMoney))
        );
        assert_eq!(broke.purses[&LEADER], 99_999);
        assert!(broke.holds.is_empty());

        let mut elsewhere = shard(150_000);
        elsewhere.serving_npcs.clear();
        assert_eq!(
            hold(&mut elsewhere, 1, LEADER),
            Err(FeeError::Refused(GuildRefusal::NpcRefused))
        );
        assert_eq!(elsewhere.purses[&LEADER], 150_000);
        assert!(elsewhere.holds.is_empty());

        let mut offline = shard(150_000);
        offline.purses.clear();
        assert_eq!(
            hold(&mut offline, 1, LEADER),
            Err(FeeError::Refused(GuildRefusal::NotEnoughMoney))
        );
        assert!(offline.holds.is_empty());
    }

    #[test]
    fn a_replayed_hold_takes_the_fee_once() {
        let mut shard = shard(250_000);
        hold(&mut shard, 1, LEADER).unwrap();
        hold(&mut shard, 1, LEADER).unwrap();
        assert_eq!(shard.purses[&LEADER], 150_000);
        assert_eq!(
            hold(&mut shard, 2, LEADER),
            Err(FeeError::Conflict("another Fee Hold is pending")),
            "a Character has one Fee Hold at a time"
        );
        assert_eq!(shard.purses[&LEADER], 150_000);
    }

    #[test]
    fn a_replayed_decision_applies_its_effect_once() {
        let mut realm = realm();
        decide(&mut realm, 1, LEADER).unwrap();
        realm.leaders.insert(GUILD, MEMBER);
        decide(&mut realm, 1, LEADER).unwrap();
        assert!(accepted(&realm, 1), "a retry reads the first decision back");
        assert_eq!(realm.tabard_events, 1);
        assert_eq!(
            decide(&mut realm, 1, MEMBER),
            Err(FeeError::Conflict("operation id reused"))
        );
    }

    #[test]
    fn a_replayed_finish_refunds_once() {
        let (mut shard, mut realm) = (shard(150_000), realm());
        hold(&mut shard, 1, MEMBER).unwrap();
        decide(&mut realm, 1, MEMBER).unwrap();
        finish_fee(&mut shard, 1, MEMBER, false).unwrap();
        finish_fee(&mut shard, 1, MEMBER, false).unwrap();
        assert_eq!(shard.purses[&MEMBER], 150_000);
    }

    #[test]
    fn a_stale_finish_leaves_a_newer_hold_alone() {
        let (mut shard, mut realm) = (shard(250_000), realm());
        pay(&mut shard, &mut realm, 1, MEMBER);
        hold(&mut shard, 2, MEMBER).unwrap();
        finish_fee(&mut shard, 1, MEMBER, false).unwrap();
        assert_eq!(shard.purses[&MEMBER], 150_000);
        assert_eq!(shard.holds[&MEMBER].operation_id, 2);
    }

    #[test]
    fn a_decision_made_before_the_hold_spends_the_fee_once() {
        let (mut shard, mut realm) = (shard(250_000), realm());
        decide(&mut realm, 1, LEADER).unwrap();
        finish_fee(&mut shard, 1, LEADER, true).unwrap();
        assert_eq!(
            shard.purses[&LEADER], 250_000,
            "a finish without its Hold spends nothing"
        );
        hold(&mut shard, 1, LEADER).unwrap();
        finish_fee(&mut shard, 1, LEADER, true).unwrap();
        finish_fee(&mut shard, 1, LEADER, true).unwrap();
        assert_eq!(shard.purses[&LEADER], 150_000);
        assert!(shard.holds.is_empty());
    }

    #[test]
    fn a_payer_out_of_the_world_keeps_the_hold_until_it_returns() {
        let (mut shard, mut realm) = (shard(150_000), realm());
        hold(&mut shard, 1, MEMBER).unwrap();
        decide(&mut realm, 1, MEMBER).unwrap();
        shard.purses.remove(&MEMBER);
        assert!(finish_fee(&mut shard, 1, MEMBER, false).is_err());
        assert_eq!(shard.holds[&MEMBER].copper, EMBLEM_COST_COPPER);
        shard.purses.insert(MEMBER, 50_000);
        finish_fee(&mut shard, 1, MEMBER, false).unwrap();
        assert_eq!(shard.purses[&MEMBER], 150_000);
        assert!(shard.holds.is_empty());
    }

    const PETITIONER: u64 = 5_090_411;
    const OUTSIDER: u64 = 5_090_403;
    const CHARTER_GUID_BASE: u64 = 5_090_420;

    fn charter_purchase(name: &str) -> GuildFeeRequest {
        GuildFeeRequest::Charter(GuildCharterPurchase {
            npc_guid: PETITIONER,
            name: name.into(),
        })
    }

    fn guild_master_shard(purse: u32) -> Shard {
        Shard {
            purses: BTreeMap::from([(OUTSIDER, purse), (MEMBER, purse)]),
            serving_npcs: vec![PETITIONER],
            ..Shard::default()
        }
    }

    fn hold_charter(shard: &mut Shard, operation_id: u64, name: &str) -> Result<(), FeeError> {
        hold_fee(
            shard,
            HeldFee::requested(operation_id, OUTSIDER, &charter_purchase(name)),
        )
    }

    fn decide_charter(
        shard: &Shard,
        realm: &mut Realm,
        operation_id: u64,
        payer_guid: u64,
    ) -> Result<(), FeeError> {
        let HeldTerms::Charter {
            name,
            charter_item_guid,
        } = shard.holds[&payer_guid].terms.clone()
        else {
            panic!("not a Charter Hold");
        };
        decide_fee(
            realm,
            operation_id,
            payer_guid,
            &GuildFeeTerms::Charter(GuildCharterTerms {
                charter_item_guid,
                name,
                payer_name: "Founder".into(),
                payer_team: 469,
            }),
        )
    }

    #[test]
    fn a_guild_charter_costs_ten_silver_and_opens_a_petition() {
        let (mut shard, mut realm) = (guild_master_shard(1_500), realm());
        hold_charter(&mut shard, 1, "Night Watch").unwrap();
        assert_eq!(shard.purses[&OUTSIDER], 500);
        assert_eq!(shard.charters, BTreeMap::from([(5_090_420, OUTSIDER)]));
        decide_charter(&shard, &mut realm, 1, OUTSIDER).unwrap();
        finish_fee(&mut shard, 1, OUTSIDER, accepted(&realm, 1)).unwrap();

        assert_eq!(shard.purses[&OUTSIDER], 500);
        assert!(shard.holds.is_empty());
        assert_eq!(shard.charters, BTreeMap::from([(5_090_420, OUTSIDER)]));
        assert_eq!(
            realm.petitions[&OUTSIDER],
            (
                40,
                GuildCharterTerms {
                    charter_item_guid: 5_090_420,
                    name: "Night Watch".into(),
                    payer_name: "Founder".into(),
                    payer_team: 469,
                }
            )
        );
        assert_eq!(realm.decided_petitions[&1], 40);
    }

    #[test]
    fn charter_hold_gates_run_in_mangos_order_and_change_nothing() {
        let cases = [
            (
                Shard {
                    serving_npcs: Vec::new(),
                    charter_unknown: true,
                    ..guild_master_shard(999)
                },
                GuildRefusal::NpcRefused,
            ),
            (
                Shard {
                    charter_unknown: true,
                    charter_room: Some(GuildRefusal::BagsFull),
                    ..guild_master_shard(999)
                },
                GuildRefusal::CharterUnavailable,
            ),
            (
                Shard {
                    charter_room: Some(GuildRefusal::CharterLimit),
                    ..guild_master_shard(999)
                },
                GuildRefusal::NotEnoughMoney,
            ),
            (
                Shard {
                    charter_room: Some(GuildRefusal::CharterLimit),
                    ..guild_master_shard(1_000)
                },
                GuildRefusal::CharterLimit,
            ),
            (
                Shard {
                    charter_room: Some(GuildRefusal::BagsFull),
                    ..guild_master_shard(1_000)
                },
                GuildRefusal::BagsFull,
            ),
        ];
        for (mut shard, refusal) in cases {
            let purse = shard.purses[&OUTSIDER];
            assert_eq!(
                hold_charter(&mut shard, 1, "Night Watch"),
                Err(FeeError::Refused(refusal))
            );
            assert_eq!(shard.purses[&OUTSIDER], purse, "{refusal:?}");
            assert!(shard.holds.is_empty() && shard.charters.is_empty());
        }
    }

    #[test]
    fn a_unique_charter_fits_only_under_its_max_count() {
        assert!(charter_fits(1, 0));
        assert!(!charter_fits(1, 1));
        assert!(charter_fits(0, 7), "max_count 0 sets no limit");
    }

    #[test]
    fn a_replayed_charter_hold_creates_one_charter() {
        let mut shard = guild_master_shard(2_500);
        hold_charter(&mut shard, 1, "Night Watch").unwrap();
        hold_charter(&mut shard, 1, "Night Watch").unwrap();
        assert_eq!(shard.purses[&OUTSIDER], 1_500);
        assert_eq!(shard.charters.len(), 1);
        assert_eq!(
            hold_charter(&mut shard, 1, "Day Watch"),
            Err(FeeError::Conflict("another Fee Hold is pending"))
        );
    }

    #[test]
    fn charter_decision_gates_run_in_order() {
        let cases = [
            (MEMBER, "Night Watch", GuildRefusal::AlreadyInGuild),
            (OUTSIDER, "Night!Watch", GuildRefusal::AlreadyHasPetition),
            (5_090_404, "Night!Watch", GuildRefusal::NameInvalid),
            (5_090_404, "KNIGHTS", GuildRefusal::NameExists),
        ];
        for (payer, name, refusal) in cases {
            let mut realm = Realm {
                guild_names: vec!["Knights".into()],
                ..realm()
            };
            realm.petitions.insert(
                OUTSIDER,
                (
                    39,
                    GuildCharterTerms {
                        charter_item_guid: 5_090_429,
                        name: "Old".into(),
                        payer_name: "Founder".into(),
                        payer_team: 469,
                    },
                ),
            );
            let terms = GuildFeeTerms::Charter(GuildCharterTerms {
                charter_item_guid: 5_090_420,
                name: name.into(),
                payer_name: "Payer".into(),
                payer_team: 469,
            });
            decide_fee(&mut realm, 1, payer, &terms).unwrap();
            assert_eq!(
                realm.decisions[&1],
                (payer, fee_kind::CHARTER, Some(refusal)),
                "{name}"
            );
            assert_eq!(realm.petitions.len(), 1, "a refusal opens no Petition");
        }
    }

    #[test]
    fn a_refused_charter_is_destroyed_and_refunded() {
        let (mut shard, mut realm) = (guild_master_shard(1_500), realm());
        realm.guild_names.push("Night Watch".into());
        hold_charter(&mut shard, 1, "Night Watch").unwrap();
        decide_charter(&shard, &mut realm, 1, OUTSIDER).unwrap();
        finish_fee(&mut shard, 1, OUTSIDER, accepted(&realm, 1)).unwrap();
        assert_eq!(shard.purses[&OUTSIDER], 1_500);
        assert!(shard.charters.is_empty() && shard.holds.is_empty());
        assert!(realm.petitions.is_empty());
    }

    #[test]
    fn a_charter_that_left_the_payer_before_a_refusal_is_not_refunded() {
        let (mut shard, mut realm) = (guild_master_shard(1_500), realm());
        realm.guild_names.push("Night Watch".into());
        hold_charter(&mut shard, 1, "Night Watch").unwrap();
        decide_charter(&shard, &mut realm, 1, OUTSIDER).unwrap();
        shard.charters.insert(5_090_420, MEMBER);
        finish_fee(&mut shard, 1, OUTSIDER, accepted(&realm, 1)).unwrap();
        assert_eq!(
            shard.purses[&OUTSIDER], 500,
            "the Charter's new holder keeps it, so the copper stays spent"
        );
        assert_eq!(shard.charters, BTreeMap::from([(5_090_420, MEMBER)]));
        assert!(shard.holds.is_empty());
    }

    #[test]
    fn a_charter_decision_without_its_charter_guid_decides_nothing() {
        let mut realm = realm();
        let terms = GuildFeeTerms::Charter(GuildCharterTerms {
            charter_item_guid: 0,
            name: "Night Watch".into(),
            payer_name: "Founder".into(),
            payer_team: 469,
        });
        assert_eq!(
            decide_fee(&mut realm, 1, OUTSIDER, &terms),
            Err(FeeError::Conflict("Guild Charter guid 0"))
        );
        assert!(realm.decisions.is_empty());
    }

    #[test]
    fn a_charter_hold_row_round_trips_its_terms() {
        let mut hold = HeldFee::requested(9, OUTSIDER, &charter_purchase("Night Watch"));
        assert_eq!(hold.copper, 1_000);
        if let HeldTerms::Charter {
            charter_item_guid, ..
        } = &mut hold.terms
        {
            *charter_item_guid = 5_090_420;
        }
        let row = hold.clone().into_row(0);
        assert_eq!(
            (row.kind, row.charter_item_guid, row.charter_name.as_str()),
            (2, 5_090_420, "Night Watch")
        );
        assert_eq!(HeldFee::from_row(row), Some(hold));
    }

    #[test]
    fn a_hold_row_round_trips_its_terms() {
        let hold = HeldFee::requested(9, LEADER, &purchase());
        assert_eq!(hold.copper, 100_000);
        assert_eq!(HeldFee::from_row(hold.clone().into_row(0)), Some(hold));
    }
}
