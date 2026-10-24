use lyracore_shared::guild::{
    event_kind, founding_gate, GuildRefusal, DEFAULT_MOTD, DEFAULT_RANKS, LEADER_RANK,
};
use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table, Timestamp};

pub(crate) mod chat;
mod cleanup;
pub(crate) use cleanup::forget_deleted_character;
pub mod fee;
pub mod membership;
pub mod petition;
pub(crate) use fee::{sweep_delete_game_guild_fee_hold, sweep_transfer_game_guild_fee_hold};
mod settings;

#[table(accessor = game_guild, index(accessor = by_leader, btree(columns = [leader_guid])))]
pub struct Guild {
    #[primary_key]
    #[auto_inc]
    pub guild_id: u32,
    #[unique]
    pub name_key: String,
    pub name: String,
    pub leader_guid: u64,
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

#[table(accessor = game_guild_member, index(accessor = by_guild, btree(columns = [guild_id])))]
pub struct GuildMember {
    #[primary_key]
    pub character_guid: u64,
    pub guild_id: u32,
    pub rank_id: u32,
    pub name: String,
    pub public_note: String,
    pub officer_note: String,
    pub realm_account_id: u64,
    pub joined_micros: i64,
}

impl GuildMember {
    fn insert(self, ctx: &ReducerContext) -> Self {
        let row = ctx.db.game_guild_member().insert(self);
        crate::native::guild_membership_changed(ctx, row.character_guid);
        row
    }

    pub(super) fn update(self, ctx: &ReducerContext) -> Self {
        let previous = member(ctx, self.character_guid);
        let row = ctx.db.game_guild_member().character_guid().update(self);
        if previous.is_none_or(|old| (old.guild_id, old.rank_id) != (row.guild_id, row.rank_id)) {
            crate::native::guild_membership_changed(ctx, row.character_guid);
        }
        row
    }

    pub(super) fn delete(ctx: &ReducerContext, guid: u64) {
        if ctx.db.game_guild_member().character_guid().delete(guid) {
            crate::native::guild_membership_changed(ctx, guid);
        }
    }
}

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
    pub kind: u8,
    pub subject_guid: u64,
    pub other_guid: u64,
    pub strings: Vec<String>,
    pub created_at: Timestamp,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildGmCreate {
    pub leader_guid: u64,
    pub leader_name: String,
    pub leader_team: u32,
    pub leader_realm_account: u64,
    pub gm_level: u8,
    pub name: String,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildInviteRequest {
    pub target_guid: u64,
    pub actor_team: u32,
    pub target_team: u32,
    pub target_ignores_actor: bool,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildAcceptRequest {
    pub actor_name: String,
    pub actor_team: u32,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildNoteEdit {
    pub target_guid: u64,
    pub text: String,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildRankEdit {
    pub rank_id: u32,
    pub rights: u32,
    pub name: String,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum GuildOp {
    GmCreate(GuildGmCreate),
    SignOn { actor_name: String },
    SignOff,
    Invite(GuildInviteRequest),
    Accept(GuildAcceptRequest),
    Decline { actor_name: String },
    Leave,
    Remove { target_guid: u64 },
    Promote { target_guid: u64 },
    Demote { target_guid: u64 },
    SetLeader { target_guid: u64 },
    Disband,
    SetMotd { text: String },
    SetInfo { text: String },
    SetPublicNote(GuildNoteEdit),
    SetOfficerNote(GuildNoteEdit),
    EditRank(GuildRankEdit),
    AddRank { name: String },
    DeleteRank,
    SignPetition(petition::GuildPetitionSign),
    OfferPetition(petition::GuildPetitionOffer),
    DeclinePetition { charter_item_guid: u64 },
    RenamePetition(petition::GuildPetitionRename),
    TurnInPetition { charter_item_guid: u64 },
    ClosePetition { petition_id: u32 },
    ForgetDeletedCharacter,
}

#[reducer]
pub fn realm_guild_op(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    op: GuildOp,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let actor_account = request_actor.ownership.as_ref().map_or(0, |t| t.account_id);
    let tokenless = request_actor.ownership.is_none();
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    if matches!(op, GuildOp::ForgetDeletedCharacter) && !tokenless {
        return Err(format!(
            "Character {actor_guid} has a World Session and is not forgotten"
        ));
    }
    apply_op(ctx, actor_guid, actor_account, op).map_err(|refusal| refusal.as_tag().to_string())
}

pub(crate) fn apply_op(
    ctx: &ReducerContext,
    actor_guid: u64,
    actor_account: u64,
    op: GuildOp,
) -> Result<(), GuildRefusal> {
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
        GuildOp::ForgetDeletedCharacter => {
            cleanup::forget_deleted_character(ctx, actor_guid);
            Ok(())
        }
    }
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
        row = row.update(ctx);
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

pub(crate) fn create_guild(
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

pub(crate) fn add_member(
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
    petition::withdraw(ctx, character_guid);
    GuildMember {
        character_guid,
        guild_id,
        rank_id,
        name: name.to_string(),
        public_note: String::new(),
        officer_note: String::new(),
        realm_account_id,
        joined_micros: ctx.timestamp.to_micros_since_unix_epoch(),
    }
    .insert(ctx);
    Ok(())
}

pub(crate) fn member(ctx: &ReducerContext, character_guid: u64) -> Option<GuildMember> {
    ctx.db
        .game_guild_member()
        .character_guid()
        .find(character_guid)
}

pub(crate) fn ranks(ctx: &ReducerContext, guild_id: u32) -> Vec<GuildRank> {
    let mut ranks: Vec<GuildRank> = ctx
        .db
        .game_guild_rank()
        .by_guild()
        .filter(guild_id)
        .collect();
    ranks.sort_by_key(|rank| rank.rank_id);
    ranks
}

pub(crate) fn lowest_rank(ctx: &ReducerContext, guild_id: u32) -> u32 {
    ctx.db
        .game_guild_rank()
        .by_guild()
        .filter(guild_id)
        .map(|rank| rank.rank_id)
        .max()
        .unwrap_or(LEADER_RANK)
}

pub(crate) fn rank_rights(ctx: &ReducerContext, guild_id: u32, rank_id: u32) -> u32 {
    ctx.db
        .game_guild_rank()
        .by_guild()
        .filter(guild_id)
        .find(|rank| rank.rank_id == rank_id)
        .map_or(0, |rank| rank.rights)
}

pub(crate) fn push_event(
    ctx: &ReducerContext,
    guild_id: u32,
    recipient_guid: u64,
    kind: u8,
    subject_guid: u64,
    other_guid: u64,
    strings: Vec<String>,
) {
    let event = ctx.db.game_guild_event().insert(GuildEvent {
        id: 0,
        guild_id,
        recipient_guid,
        kind,
        subject_guid,
        other_guid,
        strings,
        created_at: ctx.timestamp,
    });
    crate::native::guild_event(ctx, &event);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guild_op_variant_bsatn_tags_are_pinned_by_position() {
        let cases: [(&str, GuildOp, u8); 26] = [
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
            (
                "ForgetDeletedCharacter",
                GuildOp::ForgetDeletedCharacter,
                25,
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
