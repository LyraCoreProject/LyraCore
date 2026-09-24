//! Guild family: the guild window reads, CMSG_GUILD_CREATE, the `.guild create` dot-command, the
//! tabard designer, and the guild steps of world entry and exit. Every guild fact lives on
//! Realm-core, so every read and Durable Request here goes there. The Gateway adds only the
//! Character facts Realm-core cannot read: name, team, Realm Account, whether the Character is live,
//! and the actor's GM level from its Home Shard. A guild fee moves through `guild_fee`.

use super::super::guild_fee::{self, GuildFeeStore};
use super::super::*;
use lyracore_shared::guild::{event_kind, has_right, rights, GuildRefusal, LEADER_RANK};
use wow_world_messages::vanilla::{
    GuildCommand, GuildCommandResult, GuildEmblemResult, MSG_SAVE_GUILD_EMBLEM_Client,
    MSG_SAVE_GUILD_EMBLEM_Server, MSG_TABARDVENDOR_ACTIVATE,
};
use wow_world_messages::Guid;

/// Character facts a guild Gate or render needs, read from whichever World Shard holds the
/// Character.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CharacterFacts {
    pub(crate) guid: u64,
    pub(crate) name: String,
    pub(crate) race: u8,
    pub(crate) class: u8,
    pub(crate) level: u8,
    pub(crate) zone_id: u32,
    pub(crate) last_logout_micros: u64,
    /// A live entity on any World Shard, bots included.
    pub(crate) online: bool,
    /// 0 when no World Shard retains the Character's Realm Account.
    pub(crate) realm_account_id: u64,
}

/// One guild Durable Request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GuildRequest {
    /// A GM founds a Guild led by `leader_guid`.
    GmCreate {
        leader_guid: u64,
        leader_name: String,
        leader_team: u32,
        leader_realm_account: u64,
        gm_level: u8,
        name: String,
    },
    /// The actor entered the world; `actor_name` refreshes its member name snapshot.
    SignOn { actor_name: String },
    /// The actor left the world.
    SignOff,
    /// A member offers a live Character, resolved realm-wide, a place in its Guild.
    Invite {
        target_guid: u64,
        actor_team: u32,
        target_team: u32,
        target_ignores_actor: bool,
    },
    /// The actor accepts its pending Guild Invite.
    Accept { actor_name: String, actor_team: u32 },
    /// The actor declines its pending Guild Invite.
    Decline { actor_name: String },
    /// The actor leaves its Guild.
    Leave,
    /// A member expels `target_guid` from the actor's Guild.
    Remove { target_guid: u64 },
    /// A member raises `target_guid` one Guild Rank.
    Promote { target_guid: u64 },
    /// A member lowers `target_guid` one Guild Rank.
    Demote { target_guid: u64 },
    /// The Guild Leader passes leadership to `target_guid`.
    SetLeader { target_guid: u64 },
    /// The Guild Leader dissolves its Guild.
    Disband,
    /// CMSG_GUILD_MOTD.
    SetMotd { text: String },
    /// CMSG_GUILD_INFO_TEXT.
    SetInfo { text: String },
    /// CMSG_GUILD_SET_PUBLIC_NOTE, target already resolved by name.
    SetPublicNote { target_guid: u64, text: String },
    /// CMSG_GUILD_SET_OFFICER_NOTE, target already resolved by name.
    SetOfficerNote { target_guid: u64, text: String },
    /// CMSG_GUILD_RANK.
    EditRank {
        rank_id: u32,
        rights: u32,
        name: String,
    },
    /// CMSG_GUILD_ADD_RANK.
    AddRank { name: String },
    /// CMSG_GUILD_DEL_RANK.
    DeleteRank,
}

/// What Realm-core did with a guild Durable Request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GuildOutcome {
    Ran,
    Refused(GuildRefusal),
}

/// Guild reads and requests, in the seam's own vocabulary. Guild reads and `guild_op` go to
/// Realm-core; Character facts are realm-wide reads over the World Shards. Guild fees go through
/// [`GuildFeeStore`].
pub(crate) trait GuildActionStore: GuildFeeStore + Send + Sync {
    /// The membership row of `character_guid`, if it is in a Guild.
    fn guild_member(&self, character_guid: u64) -> Result<Option<codec::GuildMemberView>>;
    /// One Guild with its Guild Ranks.
    fn guild(&self, guild_id: u32) -> Result<Option<codec::GuildView>>;
    /// Every member row of one Guild.
    fn guild_members(&self, guild_id: u32) -> Result<Vec<codec::GuildMemberView>>;
    /// Facts for one Character from whichever World Shard holds it.
    fn guild_character_facts(&self, character_guid: u64) -> Result<Option<CharacterFacts>>;
    /// Every Character guid that carries `name`, across every World Shard.
    fn guild_characters_named(&self, name: &str) -> Result<Vec<u64>>;
    /// The actor's GM level, read from THIS handle: the actor's Home Shard. A copy an interrupted
    /// Transfer left on another shard is not authority.
    fn guild_gm_level(&self, actor_guid: u64) -> Result<u8>;
    /// The unit the actor has selected, 0 for none.
    fn guild_selected_target(&self, actor_guid: u64) -> u64;
    /// Does `owner_guid` have `other_guid` on their ignore list, checked realm-wide?
    fn guild_ignored_by(&self, owner_guid: u64, other_guid: u64) -> Result<bool>;
    /// Run one guild op on Realm-core as `actor_guid`.
    fn guild_op(&self, actor_guid: u64, request: GuildRequest) -> Result<GuildOutcome>;
    /// Does the NPC refuse the actor by faction? The one Gate a read path answers here.
    fn guild_npc_refuses(&self, npc_guid: u64, actor_guid: u64) -> Result<bool>;
}

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
        Ok(crate::stdb::Coordinator::guild_character_facts(
            self,
            character_guid,
        ))
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GuildActionPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
}

pub(crate) enum GuildActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

/// A lost reducer transport ends the session. Every other failure is a guild outcome the client
/// hears as silence.
fn is_fatal(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().contains("reducer transport disconnected"))
}

pub(crate) fn dispatch_guild_action<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<GuildActionOutcome> {
    let outbound = match msg {
        ClientOpcodeMessage::CMSG_GUILD_QUERY(query) => query_outbound(store, query.guild_id),
        ClientOpcodeMessage::CMSG_GUILD_ROSTER => roster_outbound(store, player),
        ClientOpcodeMessage::CMSG_GUILD_INFO => info_outbound(store, player),
        ClientOpcodeMessage::CMSG_GUILD_CREATE(create) => {
            create_outbound(store, player, create.guild_name)
        }
        ClientOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(activate) => {
            tabard_window_outbound(store, player, activate.guid.guid())
        }
        ClientOpcodeMessage::MSG_SAVE_GUILD_EMBLEM(save) => {
            save_emblem_outbound(store, player, &save)
        }
        ClientOpcodeMessage::CMSG_GUILD_INVITE(invite) => {
            invite_outbound(store, player, invite.invited_player)
        }
        ClientOpcodeMessage::CMSG_GUILD_ACCEPT => accept_outbound(store, player),
        ClientOpcodeMessage::CMSG_GUILD_DECLINE => decline_outbound(store, player),
        ClientOpcodeMessage::CMSG_GUILD_LEAVE => leave_outbound(store, player),
        ClientOpcodeMessage::CMSG_GUILD_REMOVE(remove) => named_member_op(
            store,
            player,
            remove.player_name,
            GuildOpKind::Remove,
            |target_guid| GuildRequest::Remove { target_guid },
        ),
        ClientOpcodeMessage::CMSG_GUILD_PROMOTE(promote) => named_member_op(
            store,
            player,
            promote.player_name,
            GuildOpKind::Promote,
            |target_guid| GuildRequest::Promote { target_guid },
        ),
        ClientOpcodeMessage::CMSG_GUILD_DEMOTE(demote) => named_member_op(
            store,
            player,
            demote.player_name,
            GuildOpKind::Demote,
            |target_guid| GuildRequest::Demote { target_guid },
        ),
        ClientOpcodeMessage::CMSG_GUILD_LEADER(leader) => named_member_op(
            store,
            player,
            leader.new_guild_leader_name,
            GuildOpKind::Leader,
            |target_guid| GuildRequest::SetLeader { target_guid },
        ),
        ClientOpcodeMessage::CMSG_GUILD_DISBAND => disband_outbound(store, player),
        ClientOpcodeMessage::CMSG_GUILD_MOTD(motd) => {
            motd_outbound(store, player, motd.message_of_the_day)
        }
        ClientOpcodeMessage::CMSG_GUILD_INFO_TEXT(info) => {
            info_text_outbound(store, player, info.guild_info)
        }
        ClientOpcodeMessage::CMSG_GUILD_SET_PUBLIC_NOTE(note) => set_note_outbound(
            store,
            player,
            note.player_name,
            note.note,
            |target_guid, text| GuildRequest::SetPublicNote { target_guid, text },
        ),
        ClientOpcodeMessage::CMSG_GUILD_SET_OFFICER_NOTE(note) => set_note_outbound(
            store,
            player,
            note.player_name,
            note.note,
            |target_guid, text| GuildRequest::SetOfficerNote { target_guid, text },
        ),
        ClientOpcodeMessage::CMSG_GUILD_RANK(rank) => {
            edit_rank_outbound(store, player, rank.rank_id, rank.rights, rank.rank_name)
        }
        ClientOpcodeMessage::CMSG_GUILD_ADD_RANK(add) => {
            add_rank_outbound(store, player, add.rank_name)
        }
        ClientOpcodeMessage::CMSG_GUILD_DEL_RANK => delete_rank_outbound(store, player),
        other => return Ok(GuildActionOutcome::PassThrough(other)),
    };
    match outbound {
        Ok(outbound) => Ok(GuildActionOutcome::Handled { outbound }),
        Err(error) if is_fatal(&error) => Err(error),
        Err(error) => {
            log::debug!(
                "world: guild request dropped (account {}): {error:#}",
                player.account_id
            );
            Ok(GuildActionOutcome::Handled {
                outbound: Vec::new(),
            })
        }
    }
}

fn command_result(command: GuildCommand, name: String, result: GuildCommandResult) -> Outbound {
    Outbound::One(ServerOpcodeMessage::SMSG_GUILD_COMMAND_RESULT(Box::new(
        codec::build_guild_command_result(command, name, result),
    )))
}

/// What mangos answers a guild request from outside any Guild (`cm:GuildHandler.cpp:43,248`).
fn not_in_guild() -> Outbound {
    command_result(
        GuildCommand::Create,
        String::new(),
        GuildCommandResult::GuildPlayerNotInGuild,
    )
}

/// Any Guild answers, in any state: the character list asks for the names of its Characters'
/// Guilds before world entry (`cm:Opcodes.cpp:112` STATUS_AUTHED).
fn query_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    guild_id: u32,
) -> Result<Vec<Outbound>> {
    Ok(vec![match store.guild(guild_id)? {
        Some(guild) => Outbound::One(ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(Box::new(
            codec::build_guild_query_response(&guild),
        ))),
        None => not_in_guild(),
    }])
}

/// The actor's membership and its Guild, when both exist.
fn actor_guild<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
) -> Result<Option<(codec::GuildMemberView, codec::GuildView)>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(None);
    };
    let Some(member) = store.guild_member(actor_guid)? else {
        return Ok(None);
    };
    Ok(store.guild(member.guild_id)?.map(|guild| (member, guild)))
}

/// Outside a Guild the client gets no reply (`cm:GuildHandler.cpp:265-266`). One viewer, so lines
/// are read lazily: `roster_packet` stops pulling once the roster body is full, and a member past
/// the cap never pays for a `guild_character_facts` read that would be discarded.
fn roster_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
) -> Result<Vec<Outbound>> {
    let Some((viewer, guild)) = actor_guild(store, player)? else {
        return Ok(Vec::new());
    };
    let members = store.guild_members(guild.guild_id)?;
    let now = now_micros();
    let lines = members
        .into_iter()
        .map(move |member| roster_line(store, member, now));
    let sees_officer_notes = has_right(guild.rank_rights(viewer.rank_id), rights::VIEWOFFNOTE);
    Ok(vec![Outbound::One(roster_packet(
        &guild,
        lines,
        sees_officer_notes,
    ))])
}

/// Every roster line of `guild_id`, officer note included; `roster_packet` blanks it per viewer.
/// Reading each member's Character facts is the expensive part of a roster, so a caller that
/// serves more than one viewer of the same Guild (a settings-change relay) must call this once and
/// pass the result to every `roster_packet` call, not call `roster_outbound` per viewer.
pub(crate) fn roster_lines<St: GuildActionStore + ?Sized>(
    store: &St,
    guild_id: u32,
) -> Result<Vec<codec::GuildRosterLine>> {
    let members = store.guild_members(guild_id)?;
    let now = now_micros();
    Ok(members
        .into_iter()
        .map(|member| roster_line(store, member, now))
        .collect())
}

/// One roster line, officer note included. A member whose World Shard cannot answer still lists,
/// offline, under its name snapshot.
fn roster_line<St: GuildActionStore + ?Sized>(
    store: &St,
    member: codec::GuildMemberView,
    now_micros: u64,
) -> codec::GuildRosterLine {
    let facts = store
        .guild_character_facts(member.character_guid)
        .ok()
        .flatten();
    let Some(facts) = facts else {
        return codec::GuildRosterLine {
            guid: member.character_guid,
            name: member.name,
            rank_id: member.rank_id,
            days_offline: Some(0.0),
            public_note: member.public_note,
            officer_note: member.officer_note,
            ..codec::GuildRosterLine::default()
        };
    };
    let days_offline = (!facts.online).then(|| {
        if facts.last_logout_micros == 0 {
            0.0
        } else {
            now_micros.saturating_sub(facts.last_logout_micros) as f32 / 86_400_000_000.0
        }
    });
    codec::GuildRosterLine {
        guid: member.character_guid,
        name: facts.name,
        rank_id: member.rank_id,
        level: facts.level,
        class: facts.class,
        zone_id: facts.zone_id,
        days_offline,
        public_note: member.public_note,
        officer_note: member.officer_note,
    }
}

/// SMSG_GUILD_ROSTER from `lines`, blanking each line's officer note unless `sees_officer_notes`.
/// The one place that encodes a roster, so CMSG_GUILD_ROSTER and a settings-change relay can never
/// draw the wire body two different ways. `lines` stays generic over `IntoIterator` rather than a
/// slice: the single-viewer client path feeds it a lazy iterator so a member past the roster's
/// body cap is never read, while a settings-change relay, which renders both note visibilities
/// from one shared read, feeds it `lines.iter().cloned()` over its already-collected Vec.
pub(crate) fn roster_packet(
    guild: &codec::GuildView,
    lines: impl IntoIterator<Item = codec::GuildRosterLine>,
    sees_officer_notes: bool,
) -> ServerOpcodeMessage {
    let lines = lines.into_iter().map(move |mut line| {
        if !sees_officer_notes {
            line.officer_note.clear();
        }
        line
    });
    ServerOpcodeMessage::SMSG_GUILD_ROSTER(Box::new(codec::build_guild_roster(guild, lines)))
}

fn info_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
) -> Result<Vec<Outbound>> {
    let Some((_, guild)) = actor_guild(store, player)? else {
        return Ok(vec![not_in_guild()]);
    };
    let members = store.guild_members(guild.guild_id)?;
    let member_count = u32::try_from(members.len()).unwrap_or(u32::MAX);
    Ok(vec![Outbound::One(ServerOpcodeMessage::SMSG_GUILD_INFO(
        Box::new(codec::build_guild_info(
            &guild,
            member_count,
            account_count(&members),
        )),
    ))])
}

/// Distinct Realm Accounts among `members`. A member whose Account is unknown counts as one.
fn account_count(members: &[codec::GuildMemberView]) -> u32 {
    let mut known: Vec<u64> = members
        .iter()
        .map(|member| member.realm_account_id)
        .filter(|account| *account != 0)
        .collect();
    known.sort_unstable();
    known.dedup();
    let unknown = members
        .iter()
        .filter(|member| member.realm_account_id == 0)
        .count();
    u32::try_from(known.len() + unknown).unwrap_or(u32::MAX)
}

/// CMSG_GUILD_CREATE founds a Guild for a GM only; the Guild Charter is the player path. mangos
/// replies to neither outcome (`cm:GuildHandler.cpp:46-64`).
fn create_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    name: String,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let gm_level = store.guild_gm_level(actor_guid)?;
    if gm_level == 0 {
        return Ok(Vec::new());
    }
    let Some(actor) = store.guild_character_facts(actor_guid)? else {
        return Ok(Vec::new());
    };
    let outcome = store.guild_op(actor_guid, gm_create(&actor, gm_level, name))?;
    if let GuildOutcome::Refused(refusal) = outcome {
        log::debug!("world: CMSG_GUILD_CREATE from {actor_guid} refused: {refusal:?}");
    }
    Ok(Vec::new())
}

fn gm_create(leader: &CharacterFacts, gm_level: u8, name: String) -> GuildRequest {
    GuildRequest::GmCreate {
        leader_guid: leader.guid,
        leader_name: leader.name.clone(),
        leader_team: lyracore_shared::faction::team_for_race(leader.race),
        leader_realm_account: leader.realm_account_id,
        gm_level,
        name,
    }
}

/// Every live Character on `name`, realm-wide; the first live candidate wins, like `whisper::run`.
fn live_character_named<St: GuildActionStore + ?Sized>(
    store: &St,
    name: &str,
) -> Result<Option<CharacterFacts>> {
    for guid in store.guild_characters_named(name)? {
        if let Some(facts) = store.guild_character_facts(guid)? {
            if facts.online {
                return Ok(Some(facts));
            }
        }
    }
    Ok(None)
}

/// CMSG_GUILD_INVITE (`cm:GuildHandler.cpp:66-131`): the actor offers a live Character, resolved
/// realm-wide, a place in its Guild. mangos answers "player not found" before any Guild Gate runs,
/// so this Gateway read happens first here too.
fn invite_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    typed_name: String,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let Some(target) = live_character_named(store, &typed_name)? else {
        return Ok(vec![command_result(
            GuildCommand::Invite,
            typed_name,
            GuildCommandResult::GuildPlayerNotFoundS,
        )]);
    };
    let Some(actor) = store.guild_character_facts(actor_guid)? else {
        return Ok(Vec::new());
    };
    let target_ignores_actor = store.guild_ignored_by(target.guid, actor_guid)?;
    let outcome = store.guild_op(
        actor_guid,
        GuildRequest::Invite {
            target_guid: target.guid,
            actor_team: lyracore_shared::faction::team_for_race(actor.race),
            target_team: lyracore_shared::faction::team_for_race(target.race),
            target_ignores_actor,
        },
    )?;
    Ok(match outcome {
        GuildOutcome::Ran => Vec::new(),
        GuildOutcome::Refused(refusal) => vec![membership_refusal_outbound(
            GuildOpKind::Invite,
            refusal,
            &typed_name,
            &target.name,
        )],
    })
}

/// CMSG_GUILD_ACCEPT (`cm:GuildHandler.cpp:192-211`): every failure is silent, like mangos.
fn accept_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let Some(actor) = store.guild_character_facts(actor_guid)? else {
        return Ok(Vec::new());
    };
    let outcome = store.guild_op(
        actor_guid,
        GuildRequest::Accept {
            actor_name: actor.name,
            actor_team: lyracore_shared::faction::team_for_race(actor.race),
        },
    )?;
    if let GuildOutcome::Refused(refusal) = outcome {
        log::debug!("world: CMSG_GUILD_ACCEPT from {actor_guid} refused: {refusal:?}");
    }
    Ok(Vec::new())
}

/// CMSG_GUILD_DECLINE (`cm:GuildHandler.cpp:214-238`): every failure is silent.
fn decline_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let Some(actor) = store.guild_character_facts(actor_guid)? else {
        return Ok(Vec::new());
    };
    let outcome = store.guild_op(
        actor_guid,
        GuildRequest::Decline {
            actor_name: actor.name,
        },
    )?;
    if let GuildOutcome::Refused(refusal) = outcome {
        log::debug!("world: CMSG_GUILD_DECLINE from {actor_guid} refused: {refusal:?}");
    }
    Ok(Vec::new())
}

/// CMSG_GUILD_LEAVE (`cm:GuildHandler.cpp:383-404`): mangos answers QUIT/0x00 only for an ordinary
/// member's leave, never for a lone Guild Leader's leave-and-disband. The Module refuses a Guild
/// Leader who still has company (`GuildRefusal::LeaderCannotLeave`), so a Guild Leader's Leave can
/// only ever *succeed* by disbanding alone. No member count needs reading here to tell the two
/// outcomes apart, only whether the actor already is the Guild Leader.
fn leave_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let Some((_, guild)) = actor_guild(store, player)? else {
        return Ok(vec![not_in_guild()]);
    };
    let is_leader = guild.leader_guid == actor_guid;
    let outcome = store.guild_op(actor_guid, GuildRequest::Leave)?;
    Ok(match outcome {
        GuildOutcome::Ran if is_leader => Vec::new(),
        GuildOutcome::Ran => vec![command_result(
            GuildCommand::Quit,
            guild.name,
            GuildCommandResult::PlayerNoMoreInGuild,
        )],
        GuildOutcome::Refused(refusal) => {
            vec![membership_refusal_outbound(
                GuildOpKind::Leave,
                refusal,
                "",
                "",
            )]
        }
    })
}

/// CMSG_GUILD_DISBAND (`cm:GuildHandler.cpp:424-435`).
fn disband_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let outcome = store.guild_op(actor_guid, GuildRequest::Disband)?;
    Ok(match outcome {
        GuildOutcome::Ran => Vec::new(),
        GuildOutcome::Refused(refusal) => {
            vec![membership_refusal_outbound(
                GuildOpKind::Disband,
                refusal,
                "",
                "",
            )]
        }
    })
}

/// The shape CMSG_GUILD_REMOVE, PROMOTE, DEMOTE and LEADER share: resolve `typed_name` against the
/// actor's own Guild roster, then run the Durable Request and map its outcome
/// (`cm:GuildHandler.cpp:146-152,290-296,343-349,467-473`).
///
/// No match, or two members sharing one name snapshot, sends guid 0 rather than answering locally.
/// mangos checks the actor's Rank Right before it looks the target up, so an unprivileged actor
/// hears its Refusal even when the typed name matches nobody; only the Module's Gate order can
/// reproduce that, since it alone knows the actor's Rank Rights. The Module's own membership lookup
/// then refuses guid 0 as `TargetNotInGuild`, which maps to the same reply this used to send here.
fn named_member_op<St, F>(
    store: &St,
    player: GuildActionPlayer,
    typed_name: String,
    op_kind: GuildOpKind,
    build_request: F,
) -> Result<Vec<Outbound>>
where
    St: GuildActionStore + ?Sized,
    F: FnOnce(u64) -> GuildRequest,
{
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let Some((_, guild)) = actor_guild(store, player)? else {
        return Ok(vec![not_in_guild()]);
    };
    let members = store.guild_members(guild.guild_id)?;
    let target_guid = match member_by_name(&members, &typed_name) {
        MemberMatch::Found(guid) => guid,
        MemberMatch::NotInGuild => 0,
    };
    let outcome = store.guild_op(actor_guid, build_request(target_guid))?;
    Ok(match outcome {
        GuildOutcome::Ran => Vec::new(),
        GuildOutcome::Refused(refusal) => {
            vec![membership_refusal_outbound(
                op_kind,
                refusal,
                &typed_name,
                &typed_name,
            )]
        }
    })
}

/// Which client opcode a Refusal reply is being built for: the same [`GuildRefusal`] wire-maps
/// differently depending on which op produced it.
#[derive(Clone, Copy, Debug)]
enum GuildOpKind {
    Invite,
    Remove,
    Promote,
    Demote,
    Leader,
    Leave,
    Disband,
}

/// GUILD_PERMISSIONS_OR_LEADER (0x08): both `NoPermission` (a missing Rank Right) and `NotLeader`
/// (the actor is not the Guild Leader) answer this exact reply.
fn no_permission() -> Outbound {
    command_result(
        GuildCommand::Invite,
        String::new(),
        GuildCommandResult::GuildPermissionsOrLeader,
    )
}

/// Every membership Refusal's wire reply, derived from `cm:GuildHandler.cpp`'s line-numbered
/// `SendGuildCommandResult` calls (cited on each [`GuildRefusal`] variant in `lyracore_shared`).
/// `typed_name` is the client's own input; `target_name` is the resolved target's name snapshot,
/// used only where mangos itself reads it back off the found Character rather than the packet.
fn membership_refusal_outbound(
    op: GuildOpKind,
    refusal: GuildRefusal,
    typed_name: &str,
    target_name: &str,
) -> Outbound {
    use GuildCommandResult::{
        AlreadyInGuildS, AlreadyInvitedToGuildS, GuildNameInvalid, GuildNotAllied,
        GuildPlayerNotInGuildS, GuildRankTooHighS, GuildRankTooLowS,
    };
    use GuildOpKind::{Demote, Invite, Leader, Leave, Promote, Remove};
    use GuildRefusal::{
        AlreadyInGuild, AlreadyInvited, LeaderCannotLeave, NoPermission, NotAllied, NotInGuild,
        NotLeader, RankTooHigh, RankTooLow, TargetIsSelf, TargetNotInGuild,
    };
    match (op, refusal) {
        (_, NotInGuild) => not_in_guild(),
        (_, NoPermission) | (_, NotLeader) => no_permission(),
        (Invite, NotAllied) => {
            command_result(GuildCommand::Invite, typed_name.to_string(), GuildNotAllied)
        }
        (Invite, AlreadyInGuild) => command_result(
            GuildCommand::Invite,
            target_name.to_string(),
            AlreadyInGuildS,
        ),
        (Invite, AlreadyInvited) => command_result(
            GuildCommand::Invite,
            target_name.to_string(),
            AlreadyInvitedToGuildS,
        ),
        (Remove | Promote | Demote | Leader, TargetNotInGuild) => command_result(
            GuildCommand::Invite,
            target_name.to_string(),
            GuildPlayerNotInGuildS,
        ),
        (Remove | Leave, LeaderCannotLeave) => command_result(
            GuildCommand::Quit,
            String::new(),
            GuildCommandResult::GuildPermissionsOrLeader,
        ),
        (Remove, RankTooHigh) => command_result(
            GuildCommand::Quit,
            target_name.to_string(),
            GuildRankTooHighS,
        ),
        (Promote | Demote, RankTooHigh) => command_result(
            GuildCommand::Invite,
            target_name.to_string(),
            GuildRankTooHighS,
        ),
        (Demote, RankTooLow) => command_result(
            GuildCommand::Invite,
            target_name.to_string(),
            GuildRankTooLowS,
        ),
        (Promote | Demote, TargetIsSelf) => {
            command_result(GuildCommand::Invite, String::new(), GuildNameInvalid)
        }
        (op, refusal) => {
            log::warn!("world: guild op {op:?} produced an unmapped refusal {refusal:?}");
            no_permission()
        }
    }
}

/// CMSG_GUILD_MOTD (`cm:GuildHandler.cpp:488-513`).
fn motd_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    text: String,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let outcome = store.guild_op(actor_guid, GuildRequest::SetMotd { text })?;
    Ok(match outcome {
        GuildOutcome::Ran => Vec::new(),
        GuildOutcome::Refused(GuildRefusal::NotInGuild) => vec![not_in_guild()],
        GuildOutcome::Refused(GuildRefusal::NoPermission) => vec![command_result(
            GuildCommand::Invite,
            String::new(),
            GuildCommandResult::GuildPermissionsOrLeader,
        )],
        GuildOutcome::Refused(_) => Vec::new(), // TooLong: the stock client cannot produce it.
    })
}

/// CMSG_GUILD_INFO_TEXT (`cm:GuildHandler.cpp:693-713`).
fn info_text_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    text: String,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let outcome = store.guild_op(actor_guid, GuildRequest::SetInfo { text })?;
    Ok(match outcome {
        GuildOutcome::Ran => Vec::new(),
        GuildOutcome::Refused(GuildRefusal::NotInGuild) => vec![not_in_guild()],
        GuildOutcome::Refused(GuildRefusal::NoPermission) => vec![command_result(
            GuildCommand::Create,
            String::new(),
            GuildCommandResult::GuildPermissionsOrLeader,
        )],
        GuildOutcome::Refused(_) => Vec::new(),
    })
}

/// CMSG_GUILD_SET_PUBLIC_NOTE / SET_OFFICER_NOTE, shared: resolve `target_name` against the
/// actor's own Guild through its member name snapshots (`member_by_name`), same as mangos'
/// `GetMemberSlot` (`cm:GuildHandler.cpp:526-545,564-582`), then run the edit.
///
/// No match sends guid 0 rather than answering locally: mangos checks the actor's Rank Right
/// before it looks the target up, so an actor without the right hears `NoPermission` even for a
/// name that matches nobody, and only the Module's own Gate order can reproduce that. The Module's
/// membership lookup then refuses guid 0 as `TargetNotInGuild`, which maps to the same reply this
/// used to send here.
fn set_note_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    target_name: String,
    text: String,
    request: impl FnOnce(u64, String) -> GuildRequest,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let Some((_, guild)) = actor_guild(store, player)? else {
        return Ok(vec![not_in_guild()]);
    };
    let members = store.guild_members(guild.guild_id)?;
    let target_guid = match member_by_name(&members, &target_name) {
        MemberMatch::Found(guid) => guid,
        MemberMatch::NotInGuild => 0,
    };
    let outcome = store.guild_op(actor_guid, request(target_guid, text))?;
    Ok(match outcome {
        GuildOutcome::Refused(GuildRefusal::NoPermission) => vec![command_result(
            GuildCommand::Invite,
            String::new(),
            GuildCommandResult::GuildPermissionsOrLeader,
        )],
        GuildOutcome::Refused(GuildRefusal::TargetNotInGuild) => vec![command_result(
            GuildCommand::Invite,
            target_name,
            GuildCommandResult::GuildPlayerNotInGuildS,
        )],
        _ => Vec::new(), // TooLong is silent: the stock client cannot produce it.
    })
}

/// The rank-management opcodes (RANK, ADD_RANK, DEL_RANK) share their refusal replies: `NotInGuild`
/// answers the same as every other guild-less request, `NotLeader` answers PERMISSIONS, and every
/// other refusal, including `RanksAtLimit`, is silent (`cm:GuildHandler.cpp:606-611,645-651,
/// 671-673`).
fn rank_refusal_outbound(outcome: GuildOutcome) -> Vec<Outbound> {
    match outcome {
        GuildOutcome::Refused(GuildRefusal::NotInGuild) => vec![not_in_guild()],
        GuildOutcome::Refused(GuildRefusal::NotLeader) => vec![command_result(
            GuildCommand::Invite,
            String::new(),
            GuildCommandResult::GuildPermissionsOrLeader,
        )],
        _ => Vec::new(),
    }
}

/// CMSG_GUILD_RANK.
fn edit_rank_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    rank_id: u32,
    rights: u32,
    name: String,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let outcome = store.guild_op(
        actor_guid,
        GuildRequest::EditRank {
            rank_id,
            rights,
            name,
        },
    )?;
    Ok(rank_refusal_outbound(outcome))
}

/// CMSG_GUILD_ADD_RANK.
fn add_rank_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    name: String,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let outcome = store.guild_op(actor_guid, GuildRequest::AddRank { name })?;
    Ok(rank_refusal_outbound(outcome))
}

/// CMSG_GUILD_DEL_RANK.
fn delete_rank_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let outcome = store.guild_op(actor_guid, GuildRequest::DeleteRank)?;
    Ok(rank_refusal_outbound(outcome))
}

const GUILD_CREATE_USAGE: &str = "usage: .guild create [<leader>] \"<name>\"";

/// Is this Say line a `.guild` dot-command?
pub(crate) fn is_guild_dot_command(text: &str) -> bool {
    text.strip_prefix(".guild")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// `.guild create [<leader>] "<name>"` (`cm:Level3.cpp:2945-2977`), GM level only. Without a
/// leader name the leader is the actor's selected player, else the actor. `Some(line)` is the
/// system line for the actor; success is silent, like every other dot-command.
pub(crate) fn run_guild_dot_command<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    text: &str,
) -> Result<Option<String>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(None);
    };
    let gm_level = store.guild_gm_level(actor_guid)?;
    if gm_level == 0 {
        return Ok(Some("permission denied".into()));
    }
    let Some((leader_name, guild_name)) = parse_guild_create(text) else {
        return Ok(Some(GUILD_CREATE_USAGE.into()));
    };
    let leader = match leader_name {
        Some(typed) => match store.guild_characters_named(typed)?.as_slice() {
            [] => return Ok(Some(format!("no player named {typed}"))),
            [guid] => store.guild_character_facts(*guid)?,
            _ => return Ok(Some(format!("more than one player named {typed}"))),
        },
        None => match store.guild_selected_target(actor_guid) {
            0 => store.guild_character_facts(actor_guid)?,
            target => match store.guild_character_facts(target)? {
                Some(selected) => Some(selected),
                None => store.guild_character_facts(actor_guid)?,
            },
        },
    };
    let Some(leader) = leader else {
        return Ok(Some(format!(
            "no player named {}",
            leader_name.unwrap_or_default()
        )));
    };
    let outcome = store.guild_op(
        actor_guid,
        gm_create(&leader, gm_level, guild_name.to_string()),
    );
    Ok(match outcome {
        Ok(GuildOutcome::Ran) => None,
        Ok(GuildOutcome::Refused(GuildRefusal::NotGameMaster)) => Some("permission denied".into()),
        Ok(GuildOutcome::Refused(GuildRefusal::AlreadyInGuild)) => {
            Some(format!("{} is already in a guild", leader.name))
        }
        Ok(GuildOutcome::Refused(_)) => Some("guild not created".into()),
        Err(error) if is_fatal(&error) => return Err(error),
        Err(error) => {
            log::warn!("world: .guild create by {actor_guid} failed: {error:#}");
            Some("guild not created".into())
        }
    })
}

/// Split `.guild create [<leader>] "<name>"` into the optional leader name and the quoted guild
/// name. `None` for any other shape.
fn parse_guild_create(text: &str) -> Option<(Option<&str>, &str)> {
    let rest = text.strip_prefix(".guild")?.trim_start();
    let (subcommand, rest) = rest.split_once(char::is_whitespace)?;
    if !subcommand.eq_ignore_ascii_case("create") {
        return None;
    }
    let rest = rest.trim();
    let (leader, quoted) = match rest.strip_prefix('"') {
        Some(_) => (None, rest),
        None => {
            let (leader, quoted) = rest.split_once(char::is_whitespace)?;
            (Some(leader), quoted.trim_start())
        }
    };
    let name = quoted.strip_prefix('"')?.strip_suffix('"')?;
    (!name.is_empty() && !name.contains('"')).then_some((leader, name))
}

/// MSG_TABARDVENDOR_ACTIVATE opens the tabard designer (`cm:NPCHandler.cpp:47-67`). This is a read
/// path, so only the faction refusal is answered here, silently. The NPC flag, reach and life Gates
/// run in the Fee Hold when the emblem is saved.
fn tabard_window_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    npc_guid: u64,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    if store.guild_npc_refuses(npc_guid, actor_guid)? {
        return Ok(Vec::new());
    }
    Ok(vec![tabard_designer_window(npc_guid)])
}

/// The tabard designer window for `npc_guid`. Gossip action 11 opens it too
/// (`cm:Player.cpp:11877-11880`).
pub(crate) fn tabard_designer_window(npc_guid: u64) -> Outbound {
    Outbound::One(ServerOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(
        MSG_TABARDVENDOR_ACTIVATE {
            guid: Guid::new(npc_guid),
        },
    ))
}

/// MSG_SAVE_GUILD_EMBLEM (`cm:GuildHandler.cpp:716-771`). Advisory reads of the Realm-core cache
/// answer NO_GUILD and NOT_GUILD_MASTER before any copper moves, and Realm-core decides again inside
/// the fee. mangos checks the NPC first. Here the NPC Gate runs in the Fee Hold, so a non-member at
/// the wrong NPC hears NO_GUILD. The new emblem reaches every online member, the payer included,
/// through the TABARD_CHANGED Guild Event.
fn save_emblem_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    save: &MSG_SAVE_GUILD_EMBLEM_Client,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let result = match store.guild_member(actor_guid)? {
        None => GuildEmblemResult::NoGuild,
        Some(member) if member.rank_id != LEADER_RANK => GuildEmblemResult::NotGuildMaster,
        Some(_) => {
            let request = guild_fee::FeeRequest::Emblem {
                npc_guid: save.vendor.guid(),
                emblem: guild_fee::Emblem {
                    emblem_style: save.emblem_style,
                    emblem_color: save.emblem_color,
                    border_style: save.border_style,
                    border_color: save.border_color,
                    background_color: save.background_color,
                },
            };
            emblem_result(guild_fee::pay(store, actor_guid, request)?)
        }
    };
    Ok(vec![Outbound::One(
        ServerOpcodeMessage::MSG_SAVE_GUILD_EMBLEM(MSG_SAVE_GUILD_EMBLEM_Server { result }),
    )])
}

fn emblem_result(outcome: guild_fee::FeeOutcome) -> GuildEmblemResult {
    use guild_fee::FeeOutcome::{Accepted, Refused};
    match outcome {
        Accepted => GuildEmblemResult::Success,
        Refused(GuildRefusal::NotInGuild) => GuildEmblemResult::NoGuild,
        Refused(GuildRefusal::NotLeader) => GuildEmblemResult::NotGuildMaster,
        Refused(GuildRefusal::NotEnoughMoney) => GuildEmblemResult::NotEnoughMoney,
        // The NPC Gate. mangos names the value INVALIDVENDOR; the client shows nothing for it
        // (`vm:src/game/Handlers/GuildHandler.cpp:686-693`).
        Refused(_) => GuildEmblemResult::NoMessage,
    }
}

/// The Guild Projection for `character_guid`: `(guild_id, rank_id)`, or `(0, 0)` outside a Guild or
/// when Realm-core cannot answer.
pub(crate) fn guild_projection<St: GuildActionStore + ?Sized>(
    store: &St,
    character_guid: u64,
) -> (u32, u32) {
    match store.guild_member(character_guid) {
        Ok(Some(member)) => (member.guild_id, member.rank_id),
        Ok(None) => (0, 0),
        Err(error) => {
            log::warn!("world: Guild Projection for {character_guid} unreadable: {error:#}");
            (0, 0)
        }
    }
}

/// The guild packets of world entry, sent after the viewer is registered. The self CREATE
/// carried `created`, the Guild Projection read before registration; a membership change that
/// landed in between reached no viewer, so a different current value goes out as a VALUES update,
/// `(0, 0)` after a removal. A fresh login of a member also gets the MOTD
/// (`cm:CharacterHandler.cpp:775-785`). A failed read logs and never fails the entry.
pub(crate) fn guild_world_entry<St: GuildActionStore + ?Sized>(
    store: &St,
    character_guid: u64,
    entry: codec::WorldEntry,
    created: (u32, u32),
) -> Vec<Outbound> {
    let member = match store.guild_member(character_guid) {
        Ok(member) => member,
        Err(error) => {
            log::warn!("world: guild entry for {character_guid} unreadable: {error:#}");
            return Vec::new();
        }
    };
    let (guild_id, rank_id) = member
        .as_ref()
        .map_or((0, 0), |member| (member.guild_id, member.rank_id));
    let mut outbound = Vec::new();
    if (guild_id, rank_id) != created {
        let (opcode, body) = codec::build_guild_values(character_guid, guild_id, rank_id);
        outbound.push(Outbound::Raw { opcode, body });
    }
    if member.is_none() || entry != codec::WorldEntry::FreshLogin {
        return outbound;
    }
    match store.guild(guild_id) {
        Ok(Some(guild)) => {
            let (opcode, body) = codec::build_guild_event_raw(event_kind::MOTD, &[guild.motd], 0);
            outbound.push(Outbound::Raw { opcode, body });
        }
        Ok(None) => {}
        Err(error) => log::warn!("world: guild MOTD for {character_guid} unreadable: {error:#}"),
    }
    outbound
}

/// A member entering the world fresh signs on, which tells the other online members. Answers
/// whether SIGNED_ON went out, so the World Session owes a sign-off whatever happens next.
pub(crate) fn guild_sign_on<St: GuildActionStore + ?Sized>(
    store: &St,
    character_guid: u64,
) -> bool {
    if !matches!(store.guild_member(character_guid), Ok(Some(_))) {
        return false;
    }
    let actor_name = store
        .guild_character_facts(character_guid)
        .ok()
        .flatten()
        .map(|facts| facts.name)
        .unwrap_or_default();
    run_best_effort(
        store,
        character_guid,
        GuildRequest::SignOn { actor_name },
        "sign-on",
    )
}

/// The guild part of leaving the world: a member signs off. Runs before the Account Claim is
/// released, while Realm-core still accepts the actor (`cm:WorldSession.cpp:714-724`).
pub(crate) fn guild_world_exit<St: GuildActionStore + ?Sized>(store: &St, character_guid: u64) {
    if matches!(store.guild_member(character_guid), Ok(Some(_))) {
        let _ = run_best_effort(store, character_guid, GuildRequest::SignOff, "sign-off");
    }
}

/// Run one guild op whose failure must not fail the caller. Answers whether it ran.
fn run_best_effort<St: GuildActionStore + ?Sized>(
    store: &St,
    character_guid: u64,
    request: GuildRequest,
    step: &str,
) -> bool {
    match store.guild_op(character_guid, request) {
        Ok(GuildOutcome::Ran) => true,
        Ok(GuildOutcome::Refused(refusal)) => {
            log::debug!("world: guild {step} for {character_guid} refused: {refusal:?}");
            false
        }
        Err(error) => {
            log::warn!("world: guild {step} for {character_guid} failed: {error:#}");
            false
        }
    }
}

/// Resolve a typed member name against a Guild's name snapshots, case-insensitively, as mangos
/// searches its member list (`cm:Guild.h:285-292`). No match and two matches both answer
/// [`MemberMatch::NotInGuild`], so a by-name op never picks one of two homonyms.
pub(crate) fn member_by_name(members: &[codec::GuildMemberView], name: &str) -> MemberMatch {
    let mut found = members
        .iter()
        .filter(|member| member.name.to_lowercase() == name.to_lowercase());
    match (found.next(), found.next()) {
        (Some(member), None) => MemberMatch::Found(member.character_guid),
        _ => MemberMatch::NotInGuild,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemberMatch {
    Found(u64),
    NotInGuild,
}

/// Guild data for one settings-changing Guild Event, read once and shared by every recipient's
/// job: the query response, and, for a roster-carrying kind, both roster encodings, so a viewer's
/// own job only has to pick which one its Guild Rank sees and never re-reads the Realm-core cache
/// or the World Shards. A relay is not a reply to a client request, so a Guild that is gone renders
/// nothing here rather than the `not_in_guild()` command result a live request would get.
#[derive(Clone, Default)]
pub(crate) struct GuildEventSnapshot {
    pub(crate) query: Option<ServerOpcodeMessage>,
    pub(crate) roster_hidden: Option<ServerOpcodeMessage>,
    pub(crate) roster_shown: Option<ServerOpcodeMessage>,
    pub(crate) rank_rights: Vec<(u32, u32)>,
}

impl GuildEventSnapshot {
    /// Read the Guild, and its roster too unless `kind` only needs the query response.
    pub(crate) fn build<St: GuildActionStore + ?Sized>(
        store: &St,
        guild_id: u32,
        kind: u8,
    ) -> Self {
        let Ok(Some(guild)) = store.guild(guild_id) else {
            return Self::default();
        };
        let needs_query = kind == event_kind::TABARD_CHANGED || kind == event_kind::ROSTER_REFRESH;
        let needs_roster =
            kind == event_kind::ROSTER_TO_ACTOR || kind == event_kind::ROSTER_REFRESH;
        let query = needs_query.then(|| {
            ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(Box::new(
                codec::build_guild_query_response(&guild),
            ))
        });
        if !needs_roster {
            return Self {
                query,
                ..Self::default()
            };
        }
        let Ok(lines) = roster_lines(store, guild_id) else {
            return Self {
                query,
                ..Self::default()
            };
        };
        Self {
            query,
            roster_hidden: Some(roster_packet(&guild, lines.iter().cloned(), false)),
            roster_shown: Some(roster_packet(&guild, lines.iter().cloned(), true)),
            rank_rights: guild
                .ranks
                .iter()
                .map(|rank| (rank.rank_id, rank.rights))
                .collect(),
        }
    }

    /// The query response, or nothing when the Guild is gone.
    pub(crate) fn query_response(&self) -> Option<ServerOpcodeMessage> {
        self.query.clone()
    }

    /// The roster `rank_id` sees: officer notes shown only with VIEWOFFNOTE, nothing when the
    /// Guild is gone or this snapshot carries no roster.
    pub(crate) fn roster_for_rank(&self, rank_id: u32) -> Option<ServerOpcodeMessage> {
        let rank_rights = self
            .rank_rights
            .iter()
            .find(|(id, _)| *id == rank_id)
            .map_or(0, |(_, rank_rights)| *rank_rights);
        if has_right(rank_rights, rights::VIEWOFFNOTE) {
            self.roster_shown.clone()
        } else {
            self.roster_hidden.clone()
        }
    }
}

fn now_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::guild::{founding_gate, name_key, DEFAULT_MOTD, DEFAULT_RANKS};
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        GuildMember, GuildMember_GuildMemberStatus, CMSG_GUILD_ADD_RANK, CMSG_GUILD_CREATE,
        CMSG_GUILD_INFO_TEXT, CMSG_GUILD_MOTD, CMSG_GUILD_QUERY, CMSG_GUILD_RANK,
        CMSG_GUILD_SET_OFFICER_NOTE, CMSG_GUILD_SET_PUBLIC_NOTE, CMSG_PING,
    };

    const GM: u64 = 5_090_001;
    const BOB: u64 = 5_090_002;
    const CAROL: u64 = 5_090_003;

    /// An in-memory Realm-core plus World Shards. `guild_op` applies the same Gates the Module
    /// applies, so outcomes are observable as rows.
    #[derive(Default)]
    struct InMemoryGuildActions {
        guilds: Mutex<Vec<codec::GuildView>>,
        members: Mutex<Vec<codec::GuildMemberView>>,
        characters: Vec<CharacterFacts>,
        /// GM levels as each actor's Home Shard holds them.
        gm_levels: Vec<(u64, u8)>,
        /// `(owner_guid, ignored_guid)`.
        ignored: Vec<(u64, u64)>,
        selected: u64,
        ops: Mutex<Vec<(u64, GuildRequest)>>,
        op_error: Option<String>,
        /// NPCs that refuse every actor by faction.
        refusing_npcs: Vec<u64>,
        /// Every fee request that reached the Home Shard.
        fee_requests: Mutex<Vec<guild_fee::FeeRequest>>,
        /// What the Home Shard answers a hold; `None` holds.
        hold_refusal: Option<GuildRefusal>,
        /// What Realm-core decides; `None` accepts.
        fee_refusal: Option<GuildRefusal>,
        /// What the next non-`GmCreate` op answers; `None` runs it. The Gate arithmetic for every
        /// op lives on the Module, proved by its own unit and durable tests; this seam only proves
        /// the wire mapping from a given outcome, the same shape `GuildFeeStore` below uses for the
        /// Fee Hold protocol.
        refuse_next: Mutex<Option<GuildRefusal>>,
    }

    impl InMemoryGuildActions {
        fn guild_named(&self, name: &str) -> Option<codec::GuildView> {
            self.guilds
                .lock()
                .unwrap()
                .iter()
                .find(|guild| guild.name == name)
                .cloned()
        }

        fn add_member(&self, guild_id: u32, character_guid: u64, rank_id: u32, note: &str) {
            self.members.lock().unwrap().push(codec::GuildMemberView {
                character_guid,
                guild_id,
                rank_id,
                name: format!("Snapshot{character_guid}"),
                public_note: note.into(),
                officer_note: format!("officer {note}"),
                realm_account_id: character_guid,
            });
        }

        /// Gate order after `cm:GuildHandler.cpp:46-64` and `founding_gate`.
        fn gm_create(&self, leader_guid: u64, gm_level: u8, name: String) -> Result<GuildOutcome> {
            let refused = |refusal: GuildRefusal| -> Result<GuildOutcome> {
                Ok(GuildOutcome::Refused(refusal))
            };
            if gm_level == 0 {
                return refused(GuildRefusal::NotGameMaster);
            }
            let leader_in_guild = self.guild_member(leader_guid)?.is_some();
            let mut guilds = self.guilds.lock().unwrap();
            let taken = |key: &str| guilds.iter().any(|guild| name_key(&guild.name) == key);
            if let Err(refusal) = founding_gate(&name, taken, leader_in_guild) {
                return refused(refusal);
            }
            let guild_id = guilds.len() as u32 + 1;
            guilds.push(codec::GuildView {
                guild_id,
                name,
                leader_guid,
                motd: DEFAULT_MOTD.into(),
                ranks: DEFAULT_RANKS
                    .iter()
                    .zip(0u32..)
                    .map(|((name, rights), rank_id)| codec::GuildRankView {
                        rank_id,
                        name: (*name).into(),
                        rights: *rights,
                    })
                    .collect(),
                ..codec::GuildView::default()
            });
            drop(guilds);
            self.add_member(guild_id, leader_guid, 0, "");
            Ok(GuildOutcome::Ran)
        }

        /// The next non-`GmCreate` `guild_op` call answers this refusal, so a test can pin the
        /// Gateway's reply mapping without reproducing the Module's own Gate in this Fake.
        fn refuse_next_op(&self, refusal: GuildRefusal) {
            *self.refuse_next.lock().unwrap() = Some(refusal);
        }
    }

    impl GuildActionStore for InMemoryGuildActions {
        fn guild_member(&self, character_guid: u64) -> Result<Option<codec::GuildMemberView>> {
            Ok(self
                .members
                .lock()
                .unwrap()
                .iter()
                .find(|member| member.character_guid == character_guid)
                .cloned())
        }

        fn guild(&self, guild_id: u32) -> Result<Option<codec::GuildView>> {
            Ok(self
                .guilds
                .lock()
                .unwrap()
                .iter()
                .find(|guild| guild.guild_id == guild_id)
                .cloned())
        }

        fn guild_members(&self, guild_id: u32) -> Result<Vec<codec::GuildMemberView>> {
            Ok(self
                .members
                .lock()
                .unwrap()
                .iter()
                .filter(|member| member.guild_id == guild_id)
                .cloned()
                .collect())
        }

        fn guild_character_facts(&self, character_guid: u64) -> Result<Option<CharacterFacts>> {
            Ok(self
                .characters
                .iter()
                .find(|facts| facts.guid == character_guid)
                .cloned())
        }

        fn guild_characters_named(&self, name: &str) -> Result<Vec<u64>> {
            Ok(self
                .characters
                .iter()
                .filter(|facts| facts.name.eq_ignore_ascii_case(name))
                .map(|facts| facts.guid)
                .collect())
        }

        fn guild_gm_level(&self, actor_guid: u64) -> Result<u8> {
            Ok(self
                .gm_levels
                .iter()
                .find(|(guid, _)| *guid == actor_guid)
                .map_or(0, |(_, level)| *level))
        }

        fn guild_selected_target(&self, _actor_guid: u64) -> u64 {
            self.selected
        }

        fn guild_ignored_by(&self, owner_guid: u64, other_guid: u64) -> Result<bool> {
            Ok(self
                .ignored
                .iter()
                .any(|(owner, ignored)| *owner == owner_guid && *ignored == other_guid))
        }

        fn guild_op(&self, actor_guid: u64, request: GuildRequest) -> Result<GuildOutcome> {
            self.ops.lock().unwrap().push((actor_guid, request.clone()));
            if let Some(error) = &self.op_error {
                return Err(anyhow!("{error}"));
            }
            // Every op but GmCreate is a Module Gate this Fake does not reproduce (proved by the
            // Module's own unit and durable tests instead): it answers the outcome a test scripts
            // through `refuse_next_op`, defaulting to `Ran`, so a seam test proves only the wire
            // mapping between a `GuildOutcome` and the client reply.
            if !matches!(request, GuildRequest::GmCreate { .. }) {
                if let Some(refusal) = self.refuse_next.lock().unwrap().take() {
                    return Ok(GuildOutcome::Refused(refusal));
                }
            }
            let GuildRequest::GmCreate {
                leader_guid,
                gm_level,
                name,
                ..
            } = request
            else {
                return Ok(GuildOutcome::Ran);
            };
            self.gm_create(leader_guid, gm_level, name)
        }

        fn guild_npc_refuses(&self, npc_guid: u64, _actor_guid: u64) -> Result<bool> {
            Ok(self.refusing_npcs.contains(&npc_guid))
        }
    }

    /// The fee answers the seam maps to result codes. `guild_fee`'s own tests drive the protocol.
    impl GuildFeeStore for InMemoryGuildActions {
        fn guild_fee_held(&self, _actor_guid: u64) -> Result<Option<guild_fee::FeeHold>> {
            Ok(None)
        }

        fn guild_fee_hold(
            &self,
            _actor_guid: u64,
            request: guild_fee::FeeRequest,
        ) -> Result<Result<guild_fee::FeeHold, GuildRefusal>> {
            self.fee_requests.lock().unwrap().push(request);
            if let Some(refusal) = self.hold_refusal {
                return Ok(Err(refusal));
            }
            let guild_fee::FeeRequest::Emblem { emblem, .. } = request;
            Ok(Ok(guild_fee::FeeHold {
                operation_id: 1,
                terms: guild_fee::FeeTerms::Emblem(emblem),
            }))
        }

        fn guild_fee_decide(
            &self,
            _actor_guid: u64,
            _hold: guild_fee::FeeHold,
        ) -> Result<guild_fee::FeeOutcome> {
            Ok(self.fee_refusal.map_or(
                guild_fee::FeeOutcome::Accepted,
                guild_fee::FeeOutcome::Refused,
            ))
        }

        fn guild_fee_finish(
            &self,
            _actor_guid: u64,
            _operation_id: u64,
            _accepted: bool,
        ) -> Result<()> {
            Ok(())
        }
    }

    fn facts(guid: u64, name: &str) -> CharacterFacts {
        CharacterFacts {
            guid,
            name: name.into(),
            race: 1,
            class: 1,
            level: 60,
            zone_id: 12,
            last_logout_micros: 0,
            online: true,
            realm_account_id: guid,
        }
    }

    fn realm() -> InMemoryGuildActions {
        InMemoryGuildActions {
            characters: vec![
                facts(GM, "Gamemaster"),
                facts(BOB, "Bob"),
                facts(CAROL, "Carol"),
            ],
            gm_levels: vec![(GM, 1)],
            ..InMemoryGuildActions::default()
        }
    }

    fn in_world(guid: u64) -> GuildActionPlayer {
        GuildActionPlayer {
            account_id: 7,
            self_guid: Some(guid),
        }
    }

    fn dispatch(
        store: &InMemoryGuildActions,
        player: GuildActionPlayer,
        msg: ClientOpcodeMessage,
    ) -> Vec<Outbound> {
        match dispatch_guild_action(store, player, msg).unwrap() {
            GuildActionOutcome::Handled { outbound } => outbound,
            GuildActionOutcome::PassThrough(msg) => panic!("{msg} was not handled"),
        }
    }

    fn only_message(outbound: Vec<Outbound>) -> ServerOpcodeMessage {
        match <[Outbound; 1]>::try_from(outbound) {
            Ok([Outbound::One(message)]) => message,
            _ => panic!("expected exactly one typed message"),
        }
    }

    fn command_result_of(outbound: Vec<Outbound>) -> (GuildCommand, GuildCommandResult) {
        match only_message(outbound) {
            ServerOpcodeMessage::SMSG_GUILD_COMMAND_RESULT(result) => {
                (result.command, result.result)
            }
            other => panic!("expected a command result, got {other}"),
        }
    }

    #[test]
    fn dot_create_founds_a_guild_led_by_the_gm_when_nothing_is_selected() {
        let store = realm();
        let line =
            run_guild_dot_command(&store, in_world(GM), ".guild create \"Tracer Guild\"").unwrap();
        assert_eq!(line, None);
        let guild = store.guild_named("Tracer Guild").expect("guild founded");
        assert_eq!(guild.leader_guid, GM);
        assert_eq!(guild.motd, "No message set.");
        assert_eq!(guild.ranks.len(), 5);
        assert_eq!(
            store
                .guild_member(GM)
                .unwrap()
                .map(|m| (m.guild_id, m.rank_id)),
            Some((guild.guild_id, 0))
        );
    }

    #[test]
    fn a_repeated_dot_create_of_the_same_name_answers_guild_not_created() {
        let store = realm();
        run_guild_dot_command(&store, in_world(GM), ".guild create \"Tracer Guild\"").unwrap();
        let line =
            run_guild_dot_command(&store, in_world(GM), ".guild create \"tracer guild\"").unwrap();
        assert_eq!(line.as_deref(), Some("guild not created"));
        assert_eq!(store.guilds.lock().unwrap().len(), 1);
        assert_eq!(store.members.lock().unwrap().len(), 1);
    }

    #[test]
    fn dot_create_with_a_taken_name_in_another_case_answers_guild_not_created() {
        let store = realm();
        run_guild_dot_command(&store, in_world(GM), ".guild create Bob \"Tracer Guild\"").unwrap();
        let line =
            run_guild_dot_command(&store, in_world(GM), ".guild create Carol \"tracer guild\"")
                .unwrap();
        assert_eq!(line.as_deref(), Some("guild not created"));
        assert_eq!(store.guilds.lock().unwrap().len(), 1);
        assert_eq!(store.guild_member(CAROL).unwrap(), None);
    }

    #[test]
    fn dot_create_names_an_offline_leader_found_on_any_shard() {
        let mut store = realm();
        store.characters[1].online = false;
        let line =
            run_guild_dot_command(&store, in_world(GM), ".guild create bob \"Knights\"").unwrap();
        assert_eq!(line, None);
        assert_eq!(store.guild_named("Knights").unwrap().leader_guid, BOB);
        let (_, request) = store.ops.lock().unwrap()[0].clone();
        assert_eq!(
            request,
            GuildRequest::GmCreate {
                leader_guid: BOB,
                leader_name: "Bob".into(),
                leader_team: lyracore_shared::faction::TEAM_ALLIANCE,
                leader_realm_account: BOB,
                gm_level: 1,
                name: "Knights".into(),
            }
        );
    }

    #[test]
    fn dot_create_for_a_leader_already_in_a_guild_names_the_leader() {
        let store = realm();
        store.add_member(9, BOB, 3, "");
        let line =
            run_guild_dot_command(&store, in_world(GM), ".guild create Bob \"Knights\"").unwrap();
        assert_eq!(line.as_deref(), Some("Bob is already in a guild"));
        assert!(store.guilds.lock().unwrap().is_empty());
    }

    #[test]
    fn dot_create_for_an_unknown_leader_answers_no_player_named() {
        let store = realm();
        let line =
            run_guild_dot_command(&store, in_world(GM), ".guild create Dave \"Knights\"").unwrap();
        assert_eq!(line.as_deref(), Some("no player named Dave"));
        assert!(store.ops.lock().unwrap().is_empty());
    }

    #[test]
    fn dot_create_without_a_name_uses_the_selected_player() {
        let mut store = realm();
        store.selected = CAROL;
        run_guild_dot_command(&store, in_world(GM), ".guild create \"Knights\"").unwrap();
        assert_eq!(store.guild_named("Knights").unwrap().leader_guid, CAROL);
    }

    #[test]
    fn dot_create_with_a_selected_creature_falls_back_to_the_actor() {
        let mut store = realm();
        store.selected = 0xF130_0000_0000_0042;
        run_guild_dot_command(&store, in_world(GM), ".guild create \"Knights\"").unwrap();
        assert_eq!(store.guild_named("Knights").unwrap().leader_guid, GM);
    }

    #[test]
    fn dot_create_from_gm_level_zero_is_denied_before_any_request() {
        let store = realm();
        let line =
            run_guild_dot_command(&store, in_world(BOB), ".guild create \"Knights\"").unwrap();
        assert_eq!(line.as_deref(), Some("permission denied"));
        assert!(store.ops.lock().unwrap().is_empty());
        assert!(store.guilds.lock().unwrap().is_empty());
    }

    #[test]
    fn malformed_dot_commands_answer_the_usage_line() {
        let store = realm();
        for text in [
            ".guild",
            ".guild create",
            ".guild create Knights",
            ".guild create Bob Knights",
            ".guild create \"\"",
            ".guild invite Bob",
        ] {
            assert_eq!(
                run_guild_dot_command(&store, in_world(GM), text)
                    .unwrap()
                    .as_deref(),
                Some(GUILD_CREATE_USAGE),
                "{text}"
            );
        }
        assert!(store.ops.lock().unwrap().is_empty());
    }

    #[test]
    fn only_the_guild_word_selects_the_guild_command() {
        assert!(is_guild_dot_command(".guild create \"A B\""));
        assert!(is_guild_dot_command(".guild"));
        assert!(!is_guild_dot_command(".guilds"));
        assert!(!is_guild_dot_command(".gm on"));
    }

    #[test]
    fn a_lost_reducer_transport_ends_the_session() {
        let mut store = realm();
        store.op_error = Some("realm_guild_op reducer transport disconnected: gone".into());
        let error = run_guild_dot_command(&store, in_world(GM), ".guild create \"Knights\"")
            .expect_err("fatal");
        assert!(error.to_string().contains("transport disconnected"));
    }

    #[test]
    fn guild_create_opcode_from_a_gm_founds_a_guild_and_replies_nothing() {
        let store = realm();
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_CREATE(Box::new(CMSG_GUILD_CREATE {
                guild_name: "Knights".into(),
            })),
        );
        assert!(outbound.is_empty());
        assert_eq!(store.guild_named("Knights").unwrap().leader_guid, GM);
    }

    #[test]
    fn guild_create_opcode_from_a_player_changes_nothing_and_sends_nothing() {
        let store = realm();
        let outbound = dispatch(
            &store,
            in_world(BOB),
            ClientOpcodeMessage::CMSG_GUILD_CREATE(Box::new(CMSG_GUILD_CREATE {
                guild_name: "Knights".into(),
            })),
        );
        assert!(outbound.is_empty());
        assert!(store.ops.lock().unwrap().is_empty());
        assert!(store.guilds.lock().unwrap().is_empty());
    }

    #[test]
    fn guild_query_answers_any_guild_at_character_select() {
        let store = realm();
        run_guild_dot_command(&store, in_world(GM), ".guild create \"Knights\"").unwrap();
        let character_select = GuildActionPlayer {
            account_id: 7,
            self_guid: None,
        };
        let message = only_message(dispatch(
            &store,
            character_select,
            ClientOpcodeMessage::CMSG_GUILD_QUERY(CMSG_GUILD_QUERY { guild_id: 1 }),
        ));
        let ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(response) = message else {
            panic!("expected a query response, got {message}");
        };
        assert_eq!(response.name, "Knights");
        assert_eq!(response.rank_names[0], "Guild Master");
        assert_eq!(response.rank_names[4], "Initiate");
        assert_eq!(response.rank_names[5], "");
    }

    #[test]
    fn guild_query_for_an_unknown_guild_answers_not_in_guild() {
        let store = realm();
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_QUERY(CMSG_GUILD_QUERY { guild_id: 404 }),
        );
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Create,
                GuildCommandResult::GuildPlayerNotInGuild
            )
        );
    }

    fn founded_with_members() -> InMemoryGuildActions {
        let mut store = realm();
        store.characters[2].online = false;
        // A day and a half ago: 1.5 * 86_400_000_000 micros.
        store.characters[2].last_logout_micros = now_micros() - 129_600_000_000;
        run_guild_dot_command(&store, in_world(GM), ".guild create \"Knights\"").unwrap();
        store.add_member(1, BOB, 3, "bob note");
        store.add_member(1, CAROL, 4, "carol note");
        store
    }

    fn roster_for(store: &InMemoryGuildActions, viewer: u64) -> Vec<GuildMember> {
        match only_message(dispatch(
            store,
            in_world(viewer),
            ClientOpcodeMessage::CMSG_GUILD_ROSTER,
        )) {
            ServerOpcodeMessage::SMSG_GUILD_ROSTER(roster) => roster.members,
            other => panic!("expected a roster, got {other}"),
        }
    }

    #[test]
    fn roster_lists_live_and_offline_members() {
        let store = founded_with_members();
        let members = roster_for(&store, GM);
        assert_eq!(members.len(), 3);
        let bob = members.iter().find(|m| m.guid.guid() == BOB).unwrap();
        assert_eq!(bob.name, "Bob");
        assert_eq!(bob.status, GuildMember_GuildMemberStatus::Online);
        assert_eq!(bob.rank, 3);
        let carol = members.iter().find(|m| m.guid.guid() == CAROL).unwrap();
        let GuildMember_GuildMemberStatus::Offline { time_offline } = carol.status else {
            panic!("Carol is offline");
        };
        assert!(
            (time_offline - 1.5).abs() < 0.001,
            "days offline: {time_offline}"
        );
    }

    #[test]
    fn officer_notes_reach_only_a_viewer_whose_rank_can_view_them() {
        let store = founded_with_members();
        let by_leader = roster_for(&store, GM);
        assert!(by_leader
            .iter()
            .any(|m| m.officer_note == "officer bob note"));
        let by_member = roster_for(&store, BOB);
        assert!(by_member.iter().all(|m| m.officer_note.is_empty()));
        assert!(by_member.iter().any(|m| m.public_note == "bob note"));
    }

    #[test]
    fn a_member_whose_shard_cannot_answer_lists_offline_under_its_snapshot() {
        let store = founded_with_members();
        store.add_member(1, 5_090_099, 4, "");
        let members = roster_for(&store, GM);
        let lost = members.iter().find(|m| m.guid.guid() == 5_090_099).unwrap();
        assert_eq!(lost.name, "Snapshot5090099");
        assert!(matches!(
            lost.status,
            GuildMember_GuildMemberStatus::Offline { .. }
        ));
    }

    #[test]
    fn roster_outside_a_guild_sends_nothing() {
        let store = realm();
        assert!(dispatch(
            &store,
            in_world(BOB),
            ClientOpcodeMessage::CMSG_GUILD_ROSTER
        )
        .is_empty());
    }

    #[test]
    fn roster_of_six_hundred_long_note_members_stays_under_the_client_limit() {
        let store = founded_with_members();
        for guid in 0..600 {
            store.add_member(1, 6_000_000 + guid, 4, &"n".repeat(31));
        }
        let message = ServerOpcodeMessage::SMSG_GUILD_ROSTER(Box::new(
            match only_message(dispatch(
                &store,
                in_world(GM),
                ClientOpcodeMessage::CMSG_GUILD_ROSTER,
            )) {
                ServerOpcodeMessage::SMSG_GUILD_ROSTER(roster) => *roster,
                other => panic!("expected a roster, got {other}"),
            },
        ));
        let mut wire = Vec::new();
        message.write_unencrypted_server(&mut wire).unwrap();
        assert!(wire.len() - 4 <= 0x8000 - 4);
    }

    #[test]
    fn guild_info_counts_members_and_distinct_accounts() {
        let store = founded_with_members();
        store.members.lock().unwrap()[2].realm_account_id = BOB;
        let message = only_message(dispatch(
            &store,
            in_world(BOB),
            ClientOpcodeMessage::CMSG_GUILD_INFO,
        ));
        let ServerOpcodeMessage::SMSG_GUILD_INFO(info) = message else {
            panic!("expected guild info, got {message}");
        };
        assert_eq!(info.guild_name, "Knights");
        assert_eq!(info.amount_of_characters_in_guild, 3);
        assert_eq!(info.amount_of_accounts_in_guild, 2);
    }

    #[test]
    fn guild_info_outside_a_guild_answers_not_in_guild() {
        let store = realm();
        let outbound = dispatch(&store, in_world(BOB), ClientOpcodeMessage::CMSG_GUILD_INFO);
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Create,
                GuildCommandResult::GuildPlayerNotInGuild
            )
        );
    }

    #[test]
    fn unrelated_opcode_passes_through() {
        let store = realm();
        let outcome = dispatch_guild_action(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_PING(CMSG_PING::default()),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            GuildActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_PING(_))
        ));
    }

    fn raw(outbound: Vec<Outbound>) -> Vec<(u16, Vec<u8>)> {
        outbound
            .into_iter()
            .map(|out| match out {
                Outbound::Raw { opcode, body } => (opcode, body),
                _ => panic!("guild entry packets are raw"),
            })
            .collect()
    }

    #[test]
    fn a_fresh_login_whose_create_matched_gets_only_the_motd() {
        let store = founded_with_members();
        let outbound = guild_world_entry(&store, BOB, codec::WorldEntry::FreshLogin, (1, 3));
        assert_eq!(
            raw(outbound),
            vec![codec::build_guild_event_raw(
                event_kind::MOTD,
                &["No message set.".into()],
                0
            )]
        );
    }

    #[test]
    fn a_membership_change_after_the_create_read_is_re_sent() {
        let store = founded_with_members();
        let joined = guild_world_entry(&store, BOB, codec::WorldEntry::WorldPort, (0, 0));
        assert_eq!(raw(joined), vec![codec::build_guild_values(BOB, 1, 3)]);
        let removed = guild_world_entry(&store, 5_090_404, codec::WorldEntry::WorldPort, (1, 3));
        assert_eq!(
            raw(removed),
            vec![codec::build_guild_values(5_090_404, 0, 0)]
        );
    }

    #[test]
    fn a_non_member_whose_create_matched_gets_nothing() {
        let store = founded_with_members();
        let outbound = guild_world_entry(&store, 5_090_404, codec::WorldEntry::FreshLogin, (0, 0));
        assert!(outbound.is_empty());
    }

    #[test]
    fn a_member_signs_on_and_reports_it() {
        let store = founded_with_members();
        assert!(guild_sign_on(&store, BOB));
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                BOB,
                GuildRequest::SignOn {
                    actor_name: "Bob".into()
                }
            ))
        );
        let before = store.ops.lock().unwrap().len();
        assert!(!guild_sign_on(&store, 5_090_404));
        assert_eq!(store.ops.lock().unwrap().len(), before);
    }

    #[test]
    fn a_failed_sign_on_owes_no_sign_off() {
        let mut store = founded_with_members();
        store.op_error = Some("realm_guild_op reducer transport disconnected: gone".into());
        assert!(!guild_sign_on(&store, BOB));
    }

    #[test]
    fn world_exit_signs_off_members_only() {
        let store = founded_with_members();
        guild_world_exit(&store, BOB);
        guild_world_exit(&store, 5_090_404);
        let ops = store.ops.lock().unwrap();
        assert_eq!(ops.last().cloned(), Some((BOB, GuildRequest::SignOff)));
        assert!(ops.iter().all(|(actor, _)| *actor != 5_090_404));
    }

    #[test]
    fn member_names_resolve_case_insensitively_and_homonyms_do_not_resolve() {
        let member = |guid, name: &str| codec::GuildMemberView {
            character_guid: guid,
            name: name.into(),
            ..codec::GuildMemberView::default()
        };
        let members = [member(1, "Bob"), member(2, "Alice"), member(3, "alice")];
        assert_eq!(member_by_name(&members, "BOB"), MemberMatch::Found(1));
        assert_eq!(member_by_name(&members, "Alice"), MemberMatch::NotInGuild);
        assert_eq!(member_by_name(&members, "Carol"), MemberMatch::NotInGuild);
    }

    const DESIGNER: u64 = 5_090_010;

    fn activate_tabard_vendor(
        store: &InMemoryGuildActions,
        player: GuildActionPlayer,
    ) -> Vec<Outbound> {
        dispatch(
            store,
            player,
            ClientOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(MSG_TABARDVENDOR_ACTIVATE {
                guid: Guid::new(DESIGNER),
            }),
        )
    }

    #[test]
    fn the_tabard_designer_window_opens_for_its_npc() {
        let store = realm();
        let message = only_message(activate_tabard_vendor(&store, in_world(BOB)));
        assert_eq!(
            message,
            ServerOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(MSG_TABARDVENDOR_ACTIVATE {
                guid: Guid::new(DESIGNER),
            })
        );
    }

    #[test]
    fn a_tabard_designer_that_refuses_by_faction_stays_silent() {
        let store = InMemoryGuildActions {
            refusing_npcs: vec![DESIGNER],
            ..realm()
        };
        assert!(activate_tabard_vendor(&store, in_world(BOB)).is_empty());
        let character_select = GuildActionPlayer {
            account_id: 7,
            self_guid: None,
        };
        assert!(activate_tabard_vendor(&realm(), character_select).is_empty());
    }

    const EMBLEM: guild_fee::Emblem = guild_fee::Emblem {
        emblem_style: 11,
        emblem_color: 12,
        border_style: 3,
        border_color: 14,
        background_color: 15,
    };

    fn save_emblem(store: &InMemoryGuildActions, actor: u64) -> GuildEmblemResult {
        let message = only_message(dispatch(
            store,
            in_world(actor),
            ClientOpcodeMessage::MSG_SAVE_GUILD_EMBLEM(Box::new(MSG_SAVE_GUILD_EMBLEM_Client {
                vendor: Guid::new(DESIGNER),
                emblem_style: EMBLEM.emblem_style,
                emblem_color: EMBLEM.emblem_color,
                border_style: EMBLEM.border_style,
                border_color: EMBLEM.border_color,
                background_color: EMBLEM.background_color,
            })),
        ));
        match message {
            ServerOpcodeMessage::MSG_SAVE_GUILD_EMBLEM(saved) => saved.result,
            other => panic!("expected an emblem result, got {other}"),
        }
    }

    /// Bob leads Knights; Carol is a member at rank 3.
    fn led_by_bob(
        hold_refusal: Option<GuildRefusal>,
        fee_refusal: Option<GuildRefusal>,
    ) -> InMemoryGuildActions {
        let store = InMemoryGuildActions {
            hold_refusal,
            fee_refusal,
            ..realm()
        };
        run_guild_dot_command(&store, in_world(GM), ".guild create Bob \"Knights\"").unwrap();
        store.add_member(1, CAROL, 3, "");
        store
    }

    #[test]
    fn the_guild_leader_pays_for_the_emblem_at_the_vendor_it_named() {
        let store = led_by_bob(None, None);
        assert_eq!(save_emblem(&store, BOB), GuildEmblemResult::Success);
        assert_eq!(
            *store.fee_requests.lock().unwrap(),
            vec![guild_fee::FeeRequest::Emblem {
                npc_guid: DESIGNER,
                emblem: EMBLEM,
            }]
        );
    }

    #[test]
    fn the_advisory_reads_answer_before_any_copper_moves() {
        let store = led_by_bob(None, None);
        assert_eq!(save_emblem(&store, GM), GuildEmblemResult::NoGuild);
        assert_eq!(
            save_emblem(&store, CAROL),
            GuildEmblemResult::NotGuildMaster
        );
        assert!(store.fee_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn hold_refusals_map_to_their_emblem_results() {
        for (refusal, result) in [
            (
                GuildRefusal::NotEnoughMoney,
                GuildEmblemResult::NotEnoughMoney,
            ),
            (GuildRefusal::NpcRefused, GuildEmblemResult::NoMessage),
        ] {
            let store = led_by_bob(Some(refusal), None);
            assert_eq!(save_emblem(&store, BOB), result, "{refusal:?}");
        }
    }

    #[test]
    fn realm_core_refusals_map_to_their_emblem_results() {
        for (refusal, result) in [
            (GuildRefusal::NotLeader, GuildEmblemResult::NotGuildMaster),
            (GuildRefusal::NotInGuild, GuildEmblemResult::NoGuild),
        ] {
            let store = led_by_bob(None, Some(refusal));
            assert_eq!(save_emblem(&store, BOB), result, "{refusal:?}");
        }
    }

    // Membership: invite, accept, decline, leave, remove, promote, demote, pass leadership,
    // disband. The Module's own unit and durable tests prove the Gate arithmetic; these seam
    // tests script an outcome through `refuse_next_op` and prove only the wire mapping.

    const DAVE: u64 = 5_090_004;

    fn horde_facts(guid: u64, name: &str) -> CharacterFacts {
        CharacterFacts {
            race: 2,
            ..facts(guid, name)
        }
    }

    /// `InMemoryGuildActions::add_member` snapshots every member's name as `Snapshot<guid>`
    /// regardless of its note; by-name ops must type that snapshot, not the Character's real name.
    fn snapshot_name(guid: u64) -> String {
        format!("Snapshot{guid}")
    }

    #[test]
    fn invite_reaches_a_live_target_with_both_teams_and_no_reply() {
        let mut store = founded_with_members();
        store.characters.push(facts(DAVE, "Dave"));
        let outbound = invite_outbound(&store, in_world(GM), "Dave".into()).unwrap();
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                GM,
                GuildRequest::Invite {
                    target_guid: DAVE,
                    actor_team: lyracore_shared::faction::TEAM_ALLIANCE,
                    target_team: lyracore_shared::faction::TEAM_ALLIANCE,
                    target_ignores_actor: false,
                }
            ))
        );
    }

    #[test]
    fn invite_of_an_unknown_name_answers_player_not_found() {
        let store = founded_with_members();
        let outbound = invite_outbound(&store, in_world(GM), "Nobody".into()).unwrap();
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Invite,
                GuildCommandResult::GuildPlayerNotFoundS
            )
        );
    }

    #[test]
    fn invite_of_an_offline_homonym_is_not_found() {
        let mut store = founded_with_members();
        let mut offline_dave = facts(DAVE, "Dave");
        offline_dave.online = false;
        store.characters.push(offline_dave);
        let outbound = invite_outbound(&store, in_world(GM), "Dave".into()).unwrap();
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Invite,
                GuildCommandResult::GuildPlayerNotFoundS
            )
        );
    }

    #[test]
    fn an_ignored_invite_conveys_the_ignore_verdict_and_replies_nothing() {
        let mut store = founded_with_members();
        store.characters.push(facts(DAVE, "Dave"));
        store.ignored.push((DAVE, GM));
        let outbound = invite_outbound(&store, in_world(GM), "Dave".into()).unwrap();
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                GM,
                GuildRequest::Invite {
                    target_guid: DAVE,
                    actor_team: lyracore_shared::faction::TEAM_ALLIANCE,
                    target_team: lyracore_shared::faction::TEAM_ALLIANCE,
                    target_ignores_actor: true,
                }
            ))
        );
    }

    #[test]
    fn invite_of_an_opposite_team_target_conveys_both_teams_and_maps_not_allied() {
        let mut store = founded_with_members();
        store.characters.push(horde_facts(DAVE, "Dave"));
        store.refuse_next_op(GuildRefusal::NotAllied);
        let outbound = invite_outbound(&store, in_world(GM), "dave".into()).unwrap();
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                GM,
                GuildRequest::Invite {
                    target_guid: DAVE,
                    actor_team: lyracore_shared::faction::TEAM_ALLIANCE,
                    target_team: lyracore_shared::faction::TEAM_HORDE,
                    target_ignores_actor: false,
                }
            ))
        );
        let ServerOpcodeMessage::SMSG_GUILD_COMMAND_RESULT(result) = only_message(outbound) else {
            panic!("expected a command result");
        };
        assert_eq!(result.command, GuildCommand::Invite);
        assert_eq!(result.string, "dave");
        assert_eq!(result.result, GuildCommandResult::GuildNotAllied);
    }

    #[test]
    fn invite_of_a_guilded_target_maps_already_in_guild_to_its_resolved_name() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::AlreadyInGuild);
        let outbound = invite_outbound(&store, in_world(GM), "bob".into()).unwrap();
        let ServerOpcodeMessage::SMSG_GUILD_COMMAND_RESULT(result) = only_message(outbound) else {
            panic!("expected a command result");
        };
        assert_eq!(result.string, "Bob");
        assert_eq!(result.result, GuildCommandResult::AlreadyInGuildS);
    }

    #[test]
    fn a_repeated_invite_maps_already_invited() {
        let mut store = founded_with_members();
        store.characters.push(facts(DAVE, "Dave"));
        store.refuse_next_op(GuildRefusal::AlreadyInvited);
        let outbound = invite_outbound(&store, in_world(GM), "Dave".into()).unwrap();
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Invite,
                GuildCommandResult::AlreadyInvitedToGuildS
            )
        );
    }

    #[test]
    fn invite_without_the_invite_right_answers_no_permission() {
        let mut store = founded_with_members();
        store.characters.push(facts(DAVE, "Dave"));
        store.refuse_next_op(GuildRefusal::NoPermission);
        let outbound = invite_outbound(&store, in_world(BOB), "Dave".into()).unwrap();
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Invite,
                GuildCommandResult::GuildPermissionsOrLeader
            )
        );
    }

    #[test]
    fn invite_from_outside_any_guild_answers_not_in_guild() {
        let mut store = founded_with_members();
        store.characters.push(facts(DAVE, "Dave"));
        store.refuse_next_op(GuildRefusal::NotInGuild);
        let outbound = invite_outbound(&store, in_world(DAVE), "Bob".into()).unwrap();
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Create,
                GuildCommandResult::GuildPlayerNotInGuild
            )
        );
    }

    #[test]
    fn accept_conveys_the_actors_name_and_team_and_replies_nothing() {
        let mut store = founded_with_members();
        store.characters.push(facts(DAVE, "Dave"));
        let outbound = accept_outbound(&store, in_world(DAVE)).unwrap();
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                DAVE,
                GuildRequest::Accept {
                    actor_name: "Dave".into(),
                    actor_team: lyracore_shared::faction::TEAM_ALLIANCE,
                }
            ))
        );
    }

    #[test]
    fn decline_conveys_the_actors_name_and_replies_nothing() {
        let mut store = founded_with_members();
        store.characters.push(facts(DAVE, "Dave"));
        let outbound = decline_outbound(&store, in_world(DAVE)).unwrap();
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                DAVE,
                GuildRequest::Decline {
                    actor_name: "Dave".into(),
                }
            ))
        );
    }

    #[test]
    fn leave_of_an_ordinary_member_replies_quit_success() {
        let store = founded_with_members();
        let outbound = leave_outbound(&store, in_world(BOB)).unwrap();
        assert_eq!(
            command_result_of(outbound),
            (GuildCommand::Quit, GuildCommandResult::PlayerNoMoreInGuild)
        );
    }

    #[test]
    fn leave_of_the_leader_with_company_answers_leader_cannot_leave() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::LeaderCannotLeave);
        let outbound = leave_outbound(&store, in_world(GM)).unwrap();
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Quit,
                GuildCommandResult::GuildPermissionsOrLeader
            )
        );
    }

    #[test]
    fn leave_of_a_lone_leader_disbands_silently() {
        let store = realm();
        run_guild_dot_command(&store, in_world(GM), ".guild create \"Solo\"").unwrap();
        let outbound = leave_outbound(&store, in_world(GM)).unwrap();
        assert!(outbound.is_empty());
    }

    #[test]
    fn leave_outside_a_guild_answers_not_in_guild() {
        let mut store = founded_with_members();
        store.characters.push(facts(DAVE, "Dave"));
        let outbound = leave_outbound(&store, in_world(DAVE)).unwrap();
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Create,
                GuildCommandResult::GuildPlayerNotInGuild
            )
        );
    }

    #[test]
    fn remove_of_an_unmatched_name_sends_guid_zero_and_maps_target_not_in_guild() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::TargetNotInGuild);
        let outbound = named_member_op(
            &store,
            in_world(GM),
            "Nobody".into(),
            GuildOpKind::Remove,
            |target_guid| GuildRequest::Remove { target_guid },
        )
        .unwrap();
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((GM, GuildRequest::Remove { target_guid: 0 }))
        );
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Invite,
                GuildCommandResult::GuildPlayerNotInGuildS
            )
        );
    }

    #[test]
    fn remove_succeeds_and_replies_nothing() {
        let store = founded_with_members();
        let outbound = named_member_op(
            &store,
            in_world(GM),
            snapshot_name(BOB),
            GuildOpKind::Remove,
            |target_guid| GuildRequest::Remove { target_guid },
        )
        .unwrap();
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((GM, GuildRequest::Remove { target_guid: BOB }))
        );
    }

    #[test]
    fn remove_without_the_remove_right_answers_no_permission() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::NoPermission);
        let outbound = named_member_op(
            &store,
            in_world(BOB),
            snapshot_name(CAROL),
            GuildOpKind::Remove,
            |target_guid| GuildRequest::Remove { target_guid },
        )
        .unwrap();
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Invite,
                GuildCommandResult::GuildPermissionsOrLeader
            )
        );
    }

    #[test]
    fn removing_the_leader_answers_leader_cannot_leave() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::LeaderCannotLeave);
        let outbound = named_member_op(
            &store,
            in_world(GM),
            snapshot_name(GM),
            GuildOpKind::Remove,
            |target_guid| GuildRequest::Remove { target_guid },
        )
        .unwrap();
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Quit,
                GuildCommandResult::GuildPermissionsOrLeader
            )
        );
    }

    #[test]
    fn removing_a_target_at_or_above_the_actors_rank_answers_rank_too_high() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::RankTooHigh);
        let outbound = named_member_op(
            &store,
            in_world(BOB),
            snapshot_name(CAROL),
            GuildOpKind::Remove,
            |target_guid| GuildRequest::Remove { target_guid },
        )
        .unwrap();
        assert_eq!(
            command_result_of(outbound),
            (GuildCommand::Quit, GuildCommandResult::GuildRankTooHighS)
        );
    }

    #[test]
    fn promote_succeeds_and_replies_nothing() {
        let store = founded_with_members();
        let outbound = named_member_op(
            &store,
            in_world(GM),
            snapshot_name(BOB),
            GuildOpKind::Promote,
            |target_guid| GuildRequest::Promote { target_guid },
        )
        .unwrap();
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((GM, GuildRequest::Promote { target_guid: BOB }))
        );
    }

    #[test]
    fn promoting_past_the_actors_reach_answers_rank_too_high() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::RankTooHigh);
        let outbound = named_member_op(
            &store,
            in_world(BOB),
            snapshot_name(CAROL),
            GuildOpKind::Promote,
            |target_guid| GuildRequest::Promote { target_guid },
        )
        .unwrap();
        assert_eq!(
            command_result_of(outbound),
            (GuildCommand::Invite, GuildCommandResult::GuildRankTooHighS)
        );
    }

    #[test]
    fn promoting_oneself_answers_target_is_self() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::TargetIsSelf);
        let outbound = named_member_op(
            &store,
            in_world(GM),
            snapshot_name(GM),
            GuildOpKind::Promote,
            |target_guid| GuildRequest::Promote { target_guid },
        )
        .unwrap();
        assert_eq!(
            command_result_of(outbound),
            (GuildCommand::Invite, GuildCommandResult::GuildNameInvalid)
        );
    }

    #[test]
    fn demote_succeeds_and_replies_nothing() {
        let store = founded_with_members();
        let outbound = named_member_op(
            &store,
            in_world(GM),
            snapshot_name(BOB),
            GuildOpKind::Demote,
            |target_guid| GuildRequest::Demote { target_guid },
        )
        .unwrap();
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((GM, GuildRequest::Demote { target_guid: BOB }))
        );
    }

    #[test]
    fn demoting_the_guilds_lowest_rank_answers_rank_too_low() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::RankTooLow);
        let outbound = named_member_op(
            &store,
            in_world(GM),
            snapshot_name(CAROL),
            GuildOpKind::Demote,
            |target_guid| GuildRequest::Demote { target_guid },
        )
        .unwrap();
        assert_eq!(
            command_result_of(outbound),
            (GuildCommand::Invite, GuildCommandResult::GuildRankTooLowS)
        );
    }

    #[test]
    fn leader_passes_leadership_and_replies_nothing() {
        let store = founded_with_members();
        let outbound = named_member_op(
            &store,
            in_world(GM),
            snapshot_name(BOB),
            GuildOpKind::Leader,
            |target_guid| GuildRequest::SetLeader { target_guid },
        )
        .unwrap();
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((GM, GuildRequest::SetLeader { target_guid: BOB }))
        );
    }

    #[test]
    fn leader_by_a_non_leader_answers_no_permission() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::NotLeader);
        let outbound = named_member_op(
            &store,
            in_world(BOB),
            snapshot_name(CAROL),
            GuildOpKind::Leader,
            |target_guid| GuildRequest::SetLeader { target_guid },
        )
        .unwrap();
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Invite,
                GuildCommandResult::GuildPermissionsOrLeader
            )
        );
    }

    #[test]
    fn leader_naming_the_current_leader_is_a_silent_no_op() {
        let store = founded_with_members();
        let outbound = named_member_op(
            &store,
            in_world(GM),
            snapshot_name(GM),
            GuildOpKind::Leader,
            |target_guid| GuildRequest::SetLeader { target_guid },
        )
        .unwrap();
        assert!(outbound.is_empty());
    }

    #[test]
    fn leader_of_an_unmatched_name_sends_guid_zero_and_maps_target_not_in_guild() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::TargetNotInGuild);
        let outbound = named_member_op(
            &store,
            in_world(GM),
            "Nobody".into(),
            GuildOpKind::Leader,
            |target_guid| GuildRequest::SetLeader { target_guid },
        )
        .unwrap();
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((GM, GuildRequest::SetLeader { target_guid: 0 }))
        );
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Invite,
                GuildCommandResult::GuildPlayerNotInGuildS
            )
        );
    }

    #[test]
    fn disband_by_the_leader_replies_nothing() {
        let store = founded_with_members();
        let outbound = disband_outbound(&store, in_world(GM)).unwrap();
        assert!(outbound.is_empty());
    }

    #[test]
    fn disband_by_a_non_leader_answers_no_permission() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::NotLeader);
        let outbound = disband_outbound(&store, in_world(BOB)).unwrap();
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Invite,
                GuildCommandResult::GuildPermissionsOrLeader
            )
        );
    }

    #[test]
    fn disband_outside_a_guild_answers_not_in_guild() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::NotInGuild);
        let outbound = disband_outbound(&store, in_world(DAVE)).unwrap();
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Create,
                GuildCommandResult::GuildPlayerNotInGuild
            )
        );
    }

    #[test]
    fn the_guild_invite_opcode_dispatches_to_invite_outbound() {
        let mut store = founded_with_members();
        store.characters.push(facts(DAVE, "Dave"));
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_INVITE(Box::new(
                wow_world_messages::vanilla::CMSG_GUILD_INVITE {
                    invited_player: "Dave".into(),
                },
            )),
        );
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                GM,
                GuildRequest::Invite {
                    target_guid: DAVE,
                    actor_team: lyracore_shared::faction::TEAM_ALLIANCE,
                    target_team: lyracore_shared::faction::TEAM_ALLIANCE,
                    target_ignores_actor: false,
                }
            ))
        );
    }

    #[test]
    fn guild_motd_opcode_runs_the_op_and_replies_nothing_on_success() {
        let store = founded_with_members();
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_MOTD(Box::new(CMSG_GUILD_MOTD {
                message_of_the_day: "Assemble!".into(),
            })),
        );
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                GM,
                GuildRequest::SetMotd {
                    text: "Assemble!".into()
                }
            ))
        );
    }

    #[test]
    fn guild_motd_opcode_without_the_right_answers_permissions_from_invite() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::NoPermission);
        let outbound = dispatch(
            &store,
            in_world(BOB),
            ClientOpcodeMessage::CMSG_GUILD_MOTD(Box::new(CMSG_GUILD_MOTD {
                message_of_the_day: "Nope".into(),
            })),
        );
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Invite,
                GuildCommandResult::GuildPermissionsOrLeader
            )
        );
    }

    #[test]
    fn guild_motd_opcode_outside_a_guild_answers_not_in_guild() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::NotInGuild);
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_MOTD(Box::new(CMSG_GUILD_MOTD {
                message_of_the_day: "Nope".into(),
            })),
        );
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Create,
                GuildCommandResult::GuildPlayerNotInGuild
            )
        );
    }

    #[test]
    fn guild_info_text_opcode_runs_the_op_and_replies_nothing_on_success() {
        let store = founded_with_members();
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_INFO_TEXT(Box::new(CMSG_GUILD_INFO_TEXT {
                guild_info: "About us".into(),
            })),
        );
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                GM,
                GuildRequest::SetInfo {
                    text: "About us".into()
                }
            ))
        );
    }

    #[test]
    fn guild_info_text_opcode_without_the_right_answers_permissions_from_create() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::NoPermission);
        let outbound = dispatch(
            &store,
            in_world(BOB),
            ClientOpcodeMessage::CMSG_GUILD_INFO_TEXT(Box::new(CMSG_GUILD_INFO_TEXT {
                guild_info: "Nope".into(),
            })),
        );
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Create,
                GuildCommandResult::GuildPermissionsOrLeader
            )
        );
    }

    #[test]
    fn guild_set_public_note_opcode_resolves_the_target_by_name_and_runs_the_op() {
        let store = founded_with_members();
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_SET_PUBLIC_NOTE(Box::new(CMSG_GUILD_SET_PUBLIC_NOTE {
                player_name: format!("Snapshot{BOB}"),
                note: "reliable".into(),
            })),
        );
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                GM,
                GuildRequest::SetPublicNote {
                    target_guid: BOB,
                    text: "reliable".into()
                }
            ))
        );
    }

    #[test]
    fn guild_set_officer_note_opcode_resolves_the_target_by_name_and_runs_the_op() {
        let store = founded_with_members();
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_SET_OFFICER_NOTE(Box::new(
                CMSG_GUILD_SET_OFFICER_NOTE {
                    player_name: format!("Snapshot{BOB}"),
                    note: "watch closely".into(),
                },
            )),
        );
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                GM,
                GuildRequest::SetOfficerNote {
                    target_guid: BOB,
                    text: "watch closely".into()
                }
            ))
        );
    }

    #[test]
    fn a_note_opcode_for_an_unknown_name_sends_guid_zero_and_answers_by_the_typed_name() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::TargetNotInGuild);
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_SET_PUBLIC_NOTE(Box::new(CMSG_GUILD_SET_PUBLIC_NOTE {
                player_name: "Dave".into(),
                note: "x".into(),
            })),
        );
        let ServerOpcodeMessage::SMSG_GUILD_COMMAND_RESULT(result) = only_message(outbound) else {
            panic!("expected a command result");
        };
        assert_eq!(result.command, GuildCommand::Invite);
        assert_eq!(result.string, "Dave");
        assert_eq!(result.result, GuildCommandResult::GuildPlayerNotInGuildS);
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                GM,
                GuildRequest::SetPublicNote {
                    target_guid: 0,
                    text: "x".into()
                }
            )),
            "an unresolved name still runs the op, at guid 0, so the Module's Gate order \
             checks the actor's Rank Right before it answers not-in-guild"
        );
    }

    #[test]
    fn guild_rank_opcode_runs_the_op_with_its_fields() {
        let store = founded_with_members();
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_RANK(Box::new(CMSG_GUILD_RANK {
                rank_id: 2,
                rights: 0x43,
                rank_name: "Veteran+".into(),
            })),
        );
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                GM,
                GuildRequest::EditRank {
                    rank_id: 2,
                    rights: 0x43,
                    name: "Veteran+".into()
                }
            ))
        );
    }

    #[test]
    fn guild_rank_opcode_from_a_non_leader_answers_permissions() {
        let store = founded_with_members();
        store.refuse_next_op(GuildRefusal::NotLeader);
        let outbound = dispatch(
            &store,
            in_world(BOB),
            ClientOpcodeMessage::CMSG_GUILD_RANK(Box::new(CMSG_GUILD_RANK {
                rank_id: 2,
                rights: 0,
                rank_name: "X".into(),
            })),
        );
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Invite,
                GuildCommandResult::GuildPermissionsOrLeader
            )
        );
    }

    #[test]
    fn guild_add_rank_opcode_runs_the_op_and_is_silent_at_the_limit() {
        let store = founded_with_members();
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_ADD_RANK(Box::new(CMSG_GUILD_ADD_RANK {
                rank_name: "Recruit".into(),
            })),
        );
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                GM,
                GuildRequest::AddRank {
                    name: "Recruit".into()
                }
            ))
        );

        store.refuse_next_op(GuildRefusal::RanksAtLimit);
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_ADD_RANK(Box::new(CMSG_GUILD_ADD_RANK {
                rank_name: "Overflow".into(),
            })),
        );
        assert!(outbound.is_empty(), "RanksAtLimit is silent");
    }

    #[test]
    fn guild_del_rank_opcode_runs_the_op_and_answers_permissions_for_a_non_leader() {
        let store = founded_with_members();
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_DEL_RANK,
        );
        assert!(outbound.is_empty());
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((GM, GuildRequest::DeleteRank))
        );

        store.refuse_next_op(GuildRefusal::NotLeader);
        let outbound = dispatch(
            &store,
            in_world(BOB),
            ClientOpcodeMessage::CMSG_GUILD_DEL_RANK,
        );
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Invite,
                GuildCommandResult::GuildPermissionsOrLeader
            )
        );
    }

    #[test]
    fn roster_to_actor_snapshot_carries_the_viewers_own_roster() {
        let store = founded_with_members();
        let snapshot = GuildEventSnapshot::build(&store, 1, event_kind::ROSTER_TO_ACTOR);
        let ServerOpcodeMessage::SMSG_GUILD_ROSTER(roster) =
            snapshot.roster_for_rank(0).expect("a roster")
        else {
            panic!("expected a roster");
        };
        assert_eq!(roster.members.len(), 3);
        assert!(roster
            .members
            .iter()
            .any(|m| m.officer_note == "officer bob note"));
        assert!(
            snapshot.query_response().is_none(),
            "ROSTER_TO_ACTOR carries no query response"
        );
    }

    #[test]
    fn roster_refresh_snapshot_gates_officer_notes_by_each_viewers_own_rank() {
        let store = founded_with_members();
        let snapshot = GuildEventSnapshot::build(&store, 1, event_kind::ROSTER_REFRESH);

        let ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(response) =
            snapshot.query_response().expect("a query response")
        else {
            panic!("expected a query response");
        };
        assert_eq!(response.name, "Knights");

        // Bob is rank 3 (Member), without VIEWOFFNOTE.
        let ServerOpcodeMessage::SMSG_GUILD_ROSTER(bob_roster) =
            snapshot.roster_for_rank(3).expect("a roster")
        else {
            panic!("expected a roster");
        };
        assert!(
            bob_roster.members.iter().all(|m| m.officer_note.is_empty()),
            "Bob's rank cannot view officer notes"
        );

        // The Guild Leader is rank 0, which holds every right.
        let ServerOpcodeMessage::SMSG_GUILD_ROSTER(leader_roster) =
            snapshot.roster_for_rank(0).expect("a roster")
        else {
            panic!("expected a roster");
        };
        assert!(
            leader_roster
                .members
                .iter()
                .any(|m| m.officer_note == "officer bob note"),
            "the leader's rank can view officer notes"
        );
    }

    #[test]
    fn a_query_only_kind_skips_the_roster_reads() {
        let store = founded_with_members();
        let snapshot = GuildEventSnapshot::build(&store, 1, event_kind::TABARD_CHANGED);
        assert!(snapshot.query_response().is_some());
        assert!(snapshot.roster_for_rank(0).is_none());
    }

    #[test]
    fn a_gone_guilds_snapshot_answers_nothing() {
        let store = founded_with_members();
        let snapshot = GuildEventSnapshot::build(&store, 404, event_kind::ROSTER_REFRESH);
        assert!(
            snapshot.query_response().is_none(),
            "a relay is not a reply, so a disbanded Guild's roster refresh answers nothing"
        );
        assert!(snapshot.roster_for_rank(0).is_none());
    }
}
