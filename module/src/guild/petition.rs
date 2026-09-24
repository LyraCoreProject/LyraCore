//! Petitions: how players found a Guild. A Character buys a Guild Charter through the Fee Hold
//! ([`super::fee`]), which opens a Petition here on Realm-core. Nine Characters on nine Realm
//! Accounts sign it, and the owner turns it in to found the Guild. The Gates and their order are
//! cmangos-classic's (`cm:PetitionsHandler.cpp`); the cores here are what
//! [`super::realm_guild_op`] dispatches to.
//!
//! The Charter item lives on the owner's Home Shard and the Petition on Realm-core. They share only
//! the Charter's item guid. The Charter binds when picked up, so trade and mail refuse it, and a
//! Transfer keeps its guid. A Petition loses its Charter only when the owner destroys the Charter;
//! nobody can turn that Petition in, and the owner's next Charter purchase closes it first. A
//! Charter loses its Petition when the owner joins a Guild; the Charter then does nothing.

use lyracore_shared::guild::{
    event_kind, name_key, petition_signature_key, validate_guild_name, GuildRefusal,
    GUILD_CHARTER_ENTRY, MAX_PETITION_SIGNATURES, MIN_PETITION_SIGNATURES,
};
use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table};

use super::{add_member, create_guild, game_guild, lowest_rank, member, push_event};
use crate::items::game_item_instance;

/// One open Petition: a proposed Guild, its owner and the Guild Charter that stands for it. An
/// owner has at most one.
#[table(accessor = game_guild_petition)]
pub struct GuildPetition {
    #[primary_key]
    #[auto_inc]
    pub petition_id: u32,
    #[unique]
    pub charter_item_guid: u64,
    #[unique]
    pub owner_guid: u64,
    /// The owner's name snapshot, for the Guild Leader's member row at founding.
    pub owner_name: String,
    /// `lyracore_shared::faction::TEAM_*` of the owner. Only the same team signs.
    pub team: u32,
    pub name: String,
    pub created_micros: i64,
}

/// One Signature on one Petition. At most one per Realm Account on a Petition.
#[table(
    accessor = game_guild_petition_signature,
    index(accessor = by_petition, btree(columns = [petition_id])),
    index(accessor = by_signer, btree(columns = [signer_guid]))
)]
pub struct GuildPetitionSignature {
    /// `lyracore_shared::guild::petition_signature_key(petition_id, slot)`, so the Gateway reads a
    /// Petition's Signatures by key.
    #[primary_key]
    pub signature_key: u64,
    pub petition_id: u32,
    pub signer_guid: u64,
    /// The signer's name snapshot, for its member row at founding.
    pub signer_name: String,
    /// The signer's Realm Account, never 0: a signer without a known Account is refused.
    pub signer_realm_account: u64,
    pub signed_micros: i64,
}

/// A Character signs the Petition of `charter_item_guid`. Realm-core holds no row for a
/// non-member, so the Gateway conveys the signer's name and team.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildPetitionSign {
    pub charter_item_guid: u64,
    pub actor_name: String,
    pub actor_team: u32,
}

/// The owner offers its Petition to a live Character, who then sees the signature window. The
/// Gateway resolves the target's team from its race.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildPetitionOffer {
    pub charter_item_guid: u64,
    pub target_guid: u64,
    pub target_team: u32,
}

/// The owner renames its Petition.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildPetitionRename {
    pub charter_item_guid: u64,
    pub name: String,
}

/// One Signature as the Gates read it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SignatureFacts {
    signer_guid: u64,
    signer_realm_account: u64,
}

/// What a signing attempt that passed the Gates does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SignVerdict {
    Sign,
    /// The signer's Realm Account already signed. Nothing is written, but both sides hear it
    /// (`cm:PetitionsHandler.cpp:380-396`).
    AlreadySigned,
}

/// The signature Gates in mangos order (`cm:PetitionsHandler.cpp:338-396`). mangos stays silent
/// when the owner signs; here the owner hears CANT_SIGN_OWN, as vmangos answers
/// (`vm:src/game/Handlers/PetitionsHandler.cpp:232-240`). A signer whose Realm Account is unknown
/// is refused: without it, one Signature per Account cannot hold.
fn sign_verdict(
    owner_guid: u64,
    petition_team: u32,
    signatures: &[SignatureFacts],
    signer: &SignatureFacts,
    signer_team: u32,
    signer_is_member: bool,
) -> Result<SignVerdict, GuildRefusal> {
    if signer.signer_guid == owner_guid {
        return Err(GuildRefusal::CantSignOwn);
    }
    if signer_team != petition_team {
        return Err(GuildRefusal::NotAllied);
    }
    if signer_is_member {
        return Err(GuildRefusal::AlreadyInGuild);
    }
    if signatures.len() >= MAX_PETITION_SIGNATURES {
        return Err(GuildRefusal::PetitionFull);
    }
    if signer.signer_realm_account == 0 {
        return Err(GuildRefusal::UnknownRealmAccount);
    }
    if signatures.iter().any(|signature| {
        signature.signer_guid == signer.signer_guid
            || signature.signer_realm_account == signer.signer_realm_account
    }) {
        return Ok(SignVerdict::AlreadySigned);
    }
    Ok(SignVerdict::Sign)
}

/// The turn-in Gates in mangos order (`cm:PetitionsHandler.cpp:530-571`).
fn turn_in_gate(
    owner_guid: u64,
    actor_guid: u64,
    actor_is_member: bool,
    signature_count: usize,
    name_taken: bool,
) -> Result<(), GuildRefusal> {
    if actor_is_member {
        return Err(GuildRefusal::AlreadyInGuild);
    }
    if actor_guid != owner_guid {
        return Err(GuildRefusal::NotPetitionOwner);
    }
    if signature_count < MIN_PETITION_SIGNATURES {
        return Err(GuildRefusal::NeedMoreSignatures);
    }
    if name_taken {
        return Err(GuildRefusal::NameExists);
    }
    Ok(())
}

/// The signers that join the new Guild, in slot order. A signer who joined another Guild
/// since it signed is skipped (`cm:Guild.cpp:167-179`).
fn joining_signers(
    mut signatures: Vec<GuildPetitionSignature>,
    is_member: impl Fn(u64) -> bool,
) -> Vec<GuildPetitionSignature> {
    signatures.sort_by_key(|signature| signature.signature_key);
    signatures.retain(|signature| !is_member(signature.signer_guid));
    signatures
}

fn petition_of_charter(ctx: &ReducerContext, charter_item_guid: u64) -> Option<GuildPetition> {
    ctx.db
        .game_guild_petition()
        .charter_item_guid()
        .find(charter_item_guid)
}

fn signatures_of(ctx: &ReducerContext, petition_id: u32) -> Vec<GuildPetitionSignature> {
    ctx.db
        .game_guild_petition_signature()
        .by_petition()
        .filter(petition_id)
        .collect()
}

fn guild_name_taken(ctx: &ReducerContext, name: &str) -> bool {
    ctx.db
        .game_guild()
        .name_key()
        .find(name_key(name))
        .is_some()
}

fn delete_petition(ctx: &ReducerContext, petition_id: u32) {
    ctx.db
        .game_guild_petition_signature()
        .by_petition()
        .delete(petition_id);
    ctx.db
        .game_guild_petition()
        .petition_id()
        .delete(petition_id);
}

/// Does `owner_guid` own an open Petition? The Charter decision refuses a second one
/// (`vm:src/game/Handlers/PetitionsHandler.cpp:69-71`).
pub(crate) fn owns_petition(ctx: &ReducerContext, owner_guid: u64) -> bool {
    ctx.db
        .game_guild_petition()
        .owner_guid()
        .find(owner_guid)
        .is_some()
}

/// Open the Petition an accepted Charter purchase pays for. Answers its id.
pub(crate) fn open(
    ctx: &ReducerContext,
    owner_guid: u64,
    owner_name: &str,
    team: u32,
    name: &str,
    charter_item_guid: u64,
) -> u32 {
    ctx.db
        .game_guild_petition()
        .insert(GuildPetition {
            petition_id: 0,
            charter_item_guid,
            owner_guid,
            owner_name: owner_name.to_string(),
            team,
            name: name.to_string(),
            created_micros: ctx.timestamp.to_micros_since_unix_epoch(),
        })
        .petition_id
}

/// `SignPetition`: the actor signs. Both the owner and the signer hear the result through an
/// addressed Guild Event, so an already-signed Realm Account is an outcome, not a Refusal: the
/// owner's event must commit even though no Signature does.
pub fn sign(
    ctx: &ReducerContext,
    actor_guid: u64,
    actor_account: u64,
    request: GuildPetitionSign,
) -> Result<(), GuildRefusal> {
    let petition =
        petition_of_charter(ctx, request.charter_item_guid).ok_or(GuildRefusal::NoSuchPetition)?;
    let signatures = signatures_of(ctx, petition.petition_id);
    let facts: Vec<SignatureFacts> = signatures
        .iter()
        .map(|signature| SignatureFacts {
            signer_guid: signature.signer_guid,
            signer_realm_account: signature.signer_realm_account,
        })
        .collect();
    let signer = SignatureFacts {
        signer_guid: actor_guid,
        signer_realm_account: actor_account,
    };
    let verdict = sign_verdict(
        petition.owner_guid,
        petition.team,
        &facts,
        &signer,
        request.actor_team,
        member(ctx, actor_guid).is_some(),
    )?;
    let kind = match verdict {
        SignVerdict::Sign => {
            let signature_key = (0..MAX_PETITION_SIGNATURES)
                .map(|slot| petition_signature_key(petition.petition_id, slot))
                .find(|key| !signatures.iter().any(|taken| taken.signature_key == *key))
                .ok_or(GuildRefusal::PetitionFull)?;
            ctx.db
                .game_guild_petition_signature()
                .insert(GuildPetitionSignature {
                    signature_key,
                    petition_id: petition.petition_id,
                    signer_guid: actor_guid,
                    signer_name: request.actor_name,
                    signer_realm_account: actor_account,
                    signed_micros: ctx.timestamp.to_micros_since_unix_epoch(),
                });
            event_kind::PETITION_SIGNED
        }
        SignVerdict::AlreadySigned => event_kind::PETITION_ALREADY_SIGNED,
    };
    for recipient in [petition.owner_guid, actor_guid] {
        push_event(
            ctx,
            0,
            recipient,
            kind,
            actor_guid,
            petition.charter_item_guid,
            Vec::new(),
        );
    }
    Ok(())
}

/// `OfferPetition` (`cm:PetitionsHandler.cpp:452-512`): the owner shows its Petition to a live
/// Character of its team outside any Guild.
pub fn offer(
    ctx: &ReducerContext,
    actor_guid: u64,
    request: GuildPetitionOffer,
) -> Result<(), GuildRefusal> {
    let petition =
        petition_of_charter(ctx, request.charter_item_guid).ok_or(GuildRefusal::NoSuchPetition)?;
    if petition.owner_guid != actor_guid {
        return Err(GuildRefusal::NotPetitionOwner);
    }
    if request.target_team != petition.team {
        return Err(GuildRefusal::NotAllied);
    }
    if member(ctx, request.target_guid).is_some() {
        return Err(GuildRefusal::AlreadyInGuild);
    }
    push_event(
        ctx,
        0,
        request.target_guid,
        event_kind::PETITION_OFFERED,
        actor_guid,
        petition.charter_item_guid,
        Vec::new(),
    );
    Ok(())
}

/// `DeclinePetition` (`cm:PetitionsHandler.cpp:424-450`): the owner learns who declined.
pub fn decline(
    ctx: &ReducerContext,
    actor_guid: u64,
    charter_item_guid: u64,
) -> Result<(), GuildRefusal> {
    let petition =
        petition_of_charter(ctx, charter_item_guid).ok_or(GuildRefusal::NoSuchPetition)?;
    push_event(
        ctx,
        0,
        petition.owner_guid,
        event_kind::PETITION_DECLINED,
        actor_guid,
        petition.charter_item_guid,
        Vec::new(),
    );
    Ok(())
}

/// `RenamePetition` (`cm:PetitionsHandler.cpp:284-319`).
pub fn rename(
    ctx: &ReducerContext,
    actor_guid: u64,
    request: GuildPetitionRename,
) -> Result<(), GuildRefusal> {
    let mut petition =
        petition_of_charter(ctx, request.charter_item_guid).ok_or(GuildRefusal::NoSuchPetition)?;
    if petition.owner_guid != actor_guid {
        return Err(GuildRefusal::NotPetitionOwner);
    }
    if guild_name_taken(ctx, &request.name) {
        return Err(GuildRefusal::NameExists);
    }
    validate_guild_name(&request.name)?;
    petition.name = request.name;
    ctx.db.game_guild_petition().petition_id().update(petition);
    Ok(())
}

/// `TurnInPetition` (`cm:PetitionsHandler.cpp:514-634`): found the Guild with the owner at rank 0
/// and every signer still outside a Guild at the lowest Guild Rank. The Petition and its
/// Signatures go first, so the `add_member` hook finds none of them. The Charter item is the
/// Gateway's to destroy on the Home Shard afterwards; until then it is inert, because its Petition
/// is gone.
pub fn turn_in(
    ctx: &ReducerContext,
    actor_guid: u64,
    actor_account: u64,
    charter_item_guid: u64,
) -> Result<(), GuildRefusal> {
    let petition =
        petition_of_charter(ctx, charter_item_guid).ok_or(GuildRefusal::NoSuchPetition)?;
    let signatures = signatures_of(ctx, petition.petition_id);
    turn_in_gate(
        petition.owner_guid,
        actor_guid,
        member(ctx, actor_guid).is_some(),
        signatures.len(),
        guild_name_taken(ctx, &petition.name),
    )?;
    delete_petition(ctx, petition.petition_id);
    let guild_id = create_guild(
        ctx,
        actor_guid,
        &petition.owner_name,
        petition.team,
        actor_account,
        &petition.name,
    )?;
    let rank_id = lowest_rank(ctx, guild_id);
    for signer in joining_signers(signatures, |guid| member(ctx, guid).is_some()) {
        add_member(
            ctx,
            guild_id,
            signer.signer_guid,
            &signer.signer_name,
            rank_id,
            signer.signer_realm_account,
        )?;
        push_event(
            ctx,
            guild_id,
            signer.signer_guid,
            event_kind::FOUNDER,
            0,
            0,
            vec![petition.name.clone()],
        );
    }
    Ok(())
}

/// `ClosePetition`: the owner drops its Petition, as the Gateway does before a new Charter
/// purchase when the old Charter is gone.
pub fn close(ctx: &ReducerContext, actor_guid: u64, petition_id: u32) -> Result<(), GuildRefusal> {
    let petition = ctx
        .db
        .game_guild_petition()
        .petition_id()
        .find(petition_id)
        .ok_or(GuildRefusal::NoSuchPetition)?;
    if petition.owner_guid != actor_guid {
        return Err(GuildRefusal::NotPetitionOwner);
    }
    delete_petition(ctx, petition_id);
    Ok(())
}

/// A Character joined a Guild: its own Petition closes and every Signature it made is struck, and
/// each owner who lost one gets a fresh petition query (`cm:Guild.cpp:181-183`,
/// `cm:Player.cpp:16980-17007`).
pub(super) fn forget_joiner(ctx: &ReducerContext, character_guid: u64) {
    if let Some(own) = ctx
        .db
        .game_guild_petition()
        .owner_guid()
        .find(character_guid)
    {
        delete_petition(ctx, own.petition_id);
    }
    let signed: Vec<GuildPetitionSignature> = ctx
        .db
        .game_guild_petition_signature()
        .by_signer()
        .filter(character_guid)
        .collect();
    for signature in signed {
        ctx.db
            .game_guild_petition_signature()
            .signature_key()
            .delete(signature.signature_key);
        if let Some(petition) = ctx
            .db
            .game_guild_petition()
            .petition_id()
            .find(signature.petition_id)
        {
            push_event(
                ctx,
                0,
                petition.owner_guid,
                event_kind::PETITION_CHANGED,
                character_guid,
                petition.charter_item_guid,
                Vec::new(),
            );
        }
    }
}

/// Destroy a turned-in Guild Charter on the actor's Home Shard. A Charter that is already gone is
/// Ok, so a repeated call changes nothing. Any other item is left alone. Nothing retries a failed
/// call: that Charter stays in the bags with no Petition behind it, founds nothing, and waits for
/// its owner to destroy it.
#[reducer]
pub fn gw_destroy_guild_charter(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    charter_item_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let items = ctx.db.game_item_instance();
    let Some(item) = items.guid().find(charter_item_guid) else {
        return Ok(());
    };
    if item.owner_guid != actor_guid || item.entry != GUILD_CHARTER_ENTRY {
        return Err(format!(
            "item {charter_item_guid} is not a Guild Charter of {actor_guid}"
        ));
    }
    items.guid().delete(charter_item_guid);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: u64 = 5_090_501;
    const ALLIANCE: u32 = 469;
    const HORDE: u32 = 67;

    fn signature(signer_guid: u64, signer_realm_account: u64) -> SignatureFacts {
        SignatureFacts {
            signer_guid,
            signer_realm_account,
        }
    }

    fn verdict(
        signatures: &[SignatureFacts],
        signer: SignatureFacts,
        team: u32,
        is_member: bool,
    ) -> Result<SignVerdict, GuildRefusal> {
        sign_verdict(OWNER, ALLIANCE, signatures, &signer, team, is_member)
    }

    #[test]
    fn signature_gates_run_in_mangos_order() {
        let full: Vec<SignatureFacts> = (1..=9).map(|n| signature(5_090_510 + n, n)).collect();
        assert_eq!(
            verdict(&full, signature(OWNER, 1), HORDE, true),
            Err(GuildRefusal::CantSignOwn)
        );
        assert_eq!(
            verdict(&full, signature(5_090_530, 1), HORDE, true),
            Err(GuildRefusal::NotAllied)
        );
        assert_eq!(
            verdict(&full, signature(5_090_530, 1), ALLIANCE, true),
            Err(GuildRefusal::AlreadyInGuild)
        );
        assert_eq!(
            verdict(&full, signature(5_090_530, 1), ALLIANCE, false),
            Err(GuildRefusal::PetitionFull),
            "a full Petition answers before the Account check"
        );
        assert_eq!(
            verdict(&full[..8], signature(5_090_530, 1), ALLIANCE, false),
            Ok(SignVerdict::AlreadySigned)
        );
        assert_eq!(
            verdict(&full[..8], signature(5_090_530, 30), ALLIANCE, false),
            Ok(SignVerdict::Sign)
        );
    }

    #[test]
    fn one_signature_per_realm_account_and_an_unknown_account_signs_nothing() {
        let signed = [signature(5_090_511, 76), signature(5_090_512, 77)];
        assert_eq!(
            verdict(&signed, signature(5_090_513, 0), ALLIANCE, false),
            Err(GuildRefusal::UnknownRealmAccount),
            "an Account the Gateway could not name could sign without limit"
        );
        assert_eq!(
            verdict(&signed, signature(5_090_513, 77), ALLIANCE, false),
            Ok(SignVerdict::AlreadySigned)
        );
        assert_eq!(
            verdict(&signed, signature(5_090_511, 78), ALLIANCE, false),
            Ok(SignVerdict::AlreadySigned),
            "the same Character never signs twice"
        );
        assert_eq!(
            verdict(&signed, signature(5_090_513, 78), ALLIANCE, false),
            Ok(SignVerdict::Sign)
        );
    }

    #[test]
    fn turn_in_gates_run_in_mangos_order() {
        assert_eq!(
            turn_in_gate(OWNER, 5_090_530, true, 0, true),
            Err(GuildRefusal::AlreadyInGuild)
        );
        assert_eq!(
            turn_in_gate(OWNER, 5_090_530, false, 9, false),
            Err(GuildRefusal::NotPetitionOwner)
        );
        assert_eq!(
            turn_in_gate(OWNER, OWNER, false, 8, true),
            Err(GuildRefusal::NeedMoreSignatures)
        );
        assert_eq!(
            turn_in_gate(OWNER, OWNER, false, 9, true),
            Err(GuildRefusal::NameExists)
        );
        assert_eq!(turn_in_gate(OWNER, OWNER, false, 9, false), Ok(()));
    }

    fn signature_row(slot: usize, signer_guid: u64) -> GuildPetitionSignature {
        GuildPetitionSignature {
            signature_key: petition_signature_key(4, slot),
            petition_id: 4,
            signer_guid,
            signer_name: format!("Signer{slot}"),
            signer_realm_account: signer_guid,
            signed_micros: 0,
        }
    }

    #[test]
    fn founding_skips_signers_who_joined_another_guild() {
        let signatures = vec![
            signature_row(2, 5_090_513),
            signature_row(0, 5_090_511),
            signature_row(1, 5_090_512),
        ];
        let joining: Vec<u64> = joining_signers(signatures, |guid| guid == 5_090_512)
            .into_iter()
            .map(|signature| signature.signer_guid)
            .collect();
        assert_eq!(joining, [5_090_511, 5_090_513]);
    }

    /// The actor guid is an argument, so the operator gate is the whole authorization.
    #[test]
    fn destroying_a_charter_opens_with_the_operator_gate() {
        let body = crate::test_scan::code_of(
            include_str!("petition.rs"),
            "pub fn gw_destroy_guild_charter(",
        );
        let normalized = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
            "`gw_destroy_guild_charter` no longer opens with the operator gate. Body was:\n{body}"
        );
    }
}
