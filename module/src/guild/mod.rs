//! Guilds on Realm-core. Every guild fact lives here: the Guild, its Guild Ranks, its members, the
//! Petitions that found new Guilds ([`petition`]) and the Guild Events that tell members what
//! happened. Every table is private. The copper a guild operation costs stays on the payer's Home
//! Shard in a Fee Hold until Realm-core decides ([`fee`]).
//!
//! Realm-core holds no Character rows, so the Gateway conveys the Character facts a Gate needs
//! (name, team, Realm Account, GM level) inside the Durable Request, and this Module applies the
//! rules. Every guild Durable Request enters through [`realm_guild_op`].
//!
//! PLAYER_GUILDID and PLAYER_GUILDRANK are a Gateway projection of `game_guild_member` (the Guild
//! Projection). No World Shard stores them.

use lyracore_shared::guild::{
    event_kind, founding_gate, GuildRefusal, DEFAULT_MOTD, DEFAULT_RANKS, LEADER_RANK,
};
use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table, Timestamp};

pub(crate) mod chat;
pub mod fee;
pub mod membership;
pub mod petition;
pub(crate) use fee::{sweep_delete_game_guild_fee_hold, sweep_transfer_game_guild_fee_hold};
mod settings;

/// One Guild. `name_key` makes names unique without regard to case.
#[table(accessor = game_guild, index(accessor = by_leader, btree(columns = [leader_guid])))]
pub struct Guild {
    #[primary_key]
    #[auto_inc]
    pub guild_id: u32,
    #[unique]
    pub name_key: String,
    pub name: String,
    pub leader_guid: u64,
    /// `lyracore_shared::faction::TEAM_*`, fixed at founding.
    pub team: u32,
    pub motd: String,
    pub info: String,
    pub emblem_style: u32,
    pub emblem_color: u32,
    pub border_style: u32,
    pub border_color: u32,
    pub background_color: u32,
    pub created_micros: i64,
}

/// One Guild Rank. `rank_id` 0 is the highest; ids are dense from 0.
#[table(accessor = game_guild_rank, index(accessor = by_guild, btree(columns = [guild_id])))]
pub struct GuildRank {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub guild_id: u32,
    pub rank_id: u32,
    pub name: String,
    pub rights: u32,
}

/// One Character's membership. A Character is in at most one Guild.
#[table(accessor = game_guild_member, index(accessor = by_guild, btree(columns = [guild_id])))]
pub struct GuildMember {
    #[primary_key]
    pub character_guid: u64,
    pub guild_id: u32,
    pub rank_id: u32,
    /// Name snapshot, like mangos `MemberSlot::Name` (`cm:Guild.h:173`). Guild Events and by-name
    /// member ops read it, so they need no Character row. Refreshed at sign-on.
    pub name: String,
    pub public_note: String,
    pub officer_note: String,
    /// The member's Realm Account. 0 when unknown; the guild info count treats it as its own
    /// Account.
    pub realm_account_id: u64,
    pub joined_micros: i64,
}

/// One Guild Event. A row with `recipient_guid == 0` goes to every online member of `guild_id`;
/// a nonzero `recipient_guid` addresses that one Character. The strings are final, so the Gateway
/// renders without reads. Reaped on the shared event TTL.
#[table(
    accessor = game_guild_event,
    index(accessor = by_guild, btree(columns = [guild_id])),
    index(accessor = by_recipient, btree(columns = [recipient_guid]))
)]
pub struct GuildEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub guild_id: u32,
    pub recipient_guid: u64,
    /// `lyracore_shared::guild::event_kind`.
    pub kind: u8,
    /// The Character the event is about. SMSG_GUILD_EVENT writes it after the strings when nonzero.
    pub subject_guid: u64,
    /// A second object the event names, such as a Guild Charter item.
    pub other_guid: u64,
    pub strings: Vec<String>,
    pub created_at: Timestamp,
}

/// A GM founds a Guild led by `leader_guid`. The leader may be offline.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildGmCreate {
    pub leader_guid: u64,
    pub leader_name: String,
    pub leader_team: u32,
    pub leader_realm_account: u64,
    /// The acting Character's GM level, read from its Home Shard.
    pub gm_level: u8,
    pub name: String,
}

/// A member invites a live Character on another World Shard to join its Guild. The Gateway
/// resolves the target realm-wide and conveys the facts Realm-core cannot read itself: each side's
/// team (from race) and whether the target ignores the actor.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildInviteRequest {
    pub target_guid: u64,
    pub actor_team: u32,
    pub target_team: u32,
    pub target_ignores_actor: bool,
}

/// The actor accepts its pending Guild Invite. Realm-core holds no Character row for a non-member,
/// so the Gateway conveys the name and team `add_member` needs.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildAcceptRequest {
    pub actor_name: String,
    pub actor_team: u32,
}

/// A Public or Officer Note edit: the same shape, gated by a different Rank Right and field.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildNoteEdit {
    pub target_guid: u64,
    pub text: String,
}

/// A Guild Rank rename and rights change.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildRankEdit {
    pub rank_id: u32,
    pub rights: u32,
    pub name: String,
}

/// One guild Durable Request. Later ops append variants here.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum GuildOp {
    GmCreate(GuildGmCreate),
    /// The actor entered the world. `actor_name` refreshes the member's name snapshot.
    SignOn {
        actor_name: String,
    },
    SignOff,
    Invite(GuildInviteRequest),
    Accept(GuildAcceptRequest),
    /// The actor declines its pending Guild Invite. `actor_name` addresses the inviter's event,
    /// since Realm-core holds no name snapshot for a non-member.
    Decline {
        actor_name: String,
    },
    Leave,
    Remove {
        target_guid: u64,
    },
    Promote {
        target_guid: u64,
    },
    Demote {
        target_guid: u64,
    },
    SetLeader {
        target_guid: u64,
    },
    Disband,
    SetMotd {
        text: String,
    },
    SetInfo {
        text: String,
    },
    SetPublicNote(GuildNoteEdit),
    SetOfficerNote(GuildNoteEdit),
    EditRank(GuildRankEdit),
    AddRank {
        name: String,
    },
    DeleteRank,
    SignPetition(petition::GuildPetitionSign),
    OfferPetition(petition::GuildPetitionOffer),
    /// The actor declines to sign the Petition of `charter_item_guid`.
    DeclinePetition {
        charter_item_guid: u64,
    },
    RenamePetition(petition::GuildPetitionRename),
    TurnInPetition {
        charter_item_guid: u64,
    },
    ClosePetition {
        petition_id: u32,
    },
}

/// Run one guild op for the acting Character.
///
/// Operator-gated because the actor's guid is an argument: Realm-core has no live entity to derive
/// it from. A Refusal returns its [`GuildRefusal`] tag as the whole error text and commits nothing.
#[reducer]
pub fn realm_guild_op(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    op: GuildOp,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let actor_account = request_actor.ownership.as_ref().map_or(0, |t| t.account_id);
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    match op {
        GuildOp::GmCreate(request) => gm_create(ctx, request).map(|_| ()),
        GuildOp::SignOn { actor_name } => sign_on(ctx, actor_guid, &actor_name),
        GuildOp::SignOff => sign_off(ctx, actor_guid),
        GuildOp::Invite(request) => membership::invite(ctx, actor_guid, request),
        GuildOp::Accept(request) => membership::accept(ctx, actor_guid, actor_account, request),
        GuildOp::Decline { actor_name } => membership::decline(ctx, actor_guid, &actor_name),
        GuildOp::Leave => membership::leave(ctx, actor_guid),
        GuildOp::Remove { target_guid } => membership::remove(ctx, actor_guid, target_guid),
        GuildOp::Promote { target_guid } => membership::promote(ctx, actor_guid, target_guid),
        GuildOp::Demote { target_guid } => membership::demote(ctx, actor_guid, target_guid),
        GuildOp::SetLeader { target_guid } => membership::set_leader(ctx, actor_guid, target_guid),
        GuildOp::Disband => membership::disband(ctx, actor_guid),
        GuildOp::SetMotd { text } => settings::set_motd(ctx, actor_guid, &text),
        GuildOp::SetInfo { text } => settings::set_info(ctx, actor_guid, &text),
        GuildOp::SetPublicNote(edit) => settings::set_public_note(ctx, actor_guid, edit),
        GuildOp::SetOfficerNote(edit) => settings::set_officer_note(ctx, actor_guid, edit),
        GuildOp::EditRank(edit) => settings::edit_rank(ctx, actor_guid, edit),
        GuildOp::AddRank { name } => settings::add_rank(ctx, actor_guid, &name),
        GuildOp::DeleteRank => settings::delete_rank(ctx, actor_guid),
        GuildOp::SignPetition(request) => petition::sign(ctx, actor_guid, actor_account, request),
        GuildOp::OfferPetition(request) => petition::offer(ctx, actor_guid, request),
        GuildOp::DeclinePetition { charter_item_guid } => {
            petition::decline(ctx, actor_guid, charter_item_guid)
        }
        GuildOp::RenamePetition(request) => petition::rename(ctx, actor_guid, request),
        GuildOp::TurnInPetition { charter_item_guid } => {
            petition::turn_in(ctx, actor_guid, actor_account, charter_item_guid)
        }
        GuildOp::ClosePetition { petition_id } => petition::close(ctx, actor_guid, petition_id),
    }
    .map_err(|refusal| refusal.as_tag().to_string())
}

fn gm_create(ctx: &ReducerContext, request: GuildGmCreate) -> Result<u32, GuildRefusal> {
    if request.gm_level == 0 {
        return Err(GuildRefusal::NotGameMaster);
    }
    if request.leader_guid == 0 {
        return Err(GuildRefusal::NoSuchCharacter);
    }
    create_guild(
        ctx,
        request.leader_guid,
        &request.leader_name,
        request.leader_team,
        request.leader_realm_account,
        &request.name,
    )
}

fn sign_on(ctx: &ReducerContext, actor_guid: u64, actor_name: &str) -> Result<(), GuildRefusal> {
    let mut row = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    if !actor_name.is_empty() && row.name != actor_name {
        row.name = actor_name.to_string();
        row = ctx.db.game_guild_member().character_guid().update(row);
    }
    push_event(
        ctx,
        row.guild_id,
        0,
        event_kind::SIGNED_ON,
        actor_guid,
        0,
        vec![row.name],
    );
    Ok(())
}

fn sign_off(ctx: &ReducerContext, actor_guid: u64) -> Result<(), GuildRefusal> {
    let row = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    push_event(
        ctx,
        row.guild_id,
        0,
        event_kind::SIGNED_OFF,
        actor_guid,
        0,
        vec![row.name],
    );
    Ok(())
}

/// Found a Guild with the five default Guild Ranks and `leader_guid` at rank 0
/// (`cm:Guild.cpp:104-154`). The Gates and their order are `founding_gate`'s.
pub fn create_guild(
    ctx: &ReducerContext,
    leader_guid: u64,
    leader_name: &str,
    team: u32,
    leader_realm_account: u64,
    name: &str,
) -> Result<u32, GuildRefusal> {
    let key = founding_gate(
        name,
        |key| {
            ctx.db
                .game_guild()
                .name_key()
                .find(key.to_string())
                .is_some()
        },
        member(ctx, leader_guid).is_some(),
    )?;
    let guild = ctx.db.game_guild().insert(Guild {
        guild_id: 0,
        name_key: key,
        name: name.to_string(),
        leader_guid,
        team,
        motd: DEFAULT_MOTD.to_string(),
        info: String::new(),
        emblem_style: 0,
        emblem_color: 0,
        border_style: 0,
        border_color: 0,
        background_color: 0,
        created_micros: ctx.timestamp.to_micros_since_unix_epoch(),
    });
    for (rank_id, (rank_name, rights)) in (0u32..).zip(DEFAULT_RANKS) {
        ctx.db.game_guild_rank().insert(GuildRank {
            id: 0,
            guild_id: guild.guild_id,
            rank_id,
            name: rank_name.to_string(),
            rights,
        });
    }
    add_member(
        ctx,
        guild.guild_id,
        leader_guid,
        leader_name,
        LEADER_RANK,
        leader_realm_account,
    )?;
    Ok(guild.guild_id)
}

/// Insert one member row. This is the only place a member row is inserted. Refuses a Character
/// that is already a member of any Guild (`cm:Guild.cpp:167-179`). A joiner's own Petition closes
/// and its Signatures are struck.
pub fn add_member(
    ctx: &ReducerContext,
    guild_id: u32,
    character_guid: u64,
    name: &str,
    rank_id: u32,
    realm_account_id: u64,
) -> Result<(), GuildRefusal> {
    if member(ctx, character_guid).is_some() {
        return Err(GuildRefusal::AlreadyInGuild);
    }
    petition::forget_joiner(ctx, character_guid);
    ctx.db.game_guild_member().insert(GuildMember {
        character_guid,
        guild_id,
        rank_id,
        name: name.to_string(),
        public_note: String::new(),
        officer_note: String::new(),
        realm_account_id,
        joined_micros: ctx.timestamp.to_micros_since_unix_epoch(),
    });
    Ok(())
}

/// The membership row of `character_guid`, if it is in a Guild.
pub fn member(ctx: &ReducerContext, character_guid: u64) -> Option<GuildMember> {
    ctx.db
        .game_guild_member()
        .character_guid()
        .find(character_guid)
}

/// The Guild Ranks of `guild_id`, highest first.
pub fn ranks(ctx: &ReducerContext, guild_id: u32) -> Vec<GuildRank> {
    let mut ranks: Vec<GuildRank> = ctx
        .db
        .game_guild_rank()
        .by_guild()
        .filter(guild_id)
        .collect();
    ranks.sort_by_key(|rank| rank.rank_id);
    ranks
}

/// The lowest Guild Rank's id: where a new member starts.
pub fn lowest_rank(ctx: &ReducerContext, guild_id: u32) -> u32 {
    ctx.db
        .game_guild_rank()
        .by_guild()
        .filter(guild_id)
        .map(|rank| rank.rank_id)
        .max()
        .unwrap_or(LEADER_RANK)
}

/// The Rank Rights of one Guild Rank. An unknown rank reads 0 (`cm:Guild.cpp:660-666`).
pub fn rank_rights(ctx: &ReducerContext, guild_id: u32, rank_id: u32) -> u32 {
    ctx.db
        .game_guild_rank()
        .by_guild()
        .filter(guild_id)
        .find(|rank| rank.rank_id == rank_id)
        .map_or(0, |rank| rank.rights)
}

/// Write one Guild Event. `recipient_guid == 0` broadcasts to the Guild's online members.
pub fn push_event(
    ctx: &ReducerContext,
    guild_id: u32,
    recipient_guid: u64,
    kind: u8,
    subject_guid: u64,
    other_guid: u64,
    strings: Vec<String>,
) {
    ctx.db.game_guild_event().insert(GuildEvent {
        id: 0,
        guild_id,
        recipient_guid,
        kind,
        subject_guid,
        other_guid,
        strings,
        created_at: ctx.timestamp,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_scan::code_of;

    /// The actor guid is an argument, so the operator gate is the whole authorization. A gate
    /// that is present but neutralized (`if false`, `let _ =`, an early return) is no gate.
    #[test]
    fn the_realm_guild_op_reducer_opens_with_the_operator_gate() {
        let body = code_of(include_str!("mod.rs"), "pub fn realm_guild_op(");
        let normalized = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
            "`realm_guild_op` no longer opens with the operator gate. Body was:\n{body}"
        );
    }

    /// BSATN encodes a `GuildOp` variant as its ordinal position, so appending a variant in the
    /// wrong place silently renames every variant after it on the wire. This pins each variant's
    /// tag byte to a hand-written value: reordering `GuildOp` fails this test instead of shipping
    /// a schema a live client silently decodes as the wrong op.
    #[test]
    fn guild_op_variant_bsatn_tags_are_pinned_by_position() {
        let cases: [(&str, GuildOp, u8); 25] = [
            (
                "GmCreate",
                GuildOp::GmCreate(GuildGmCreate {
                    leader_guid: 0,
                    leader_name: String::new(),
                    leader_team: 0,
                    leader_realm_account: 0,
                    gm_level: 0,
                    name: String::new(),
                }),
                0,
            ),
            (
                "SignOn",
                GuildOp::SignOn {
                    actor_name: String::new(),
                },
                1,
            ),
            ("SignOff", GuildOp::SignOff, 2),
            (
                "Invite",
                GuildOp::Invite(GuildInviteRequest {
                    target_guid: 0,
                    actor_team: 0,
                    target_team: 0,
                    target_ignores_actor: false,
                }),
                3,
            ),
            (
                "Accept",
                GuildOp::Accept(GuildAcceptRequest {
                    actor_name: String::new(),
                    actor_team: 0,
                }),
                4,
            ),
            (
                "Decline",
                GuildOp::Decline {
                    actor_name: String::new(),
                },
                5,
            ),
            ("Leave", GuildOp::Leave, 6),
            ("Remove", GuildOp::Remove { target_guid: 0 }, 7),
            ("Promote", GuildOp::Promote { target_guid: 0 }, 8),
            ("Demote", GuildOp::Demote { target_guid: 0 }, 9),
            ("SetLeader", GuildOp::SetLeader { target_guid: 0 }, 10),
            ("Disband", GuildOp::Disband, 11),
            (
                "SetMotd",
                GuildOp::SetMotd {
                    text: String::new(),
                },
                12,
            ),
            (
                "SetInfo",
                GuildOp::SetInfo {
                    text: String::new(),
                },
                13,
            ),
            (
                "SetPublicNote",
                GuildOp::SetPublicNote(GuildNoteEdit {
                    target_guid: 0,
                    text: String::new(),
                }),
                14,
            ),
            (
                "SetOfficerNote",
                GuildOp::SetOfficerNote(GuildNoteEdit {
                    target_guid: 0,
                    text: String::new(),
                }),
                15,
            ),
            (
                "EditRank",
                GuildOp::EditRank(GuildRankEdit {
                    rank_id: 0,
                    rights: 0,
                    name: String::new(),
                }),
                16,
            ),
            (
                "AddRank",
                GuildOp::AddRank {
                    name: String::new(),
                },
                17,
            ),
            ("DeleteRank", GuildOp::DeleteRank, 18),
            (
                "SignPetition",
                GuildOp::SignPetition(petition::GuildPetitionSign {
                    charter_item_guid: 0,
                    actor_name: String::new(),
                    actor_team: 0,
                }),
                19,
            ),
            (
                "OfferPetition",
                GuildOp::OfferPetition(petition::GuildPetitionOffer {
                    charter_item_guid: 0,
                    target_guid: 0,
                    target_team: 0,
                }),
                20,
            ),
            (
                "DeclinePetition",
                GuildOp::DeclinePetition {
                    charter_item_guid: 0,
                },
                21,
            ),
            (
                "RenamePetition",
                GuildOp::RenamePetition(petition::GuildPetitionRename {
                    charter_item_guid: 0,
                    name: String::new(),
                }),
                22,
            ),
            (
                "TurnInPetition",
                GuildOp::TurnInPetition {
                    charter_item_guid: 0,
                },
                23,
            ),
            (
                "ClosePetition",
                GuildOp::ClosePetition { petition_id: 0 },
                24,
            ),
        ];

        for (name, op, expected_tag) in cases {
            let encoded = spacetimedb::spacetimedb_lib::bsatn::to_vec(&op)
                .unwrap_or_else(|error| panic!("GuildOp::{name} failed to encode: {error:?}"));
            assert_eq!(
                encoded.first().copied(),
                Some(expected_tag),
                "GuildOp::{name} moved off BSATN tag {expected_tag}; a client still on the old \
                 wire schema would silently decode a different variant after this ships"
            );
        }
    }
}
