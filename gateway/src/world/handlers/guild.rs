//! Guild family: founding a guild and the two screens that read one back all enter through
//! `dispatch_guild_action`. Every guild read and request the world session needs lives on
//! `GuildActionStore`; the routing behind those three methods (realm-core versus the player's own
//! shard) is `world::guild`'s job, so nothing here knows how many databases there are.

use super::super::*;
use crate::world::guild::{GuildRoster, GuildView};
use wow_world_messages::vanilla::{
    GuildCommand, GuildCommandResult, GuildEvent as WireGuildEvent, SMSG_GUILD_EVENT,
};

/// The durable requests and reads the guild family needs, in the seam's own vocabulary so it can be
/// exercised without the broad `WorldStore`. Each is ALREADY ROUTED: the implementation picks the
/// database, and the dispatcher below only decides what to say to the client.
pub(crate) trait GuildActionStore: Send + Sync {
    /// Found a guild named `name` with `self_guid` as its master. The module owns every gate, so an
    /// `Err` here is a gameplay answer carrying one of `lyracore_shared::guild::err`'s strings.
    fn guild_create(&self, account_id: u64, self_guid: u64, name: &str) -> Result<()>;

    /// The guild `guild_id` names. `None` = no such guild (a stale id the client still holds).
    fn guild_view(&self, guild_id: u64) -> Result<Option<GuildView>>;

    /// The guild `character_guid` belongs to. `None` = guildless.
    fn guild_of(&self, character_guid: u64) -> Result<Option<u64>>;

    /// Invite the character named `name` into the caller's guild. The name is resolved realm-wide
    /// by the implementation, so an `Err` here is either a gate's refusal (one of
    /// `lyracore_shared::guild::err`'s strings) or a lost transport.
    fn guild_invite(&self, account_id: u64, self_guid: u64, name: &str) -> Result<()>;

    /// Answer the caller's own pending invite: join the guild, or refuse and tell the inviter.
    fn guild_answer_invite(&self, account_id: u64, self_guid: u64, accept: bool) -> Result<()>;

    /// `guild_id`'s roster, ALREADY RENDERED: the authority's guids, ranks and notes with each
    /// member's name, level, class, area and online flag filled in from the shards. `None` = no
    /// such guild. The fan-out belongs to the routing layer, so nothing here knows how many
    /// databases answered.
    fn guild_roster(&self, guild_id: u64) -> Result<Option<GuildRoster>>;
    /// Remove the caller from their own guild. Refused for a master with members left behind: the
    /// module owns that gate, and answers with `lyracore_shared::guild::err`'s strings.
    fn guild_leave(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// Remove `target_name` from the caller's guild. Master only, and the name is resolved against
    /// the guild's own roster, so a homonym on another shard cannot be kicked by accident.
    fn guild_remove(&self, account_id: u64, self_guid: u64, target_name: &str) -> Result<()>;

    /// Destroy the caller's guild and every row that belongs to it. Master only.
    fn guild_disband(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// Hand the caller's guild to `target_name`. Master only.
    fn guild_set_master(&self, account_id: u64, self_guid: u64, target_name: &str) -> Result<()>;
    /// Deliver `/g` line `text` from `self_guid` through the guild-chat relay. `Err` carries one of
    /// `lyracore_shared::guild::err`'s strings (today, only `NOT_IN_GUILD`) or a transport failure;
    /// the caller decides what to say to the client either way.
    fn guild_chat(&self, self_guid: u64, text: &str) -> Result<()>;
    /// Set `self_guid`'s guild's MOTD to `motd` (master-only; an empty value clears it).
    fn guild_set_motd(&self, account_id: u64, self_guid: u64, motd: &str) -> Result<()>;

    /// Set `self_guid`'s guild's info text to `text` — same gate and shape as
    /// [`Self::guild_set_motd`], no broadcast.
    fn guild_set_info_text(&self, account_id: u64, self_guid: u64, text: &str) -> Result<()>;

    /// Set `target_name`'s public note to `note`. `target_name` is the wire's typed name — the
    /// implementation resolves it (realm-wide, like an invite target), so an unresolved name is a
    /// distinct refusal from every module-side gate. `self_guid` must be `target_name` itself (a
    /// member may set their OWN note) or the guild master.
    fn guild_set_public_note(
        &self,
        account_id: u64,
        self_guid: u64,
        target_name: &str,
        note: &str,
    ) -> Result<()>;

    /// Set `target_name`'s officer note to `note` — same resolution as
    /// [`Self::guild_set_public_note`], but master-only.
    fn guild_set_officer_note(
        &self,
        account_id: u64,
        self_guid: u64,
        target_name: &str,
        note: &str,
    ) -> Result<()>;
}

impl GuildActionStore for crate::stdb::Coordinator {
    fn guild_create(&self, account_id: u64, self_guid: u64, name: &str) -> Result<()> {
        crate::world::guild::run(
            self,
            account_id,
            self_guid,
            crate::world::guild::Op::Create(name.to_string()),
        )
    }

    fn guild_view(&self, guild_id: u64) -> Result<Option<GuildView>> {
        crate::world::guild::view(self, guild_id)
    }

    fn guild_of(&self, character_guid: u64) -> Result<Option<u64>> {
        crate::world::guild::guild_of(self, character_guid)
    }

    fn guild_invite(&self, account_id: u64, self_guid: u64, name: &str) -> Result<()> {
        crate::world::guild::invite(self, account_id, self_guid, name)
    }

    fn guild_answer_invite(&self, account_id: u64, self_guid: u64, accept: bool) -> Result<()> {
        crate::world::guild::answer_invite(self, account_id, self_guid, accept)
    }

    fn guild_roster(&self, guild_id: u64) -> Result<Option<GuildRoster>> {
        crate::world::guild::roster(self, guild_id)
    }

    fn guild_leave(&self, account_id: u64, self_guid: u64) -> Result<()> {
        let actor_name = crate::world::guild::own_name(self, self_guid)?;
        crate::world::guild::run(
            self,
            account_id,
            self_guid,
            crate::world::guild::Op::Leave { actor_name },
        )
    }

    fn guild_remove(&self, account_id: u64, self_guid: u64, target_name: &str) -> Result<()> {
        crate::world::guild::run(
            self,
            account_id,
            self_guid,
            crate::world::guild::Op::Remove {
                target_guid: crate::world::guild::member_by_name(self, self_guid, target_name)?,
                target_name: target_name.to_string(),
            },
        )
    }

    fn guild_set_motd(&self, account_id: u64, self_guid: u64, motd: &str) -> Result<()> {
        crate::world::guild::run(
            self,
            account_id,
            self_guid,
            crate::world::guild::Op::SetMotd(motd.to_string()),
        )
    }

    fn guild_set_info_text(&self, account_id: u64, self_guid: u64, text: &str) -> Result<()> {
        crate::world::guild::run(
            self,
            account_id,
            self_guid,
            crate::world::guild::Op::SetInfoText(text.to_string()),
        )
    }

    fn guild_set_public_note(
        &self,
        account_id: u64,
        self_guid: u64,
        target_name: &str,
        note: &str,
    ) -> Result<()> {
        crate::world::guild::run(
            self,
            account_id,
            self_guid,
            crate::world::guild::Op::SetPublicNote {
                target_guid: crate::world::guild::member_by_name(self, self_guid, target_name)?,
                note: note.to_string(),
            },
        )
    }

    fn guild_set_officer_note(
        &self,
        account_id: u64,
        self_guid: u64,
        target_name: &str,
        note: &str,
    ) -> Result<()> {
        crate::world::guild::run(
            self,
            account_id,
            self_guid,
            crate::world::guild::Op::SetOfficerNote {
                target_guid: crate::world::guild::member_by_name(self, self_guid, target_name)?,
                note: note.to_string(),
            },
        )
    }

    fn guild_disband(&self, account_id: u64, self_guid: u64) -> Result<()> {
        crate::world::guild::run(
            self,
            account_id,
            self_guid,
            crate::world::guild::Op::Disband,
        )
    }

    fn guild_set_master(&self, account_id: u64, self_guid: u64, target_name: &str) -> Result<()> {
        let target_guid = crate::world::guild::member_by_name(self, self_guid, target_name)?;
        crate::world::guild::run(
            self,
            account_id,
            self_guid,
            crate::world::guild::Op::Leader {
                target_guid,
                target_name: target_name.to_string(),
            },
        )
    }

    fn guild_chat(&self, self_guid: u64, text: &str) -> Result<()> {
        crate::world::guild::send_chat(self, self_guid, text.to_string())
    }
}

/// Who is asking. `self_guid` is `None` before world entry — every guild opcode names the acting
/// character, so those pass through instead of being evaluated against a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GuildActionPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
}

pub(crate) enum GuildActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuildActionErrorClass {
    GameplayRefusal,
    Fatal,
}

/// A lost reducer transport is the one guild failure a session cannot continue through; everything
/// else is the module answering "no". Copied verbatim from the item/quest seams so the whole
/// gateway classifies one way.
fn classify_guild_action_error(error: &anyhow::Error) -> GuildActionErrorClass {
    if error
        .chain()
        .any(|cause| cause.to_string().contains("reducer transport disconnected"))
    {
        GuildActionErrorClass::Fatal
    } else {
        GuildActionErrorClass::GameplayRefusal
    }
}

/// Map a create refusal onto the wire's own result code. The strings are the shared contract, so a
/// reword module-side is a compile error rather than a silent fall-through to the internal code.
fn create_result_for(error: &anyhow::Error) -> GuildCommandResult {
    use lyracore_shared::guild::err;
    let text = format!("{error:#}");
    if text.contains(err::ALREADY_IN_GUILD) {
        GuildCommandResult::AlreadyInGuild
    } else if text.contains(err::NAME_TAKEN) {
        GuildCommandResult::GuildNameExistsS
    } else if text.contains(err::NAME_INVALID) {
        GuildCommandResult::GuildNameInvalid
    } else {
        GuildCommandResult::GuildInternal
    }
}

/// Map a teardown refusal onto the wire's own result code — leave, kick, disband and the leadership
/// transfer all share these four, because they share the same two gates.
///
/// `GuildPermissionsOrLeader` is wire code 0x08, which vanilla uses for both "you lack the rights"
/// and "the leader cannot just leave". That is exactly the pair of refusals folded into it here.
fn teardown_result_for(error: &anyhow::Error) -> GuildCommandResult {
    use lyracore_shared::guild::err;
    let text = format!("{error:#}");
    if text.contains(err::TARGET_NOT_IN_GUILD) {
        GuildCommandResult::GuildPlayerNotInGuildS
    } else if text.contains(err::NOT_IN_GUILD) {
        GuildCommandResult::GuildPlayerNotInGuild
    } else if text.contains(err::NOT_GUILD_MASTER)
        || text.contains(err::MASTER_MUST_TRANSFER_OR_DISBAND)
        || text.contains(err::CANNOT_REMOVE_SELF)
    {
        GuildCommandResult::GuildPermissionsOrLeader
    } else {
        GuildCommandResult::GuildInternal
    }
}

/// Map an MOTD/info-text/note refusal onto the wire's own result code, off the same
/// classified-string contract [`create_result_for`] reads.
///
/// A note names its target the way a kick does — through `world::guild::member_by_name`, so a
/// homonym on another shard cannot be annotated by accident — and refuses with the same
/// `TARGET_NOT_IN_GUILD`. The client renders it as "no player named X", which from the setter's
/// side is what "not a member of your guild" means.
fn setter_result_for(error: &anyhow::Error) -> GuildCommandResult {
    use lyracore_shared::guild::err;
    let text = format!("{error:#}");
    if text.contains(err::NOT_GUILD_MASTER) {
        GuildCommandResult::GuildPermissionsOrLeader
    } else if text.contains(err::TARGET_NOT_IN_GUILD) {
        GuildCommandResult::GuildPlayerNotFoundS
    } else if text.contains(err::NOT_IN_GUILD) {
        GuildCommandResult::GuildPlayerNotInGuild
    } else {
        GuildCommandResult::GuildInternal
    }
}

/// One `SMSG_GUILD_COMMAND_RESULT`, the family's only error channel.
fn command_result(
    command: GuildCommand,
    subject: &str,
    result: GuildCommandResult,
) -> Vec<Outbound> {
    vec![Outbound::One(
        ServerOpcodeMessage::SMSG_GUILD_COMMAND_RESULT(Box::new(
            codec::build_guild_command_result(command, subject, result),
        )),
    )]
}

pub(crate) fn dispatch_guild_action<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<GuildActionOutcome> {
    let Some(self_guid) = player.self_guid else {
        return Ok(GuildActionOutcome::PassThrough(msg));
    };
    match msg {
        ClientOpcodeMessage::CMSG_GUILD_CREATE(s) => {
            guild_create(store, player.account_id, self_guid, &s.guild_name)
        }
        ClientOpcodeMessage::CMSG_GUILD_QUERY(s) => guild_query(store, u64::from(s.guild_id)),
        ClientOpcodeMessage::CMSG_GUILD_INFO => guild_info(store, self_guid),
        ClientOpcodeMessage::CMSG_GUILD_INVITE(s) => {
            guild_invite(store, player.account_id, self_guid, &s.invited_player)
        }
        ClientOpcodeMessage::CMSG_GUILD_ACCEPT => {
            guild_answer_invite(store, player.account_id, self_guid, true)
        }
        ClientOpcodeMessage::CMSG_GUILD_DECLINE => {
            guild_answer_invite(store, player.account_id, self_guid, false)
        }
        ClientOpcodeMessage::CMSG_GUILD_ROSTER => guild_roster(store, self_guid),
        ClientOpcodeMessage::CMSG_GUILD_LEAVE => guild_leave(store, player.account_id, self_guid),
        ClientOpcodeMessage::CMSG_GUILD_REMOVE(s) => {
            guild_remove(store, player.account_id, self_guid, &s.player_name)
        }
        ClientOpcodeMessage::CMSG_GUILD_DISBAND => {
            guild_disband(store, player.account_id, self_guid)
        }
        ClientOpcodeMessage::CMSG_GUILD_LEADER(s) => guild_leader(
            store,
            player.account_id,
            self_guid,
            &s.new_guild_leader_name,
        ),
        ClientOpcodeMessage::CMSG_GUILD_MOTD(s) => {
            guild_set_motd(store, player.account_id, self_guid, &s.message_of_the_day)
        }
        ClientOpcodeMessage::CMSG_GUILD_INFO_TEXT(s) => {
            guild_set_info_text(store, player.account_id, self_guid, &s.guild_info)
        }
        ClientOpcodeMessage::CMSG_GUILD_SET_PUBLIC_NOTE(s) => {
            guild_set_public_note(store, player.account_id, self_guid, &s.player_name, &s.note)
        }
        ClientOpcodeMessage::CMSG_GUILD_SET_OFFICER_NOTE(s) => {
            guild_set_officer_note(store, player.account_id, self_guid, &s.player_name, &s.note)
        }
        other => Ok(GuildActionOutcome::PassThrough(other)),
    }
}

/// `CMSG_GUILD_CREATE`. The reply is the wire's own verdict either way, because a client that hears
/// nothing leaves its create dialog open forever. Success is result code 0 — the wire spells it
/// `PlayerNoMoreInGuild`, but vanilla's own name for that code is "no message/error", and the client
/// re-queries the guild off the back of it.
fn guild_create<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    name: &str,
) -> Result<GuildActionOutcome> {
    let success = command_result(
        GuildCommand::Create,
        name,
        GuildCommandResult::PlayerNoMoreInGuild,
    );
    write_verb(
        GuildCommand::Create,
        name,
        "create",
        create_result_for,
        success,
        || store.guild_create(account_id, self_guid, name),
    )
}

/// `CMSG_GUILD_MOTD`. Master-only (D3); an empty string is a valid write that CLEARS the MOTD, not a
/// refusal. The caller — themselves an online member — hears `SMSG_GUILD_EVENT(Motd)` directly; every
/// OTHER member is reached by the `game_guild_event` relay, off the rows the module drops in the
/// same transaction.
fn guild_set_motd<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    motd: &str,
) -> Result<GuildActionOutcome> {
    let success = vec![Outbound::One(ServerOpcodeMessage::SMSG_GUILD_EVENT(
        Box::new(SMSG_GUILD_EVENT {
            event: WireGuildEvent::Motd,
            event_descriptions: vec![motd.to_string()],
        }),
    ))];
    setter("", "motd", success, || {
        store.guild_set_motd(account_id, self_guid, motd)
    })
}

/// `CMSG_GUILD_INFO_TEXT`. Same gate and shape as [`guild_set_motd`], but no broadcast on success —
/// an empty outbound batch, like the read-only handlers answer an absent guild.
fn guild_set_info_text<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    text: &str,
) -> Result<GuildActionOutcome> {
    setter("", "info text", Vec::new(), || {
        store.guild_set_info_text(account_id, self_guid, text)
    })
}

/// `CMSG_GUILD_SET_PUBLIC_NOTE`. The wire carries the target by NAME, resolved against the caller's
/// own roster the way a kick's is. A member may set their own row; setting anyone else's is
/// master-only (D3). No broadcast on success.
fn guild_set_public_note<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    target_name: &str,
    note: &str,
) -> Result<GuildActionOutcome> {
    setter(target_name, "public note", Vec::new(), || {
        store.guild_set_public_note(account_id, self_guid, target_name, note)
    })
}

/// `CMSG_GUILD_SET_OFFICER_NOTE`. Same name resolution as [`guild_set_public_note`], but master-only
/// with no member exception — proper officer-rank gating is a separate future issue (D3).
fn guild_set_officer_note<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    target_name: &str,
    note: &str,
) -> Result<GuildActionOutcome> {
    setter(target_name, "officer note", Vec::new(), || {
        store.guild_set_officer_note(account_id, self_guid, target_name, note)
    })
}

/// `CMSG_GUILD_QUERY`. An unknown id answers with nothing: the client asks about guilds it has seen
/// on other players' name plates, and one that has since disbanded is not an error.
fn guild_query<St: GuildActionStore + ?Sized>(
    store: &St,
    guild_id: u64,
) -> Result<GuildActionOutcome> {
    let Some(view) = store.guild_view(guild_id)? else {
        return Ok(GuildActionOutcome::Handled {
            outbound: Vec::new(),
        });
    };
    Ok(GuildActionOutcome::Handled {
        outbound: vec![Outbound::One(
            ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(Box::new(
                codec::build_guild_query_response(view.guild_id, &view.name, &view.rank_names),
            )),
        )],
    })
}

/// `CMSG_GUILD_INFO` — the caller's OWN guild's information panel. Guildless answers with nothing;
/// the client only opens the panel from inside a guild.
fn guild_info<St: GuildActionStore + ?Sized>(
    store: &St,
    self_guid: u64,
) -> Result<GuildActionOutcome> {
    let view = match store.guild_of(self_guid)? {
        Some(guild_id) => store.guild_view(guild_id)?,
        None => None,
    };
    let Some(view) = view else {
        return Ok(GuildActionOutcome::Handled {
            outbound: Vec::new(),
        });
    };
    Ok(GuildActionOutcome::Handled {
        outbound: vec![Outbound::One(ServerOpcodeMessage::SMSG_GUILD_INFO(
            Box::new(codec::build_guild_info(
                &view.name,
                view.created_micros,
                view.member_count,
            )),
        ))],
    })
}

/// Map an invite refusal onto the wire's own result code. Same shape as [`create_result_for`], and
/// the strings are the same shared contract — the five gates the ticket's invite flow names, plus
/// the internal default for anything the module has not told the gateway about.
fn invite_result_for(error: &anyhow::Error) -> GuildCommandResult {
    use lyracore_shared::guild::err;
    let text = format!("{error:#}");
    if text.contains(err::NOT_IN_GUILD) {
        GuildCommandResult::GuildPlayerNotInGuild
    } else if text.contains(err::NOT_GUILD_MASTER) {
        GuildCommandResult::GuildPermissionsOrLeader
    } else if text.contains(err::TARGET_NOT_FOUND) {
        GuildCommandResult::GuildPlayerNotFoundS
    } else if text.contains(err::TARGET_IN_GUILD) {
        GuildCommandResult::AlreadyInGuildS
    } else if text.contains(err::ALREADY_INVITED) {
        GuildCommandResult::AlreadyInvitedToGuildS
    } else {
        GuildCommandResult::GuildInternal
    }
}

/// `CMSG_GUILD_INVITE`. The client interpolates `name` into whichever line the result code selects,
/// so the typed name is echoed back verbatim — including on the refusal that says nobody has it.
fn guild_invite<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    name: &str,
) -> Result<GuildActionOutcome> {
    let success = command_result(
        GuildCommand::Invite,
        name,
        GuildCommandResult::PlayerNoMoreInGuild,
    );
    write_verb(
        GuildCommand::Invite,
        name,
        "invite",
        invite_result_for,
        success,
        || store.guild_invite(account_id, self_guid, name),
    )
}

/// `CMSG_GUILD_ACCEPT` / `CMSG_GUILD_DECLINE`.
///
/// Answers the caller nothing either way, which is not an oversight: what the accepting client
/// renders is the `Joined` broadcast and its own re-query, and what a decline produces is a
/// notification to the INVITER. A refusal here means the dialog is already gone — answered twice,
/// or reaped by the invite GC — so there is no command result to send about it.
fn guild_answer_invite<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    accept: bool,
) -> Result<GuildActionOutcome> {
    if let Err(e) = store.guild_answer_invite(account_id, self_guid, accept) {
        if classify_guild_action_error(&e) == GuildActionErrorClass::Fatal {
            return Err(e);
        }
        log::debug!("world: guild invite answer refused (account {account_id}): {e:#}");
    }
    Ok(GuildActionOutcome::Handled {
        outbound: Vec::new(),
    })
}

/// `CMSG_GUILD_ROSTER` — the guild panel, and the one guild screen realm-core cannot answer on its
/// own: it holds guids, ranks and notes, so the name, level, class, area and online flag of every
/// member arrive from the shards through the routing layer.
///
/// Offline members are listed, not omitted — reading who is in the guild while they are away is
/// most of what the panel is for.
///
/// A caller in no guild is refused with `GuildPlayerNotInGuild` under the `Create` command, the
/// pair vanilla's own roster handler sends. A membership pointing at a guild that has since gone
/// gets the same answer: from the client's side those are the same fact.
fn guild_roster<St: GuildActionStore + ?Sized>(
    store: &St,
    self_guid: u64,
) -> Result<GuildActionOutcome> {
    let roster = match store.guild_of(self_guid)? {
        Some(guild_id) => store.guild_roster(guild_id)?,
        None => None,
    };
    let Some(roster) = roster else {
        return Ok(GuildActionOutcome::Handled {
            outbound: command_result(
                GuildCommand::Create,
                "",
                GuildCommandResult::GuildPlayerNotInGuild,
            ),
        });
    };
    Ok(GuildActionOutcome::Handled {
        outbound: vec![Outbound::One(ServerOpcodeMessage::SMSG_GUILD_ROSTER(
            Box::new(codec::build_guild_roster(&roster)),
        ))],
    })
}

/// The shape every guild WRITE verb shares: one durable request, then the wire's verdict either
/// way. A client that hears nothing leaves its dialog open, so a refusal is always answered.
///
/// `classify` is the family's own error map — the one thing that genuinely differs between a create,
/// an invite, a teardown and a setter — and `success` is what a client hears when the request lands.
fn write_verb(
    command: GuildCommand,
    subject: &str,
    verb: &str,
    classify: fn(&anyhow::Error) -> GuildCommandResult,
    success: Vec<Outbound>,
    request: impl FnOnce() -> Result<()>,
) -> Result<GuildActionOutcome> {
    if let Err(e) = request() {
        if classify_guild_action_error(&e) == GuildActionErrorClass::Fatal {
            return Err(e);
        }
        log::debug!("world: guild {verb} refused: {e:#}");
        return Ok(GuildActionOutcome::Handled {
            outbound: command_result(command, subject, classify(&e)),
        });
    }
    Ok(GuildActionOutcome::Handled { outbound: success })
}

/// A teardown verb: leave, kick, disband or the leadership transfer. Success is wire code 0 —
/// vanilla's "no message/error".
fn teardown(
    command: GuildCommand,
    subject: &str,
    verb: &str,
    request: impl FnOnce() -> Result<()>,
) -> Result<GuildActionOutcome> {
    let success = command_result(command, subject, GuildCommandResult::PlayerNoMoreInGuild);
    write_verb(
        command,
        subject,
        verb,
        teardown_result_for,
        success,
        request,
    )
}

/// A setter verb: MOTD, info text or either note. `GuildCommand` has no variant for any of them
/// (vanilla's wire defines only Create/Invite/Quit/Founder/Unknown19/Unknown20), and `Founder` is
/// the closest of the six to "you are not the guild master", so every refusal here answers with it.
fn setter(
    subject: &str,
    verb: &str,
    success: Vec<Outbound>,
    request: impl FnOnce() -> Result<()>,
) -> Result<GuildActionOutcome> {
    write_verb(
        GuildCommand::Founder,
        subject,
        verb,
        setter_result_for,
        success,
        request,
    )
}

/// `CMSG_GUILD_LEAVE`. `GuildCommand::Quit` is the client's own vocabulary for a member leaving,
/// and it is also what the master's `GuildPermissionsOrLeader` refusal has to arrive under for the
/// client to render "the guild leader cannot leave" rather than "you lack permission".
fn guild_leave<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
) -> Result<GuildActionOutcome> {
    teardown(GuildCommand::Quit, "", "leave", || {
        store.guild_leave(account_id, self_guid)
    })
}

/// `CMSG_GUILD_REMOVE` — the kick. The subject is the typed name, so a refusal names whoever the
/// master meant even when that name resolves to nobody.
fn guild_remove<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    target_name: &str,
) -> Result<GuildActionOutcome> {
    teardown(GuildCommand::Quit, target_name, "remove", || {
        store.guild_remove(account_id, self_guid, target_name)
    })
}

/// `CMSG_GUILD_DISBAND`. `GuildCommand::Founder` because destroying a guild is a founder's act, not
/// a member leaving one.
fn guild_disband<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
) -> Result<GuildActionOutcome> {
    teardown(GuildCommand::Founder, "", "disband", || {
        store.guild_disband(account_id, self_guid)
    })
}

/// `CMSG_GUILD_LEADER` — the explicit succession this system has instead of an automatic one.
fn guild_leader<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    target_name: &str,
) -> Result<GuildActionOutcome> {
    teardown(GuildCommand::Founder, target_name, "leader", || {
        store.guild_set_master(account_id, self_guid, target_name)
    })
}

/// `CMSG_MESSAGECHAT` (`ChatType::Guild`, `/g`) — not part of `dispatch_guild_action`'s match (that
/// seam only owns the `CMSG_GUILD_*` family; chat rides `CMSG_MESSAGECHAT`, whose OTHER chat types
/// stay `handlers/query.rs`'s to dispatch), but the SAME seam shape so the branch is testable
/// without a socket: called directly from `handlers/query.rs`'s `ChatType::Guild` arm.
///
/// A caller with no guild answers `SMSG_GUILD_COMMAND_RESULT(GuildPlayerNotInGuild)` and delivers
/// nothing. Any OTHER gameplay refusal (not in world / empty message) is silently dropped, matching
/// say/yell/party — the client never sends an empty line anyway. A lost reducer transport still
/// propagates, exactly as every other guild op does.
pub(crate) fn guild_chat_action<St: GuildActionStore + ?Sized>(
    store: &St,
    self_guid: u64,
    text: String,
) -> Result<Vec<Outbound>> {
    if let Err(e) = store.guild_chat(self_guid, &text) {
        if classify_guild_action_error(&e) == GuildActionErrorClass::Fatal {
            return Err(e);
        }
        if format!("{e:#}").contains(lyracore_shared::guild::err::NOT_IN_GUILD) {
            log::debug!("world: guild chat refused (guid {self_guid}): {e:#}");
            return Ok(command_result(
                GuildCommand::Quit,
                "",
                GuildCommandResult::GuildPlayerNotInGuild,
            ));
        }
        log::debug!("world: guild chat dropped (guid {self_guid}): {e:#}");
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::guild::{err, DEFAULT_RANK_NAMES};
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        CMSG_GUILD_CREATE, CMSG_GUILD_INVITE, CMSG_GUILD_QUERY, CMSG_PING,
    };

    /// Records the durable requests the seam makes and answers its reads from canned rows, so each
    /// dispatch branch is exercised without a database.
    #[derive(Default)]
    struct InMemoryGuildActions {
        create_requests: Mutex<Vec<(u64, u64, String)>>,
        create_error: Option<String>,
        guilds: Vec<GuildView>,
        membership: Vec<(u64, u64)>,
        read_error: Option<String>,
        /// `(account_id, self_guid, invited_name)` per `CMSG_GUILD_INVITE` that reached the
        /// durable layer — the "writes exactly one invite" assertion.
        invite_requests: Mutex<Vec<(u64, u64, String)>>,
        invite_error: Option<String>,
        /// `(account_id, self_guid, accept)` per answered invite.
        answer_requests: Mutex<Vec<(u64, u64, bool)>>,
        answer_error: Option<String>,
        /// Rosters keyed by guild id, ALREADY rendered — the shard fan-out is the routing layer's,
        /// and is pinned there (`world/guild_tests.rs`).
        rosters: Vec<(u64, GuildRoster)>,
        /// The teardown verbs' requests: `(account_id, self_guid, subject)`. One log, because a
        /// dispatch test cares which verb ran and with what, and the verb is named by the test.
        teardown_requests: Mutex<Vec<(&'static str, u64, u64, String)>>,
        /// When set, every teardown verb refuses with this message — the durable layer's answer,
        /// which the seam's only job is to classify.
        teardown_error: Option<String>,
        chat_requests: Mutex<Vec<(u64, String)>>,
        chat_error: Option<String>,
        motd_requests: Mutex<Vec<(u64, u64, String)>>,
        motd_error: Option<String>,
        info_text_requests: Mutex<Vec<(u64, u64, String)>>,
        info_text_error: Option<String>,
        public_note_requests: Mutex<Vec<(u64, u64, String, String)>>,
        public_note_error: Option<String>,
        officer_note_requests: Mutex<Vec<(u64, u64, String, String)>>,
        officer_note_error: Option<String>,
    }

    impl GuildActionStore for InMemoryGuildActions {
        fn guild_create(&self, account_id: u64, self_guid: u64, name: &str) -> Result<()> {
            self.create_requests
                .lock()
                .unwrap()
                .push((account_id, self_guid, name.to_string()));
            self.create_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn guild_view(&self, guild_id: u64) -> Result<Option<GuildView>> {
            if let Some(error) = &self.read_error {
                anyhow::bail!("{error}");
            }
            Ok(self.guilds.iter().find(|g| g.guild_id == guild_id).cloned())
        }

        fn guild_of(&self, character_guid: u64) -> Result<Option<u64>> {
            Ok(self
                .membership
                .iter()
                .find(|(guid, _)| *guid == character_guid)
                .map(|(_, guild_id)| *guild_id))
        }

        fn guild_invite(&self, account_id: u64, self_guid: u64, name: &str) -> Result<()> {
            self.invite_requests
                .lock()
                .unwrap()
                .push((account_id, self_guid, name.to_string()));
            self.invite_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn guild_set_motd(&self, account_id: u64, self_guid: u64, motd: &str) -> Result<()> {
            self.motd_requests
                .lock()
                .unwrap()
                .push((account_id, self_guid, motd.to_string()));
            self.motd_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn guild_answer_invite(&self, account_id: u64, self_guid: u64, accept: bool) -> Result<()> {
            self.answer_requests
                .lock()
                .unwrap()
                .push((account_id, self_guid, accept));
            self.answer_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn guild_set_info_text(&self, account_id: u64, self_guid: u64, text: &str) -> Result<()> {
            self.info_text_requests
                .lock()
                .unwrap()
                .push((account_id, self_guid, text.to_string()));
            self.info_text_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn guild_roster(&self, guild_id: u64) -> Result<Option<GuildRoster>> {
            if let Some(error) = &self.read_error {
                anyhow::bail!("{error}");
            }
            Ok(self
                .rosters
                .iter()
                .find(|(id, _)| *id == guild_id)
                .map(|(_, roster)| roster.clone()))
        }

        fn guild_leave(&self, account_id: u64, self_guid: u64) -> Result<()> {
            self.teardown("leave", account_id, self_guid, String::new())
        }

        fn guild_remove(&self, account_id: u64, self_guid: u64, target_name: &str) -> Result<()> {
            self.teardown("remove", account_id, self_guid, target_name.to_string())
        }

        fn guild_disband(&self, account_id: u64, self_guid: u64) -> Result<()> {
            self.teardown("disband", account_id, self_guid, String::new())
        }

        fn guild_set_master(
            &self,
            account_id: u64,
            self_guid: u64,
            target_name: &str,
        ) -> Result<()> {
            self.teardown("leader", account_id, self_guid, target_name.to_string())
        }

        fn guild_set_public_note(
            &self,
            account_id: u64,
            self_guid: u64,
            target_name: &str,
            note: &str,
        ) -> Result<()> {
            self.public_note_requests.lock().unwrap().push((
                account_id,
                self_guid,
                target_name.to_string(),
                note.to_string(),
            ));
            self.public_note_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn guild_set_officer_note(
            &self,
            account_id: u64,
            self_guid: u64,
            target_name: &str,
            note: &str,
        ) -> Result<()> {
            self.officer_note_requests.lock().unwrap().push((
                account_id,
                self_guid,
                target_name.to_string(),
                note.to_string(),
            ));
            self.officer_note_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn guild_chat(&self, self_guid: u64, text: &str) -> Result<()> {
            self.chat_requests
                .lock()
                .unwrap()
                .push((self_guid, text.to_string()));
            self.chat_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }
    }

    impl InMemoryGuildActions {
        fn teardown(
            &self,
            verb: &'static str,
            account_id: u64,
            self_guid: u64,
            subject: String,
        ) -> Result<()> {
            self.teardown_requests
                .lock()
                .unwrap()
                .push((verb, account_id, self_guid, subject));
            self.teardown_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }
    }

    const GINGER: u64 = 42;

    fn player() -> GuildActionPlayer {
        GuildActionPlayer {
            account_id: 7,
            self_guid: Some(GINGER),
        }
    }

    fn silver_hand() -> GuildView {
        GuildView {
            guild_id: 3,
            name: "The Silver Hand".into(),
            master_guid: GINGER,
            motd: String::new(),
            info_text: String::new(),
            // 2026-08-15T00:00:00Z
            created_micros: 20_680i64 * 86_400 * 1_000_000,
            member_count: 4,
            rank_names: DEFAULT_RANK_NAMES.iter().map(|n| n.to_string()).collect(),
        }
    }

    fn create(name: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_GUILD_CREATE(Box::new(CMSG_GUILD_CREATE {
            guild_name: name.into(),
        }))
    }

    fn refused(error: &str) -> InMemoryGuildActions {
        InMemoryGuildActions {
            create_error: Some(error.into()),
            ..Default::default()
        }
    }

    fn handled(outcome: GuildActionOutcome) -> Vec<Outbound> {
        match outcome {
            GuildActionOutcome::Handled { outbound } => outbound,
            GuildActionOutcome::PassThrough(_) => panic!("the guild seam must consume this opcode"),
        }
    }

    fn command_result_of(outbound: &[Outbound]) -> (GuildCommand, String, GuildCommandResult) {
        match outbound {
            [Outbound::One(ServerOpcodeMessage::SMSG_GUILD_COMMAND_RESULT(m))] => {
                (m.command, m.string.clone(), m.result)
            }
            other => panic!(
                "expected one SMSG_GUILD_COMMAND_RESULT, got {} message(s)",
                other.len()
            ),
        }
    }

    /// AC1/AC2: a valid create reaches the durable layer with the caller's account and guid, and
    /// the client is told the command succeeded.
    #[test]
    fn a_valid_create_requests_the_guild_and_answers_the_success_code() {
        let actions = InMemoryGuildActions::default();

        let outbound =
            handled(dispatch_guild_action(&actions, player(), create("The Silver Hand")).unwrap());

        assert_eq!(
            actions.create_requests.lock().unwrap().as_slice(),
            &[(7, GINGER, "The Silver Hand".to_string())]
        );
        assert_eq!(
            command_result_of(&outbound),
            (
                GuildCommand::Create,
                "The Silver Hand".to_string(),
                GuildCommandResult::PlayerNoMoreInGuild
            ),
            "wire code 0 is vanilla's 'no message/error', i.e. success"
        );
    }

    /// AC3: a duplicate name is `GuildNameExistsS`, and the refusal is the ONLY thing sent.
    #[test]
    fn a_taken_name_answers_guild_name_exists() {
        let actions = refused(err::NAME_TAKEN);

        let outbound =
            handled(dispatch_guild_action(&actions, player(), create("The Silver Hand")).unwrap());

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildNameExistsS
        );
    }

    /// AC4: an invalid name is `GuildNameInvalid`.
    #[test]
    fn an_invalid_name_answers_guild_name_invalid() {
        let actions = refused(err::NAME_INVALID);

        let outbound = handled(dispatch_guild_action(&actions, player(), create("X")).unwrap());

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildNameInvalid
        );
    }

    /// AC5: a founder who is already in a guild is `AlreadyInGuild`.
    #[test]
    fn a_founder_already_in_a_guild_answers_already_in_guild() {
        let actions = refused(err::ALREADY_IN_GUILD);

        let outbound =
            handled(dispatch_guild_action(&actions, player(), create("Second")).unwrap());

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::AlreadyInGuild
        );
    }

    /// AC10: an unclassified refusal still answers — silently dropping it leaves the client's
    /// create dialog open — and never ends the session.
    #[test]
    fn an_unclassified_refusal_answers_the_internal_code_and_keeps_the_session_alive() {
        let actions = refused("something the module has not told the gateway about");

        let outbound = handled(dispatch_guild_action(&actions, player(), create("Any")).unwrap());

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildInternal
        );
    }

    /// AC10: a lost reducer transport is the one failure that propagates.
    #[test]
    fn a_lost_reducer_transport_propagates_as_an_error() {
        let actions = refused("reducer transport disconnected");

        let error = match dispatch_guild_action(&actions, player(), create("Any")) {
            Err(error) => error,
            Ok(_) => panic!("a lost transport must not be swallowed as a refusal"),
        };

        assert!(format!("{error:#}").contains("reducer transport disconnected"));
    }

    /// AC6: the query answers with the guild's id, name and all ten rank names in rank order.
    #[test]
    fn a_query_for_a_live_guild_answers_with_its_name_and_ten_ranks() {
        let actions = InMemoryGuildActions {
            guilds: vec![silver_hand()],
            ..Default::default()
        };

        let outbound = handled(
            dispatch_guild_action(
                &actions,
                player(),
                ClientOpcodeMessage::CMSG_GUILD_QUERY(CMSG_GUILD_QUERY { guild_id: 3 }),
            )
            .unwrap(),
        );

        let [Outbound::One(ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(m))] =
            outbound.as_slice()
        else {
            panic!("expected one SMSG_GUILD_QUERY_RESPONSE");
        };
        assert_eq!(m.id, 3);
        assert_eq!(m.name, "The Silver Hand");
        assert_eq!(m.rank_names[0], "Guild Master");
        assert_eq!(m.rank_names[4], "Initiate");
        assert_eq!(m.rank_names[9], "");
    }

    /// AC7: an unknown guild id is answered with an empty batch, not an error and not a packet.
    #[test]
    fn a_query_for_an_unknown_guild_is_handled_with_an_empty_batch() {
        let actions = InMemoryGuildActions::default();

        let outbound = handled(
            dispatch_guild_action(
                &actions,
                player(),
                ClientOpcodeMessage::CMSG_GUILD_QUERY(CMSG_GUILD_QUERY { guild_id: 999 }),
            )
            .unwrap(),
        );

        assert!(outbound.is_empty());
    }

    /// AC8: the info panel carries the name, the founding date and the member count.
    #[test]
    fn the_info_panel_answers_with_the_callers_own_guild() {
        let actions = InMemoryGuildActions {
            guilds: vec![silver_hand()],
            membership: vec![(GINGER, 3)],
            ..Default::default()
        };

        let outbound = handled(
            dispatch_guild_action(&actions, player(), ClientOpcodeMessage::CMSG_GUILD_INFO)
                .unwrap(),
        );

        let [Outbound::One(ServerOpcodeMessage::SMSG_GUILD_INFO(m))] = outbound.as_slice() else {
            panic!("expected one SMSG_GUILD_INFO");
        };
        assert_eq!(m.guild_name, "The Silver Hand");
        assert_eq!(
            (m.created_day, m.created_month, m.created_year),
            (14, 7, 2026)
        );
        assert_eq!(m.amount_of_characters_in_guild, 4);
    }

    #[test]
    fn the_info_panel_answers_a_guildless_caller_with_an_empty_batch() {
        let actions = InMemoryGuildActions::default();

        let outbound = handled(
            dispatch_guild_action(&actions, player(), ClientOpcodeMessage::CMSG_GUILD_INFO)
                .unwrap(),
        );

        assert!(outbound.is_empty());
    }

    /// A read that cannot reach its database is not a refusal to classify — it propagates.
    #[test]
    fn a_failed_read_propagates_rather_than_answering_an_empty_batch() {
        let actions = InMemoryGuildActions {
            read_error: Some("realm-core unreachable".into()),
            ..Default::default()
        };

        let outcome = dispatch_guild_action(
            &actions,
            player(),
            ClientOpcodeMessage::CMSG_GUILD_QUERY(CMSG_GUILD_QUERY { guild_id: 3 }),
        );

        assert!(
            outcome.is_err(),
            "a broken read must not look like an unknown guild"
        );
    }

    #[test]
    fn a_player_outside_the_world_passes_every_guild_opcode_through() {
        let actions = InMemoryGuildActions::default();
        let player = GuildActionPlayer {
            self_guid: None,
            ..player()
        };

        let outcome = dispatch_guild_action(&actions, player, create("Any")).unwrap();

        assert!(matches!(
            outcome,
            GuildActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_GUILD_CREATE(_))
        ));
        assert!(actions.create_requests.lock().unwrap().is_empty());
    }

    // --- the invite handshake ---------------------------------------------------------------

    fn invite(name: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_GUILD_INVITE(Box::new(CMSG_GUILD_INVITE {
            invited_player: name.into(),
        }))
    }

    fn invite_refused(error: &str) -> InMemoryGuildActions {
        InMemoryGuildActions {
            invite_error: Some(error.into()),
            ..Default::default()
        }
    }

    /// AC1: an accepted invite reaches the durable layer exactly once, carrying the caller's
    /// account and guid and the name they typed, and the client is told the command succeeded.
    #[test]
    fn a_valid_invite_requests_it_once_and_answers_the_success_code() {
        let actions = InMemoryGuildActions::default();

        let outbound = handled(dispatch_guild_action(&actions, player(), invite("Vim")).unwrap());

        assert_eq!(
            actions.invite_requests.lock().unwrap().as_slice(),
            &[(7, GINGER, "Vim".to_string())],
            "exactly one invite, for the character the client named"
        );
        assert_eq!(
            command_result_of(&outbound),
            (
                GuildCommand::Invite,
                "Vim".to_string(),
                GuildCommandResult::PlayerNoMoreInGuild
            ),
            "wire code 0 is vanilla's 'no message/error', i.e. success"
        );
    }

    /// AC3: each of the five invite gates answers its own `GuildCommandResult`, and the refusal is
    /// the only thing the client hears.
    #[test]
    fn every_invite_gate_answers_its_own_result_code() {
        for (refusal, expected) in [
            (err::NOT_IN_GUILD, GuildCommandResult::GuildPlayerNotInGuild),
            (
                err::NOT_GUILD_MASTER,
                GuildCommandResult::GuildPermissionsOrLeader,
            ),
            (
                err::TARGET_NOT_FOUND,
                GuildCommandResult::GuildPlayerNotFoundS,
            ),
            (err::TARGET_IN_GUILD, GuildCommandResult::AlreadyInGuildS),
            (
                err::ALREADY_INVITED,
                GuildCommandResult::AlreadyInvitedToGuildS,
            ),
        ] {
            let actions = invite_refused(refusal);

            let outbound =
                handled(dispatch_guild_action(&actions, player(), invite("Vim")).unwrap());

            assert_eq!(
                command_result_of(&outbound),
                (GuildCommand::Invite, "Vim".to_string(), expected),
                "`{refusal}` must answer {expected:?}"
            );
        }
    }

    /// AC3, the other half: a refusal writes nothing. The gates run module-side inside the
    /// transaction, so the seam's job is to carry the refusal back without a second attempt.
    #[test]
    fn a_refused_invite_is_requested_once_and_never_retried() {
        let actions = invite_refused(err::TARGET_IN_GUILD);

        handled(dispatch_guild_action(&actions, player(), invite("Vim")).unwrap());

        assert_eq!(actions.invite_requests.lock().unwrap().len(), 1);
        assert!(actions.answer_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unclassified_invite_refusal_answers_the_internal_code() {
        let actions = invite_refused("something the module has not told the gateway about");

        let outbound = handled(dispatch_guild_action(&actions, player(), invite("Vim")).unwrap());

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildInternal
        );
    }

    #[test]
    fn a_lost_transport_during_an_invite_propagates_as_an_error() {
        let actions = invite_refused("reducer transport disconnected");

        assert!(
            dispatch_guild_action(&actions, player(), invite("Vim")).is_err(),
            "a lost transport must not be swallowed as a refusal"
        );
    }

    /// AC4/AC5/AC6: accept and decline are the same durable request with opposite answers, and the
    /// caller hears nothing back — the accepting client renders the `Joined` broadcast, and a
    /// decline is a notification to the inviter.
    #[test]
    fn accept_and_decline_reach_the_authority_with_opposite_answers_and_reply_nothing() {
        for (msg, accept) in [
            (ClientOpcodeMessage::CMSG_GUILD_ACCEPT, true),
            (ClientOpcodeMessage::CMSG_GUILD_DECLINE, false),
        ] {
            let actions = InMemoryGuildActions::default();

            let outbound = handled(dispatch_guild_action(&actions, player(), msg).unwrap());

            assert_eq!(
                actions.answer_requests.lock().unwrap().as_slice(),
                &[(7, GINGER, accept)]
            );
            assert!(outbound.is_empty());
        }
    }

    /// AC7/AC8: answering an invite that is gone — never sent, already answered, or reaped by the
    /// two-minute GC — is a no-op. It writes nothing, says nothing, and never ends the session.
    #[test]
    fn answering_a_missing_invite_is_a_silent_no_op() {
        let actions = InMemoryGuildActions {
            answer_error: Some(err::NO_PENDING_INVITE.into()),
            ..Default::default()
        };

        let outbound = handled(
            dispatch_guild_action(&actions, player(), ClientOpcodeMessage::CMSG_GUILD_ACCEPT)
                .unwrap(),
        );

        assert!(outbound.is_empty(), "there is no dialog left to answer");
        assert!(actions.invite_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn a_lost_transport_while_answering_an_invite_propagates_as_an_error() {
        let actions = InMemoryGuildActions {
            answer_error: Some("reducer transport disconnected".into()),
            ..Default::default()
        };

        assert!(
            dispatch_guild_action(&actions, player(), ClientOpcodeMessage::CMSG_GUILD_ACCEPT)
                .is_err()
        );
    }

    // --- Teardown: leave, kick, disband and the leadership transfer -------------------------

    use wow_world_messages::vanilla::{CMSG_GUILD_LEADER, CMSG_GUILD_REMOVE};

    /// Every teardown verb refuses with `error`; the roster fixture is the one the reads answer
    /// from, so a refusal is never mistaken for "there was no guild to act on".
    fn teardown_refused(error: &str) -> InMemoryGuildActions {
        InMemoryGuildActions {
            guilds: vec![silver_hand()],
            membership: vec![(GINGER, 3)],
            teardown_error: Some(error.into()),
            ..Default::default()
        }
    }

    fn seeded() -> InMemoryGuildActions {
        InMemoryGuildActions {
            guilds: vec![silver_hand()],
            membership: vec![(GINGER, 3)],
            ..Default::default()
        }
    }

    fn requests(actions: &InMemoryGuildActions) -> Vec<(&'static str, u64, u64, String)> {
        actions.teardown_requests.lock().unwrap().clone()
    }

    fn remove(name: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_GUILD_REMOVE(Box::new(CMSG_GUILD_REMOVE {
            player_name: name.into(),
        }))
    }

    fn leader(name: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_GUILD_LEADER(Box::new(CMSG_GUILD_LEADER {
            new_guild_leader_name: name.into(),
        }))
    }

    /// AC1: a leave reaches the durable layer naming the caller, and the client is told it worked.
    #[test]
    fn a_leave_requests_the_removal_and_answers_the_success_code() {
        let actions = seeded();

        let outbound = handled(
            dispatch_guild_action(&actions, player(), ClientOpcodeMessage::CMSG_GUILD_LEAVE)
                .unwrap(),
        );

        assert_eq!(
            requests(&actions),
            vec![("leave", 7, GINGER, String::new())]
        );
        assert_eq!(
            command_result_of(&outbound),
            (
                GuildCommand::Quit,
                String::new(),
                GuildCommandResult::PlayerNoMoreInGuild
            ),
            "wire code 0 is vanilla's 'no message/error', i.e. success"
        );
    }

    /// AC2: a master who would leave members behind is refused, and the refusal is the only thing
    /// sent — succession is a decision the player makes, not one the realm makes for them.
    #[test]
    fn a_master_leaving_with_members_remaining_answers_permissions_or_leader() {
        let actions = teardown_refused(err::MASTER_MUST_TRANSFER_OR_DISBAND);

        let outbound = handled(
            dispatch_guild_action(&actions, player(), ClientOpcodeMessage::CMSG_GUILD_LEAVE)
                .unwrap(),
        );

        assert_eq!(
            command_result_of(&outbound),
            (
                GuildCommand::Quit,
                String::new(),
                GuildCommandResult::GuildPermissionsOrLeader
            )
        );
    }

    /// AC4: a kick carries the typed name to the durable layer and answers under it, so the client
    /// can render "you have removed <name>".
    #[test]
    fn a_kick_requests_the_named_member_and_answers_under_that_name() {
        let actions = seeded();

        let outbound = handled(dispatch_guild_action(&actions, player(), remove("Vim")).unwrap());

        assert_eq!(
            requests(&actions),
            vec![("remove", 7, GINGER, "Vim".to_string())]
        );
        assert_eq!(
            command_result_of(&outbound),
            (
                GuildCommand::Quit,
                "Vim".to_string(),
                GuildCommandResult::PlayerNoMoreInGuild
            )
        );
    }

    /// AC5: a kick by somebody who is not the master is `GuildPermissionsOrLeader`.
    #[test]
    fn a_kick_by_a_non_master_answers_permissions_or_leader() {
        let actions = teardown_refused(err::NOT_GUILD_MASTER);

        let outbound = handled(dispatch_guild_action(&actions, player(), remove("Vim")).unwrap());

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildPermissionsOrLeader
        );
    }

    /// AC6: a kick naming somebody who is not in the guild — including a name that names nobody at
    /// all — is `GuildPlayerNotInGuildS`, the refusal that carries the name back.
    #[test]
    fn a_kick_naming_a_non_member_answers_player_not_in_guild_s() {
        let actions = teardown_refused(err::TARGET_NOT_IN_GUILD);

        let outbound =
            handled(dispatch_guild_action(&actions, player(), remove("Nobody")).unwrap());

        assert_eq!(
            command_result_of(&outbound),
            (
                GuildCommand::Quit,
                "Nobody".to_string(),
                GuildCommandResult::GuildPlayerNotInGuildS
            )
        );
    }

    /// AC7: a master kicking themselves is refused rather than walking into the succession path.
    #[test]
    fn a_master_kicking_themselves_answers_permissions_or_leader() {
        let actions = teardown_refused(err::CANNOT_REMOVE_SELF);

        let outbound =
            handled(dispatch_guild_action(&actions, player(), remove("Ginger")).unwrap());

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildPermissionsOrLeader
        );
    }

    /// AC8: a disband reaches the durable layer naming the caller, under the founder command.
    #[test]
    fn a_disband_requests_the_cascade_and_answers_the_success_code() {
        let actions = seeded();

        let outbound = handled(
            dispatch_guild_action(&actions, player(), ClientOpcodeMessage::CMSG_GUILD_DISBAND)
                .unwrap(),
        );

        assert_eq!(
            requests(&actions),
            vec![("disband", 7, GINGER, String::new())]
        );
        assert_eq!(
            command_result_of(&outbound),
            (
                GuildCommand::Founder,
                String::new(),
                GuildCommandResult::PlayerNoMoreInGuild
            )
        );
    }

    /// AC9: a disband by a non-master is refused and nothing else is sent.
    #[test]
    fn a_disband_by_a_non_master_answers_permissions_or_leader() {
        let actions = teardown_refused(err::NOT_GUILD_MASTER);

        let outbound = handled(
            dispatch_guild_action(&actions, player(), ClientOpcodeMessage::CMSG_GUILD_DISBAND)
                .unwrap(),
        );

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildPermissionsOrLeader
        );
    }

    /// AC10: a leadership transfer carries the new master's name to the durable layer.
    #[test]
    fn a_leadership_transfer_requests_the_named_member_and_answers_under_that_name() {
        let actions = seeded();

        let outbound = handled(dispatch_guild_action(&actions, player(), leader("Vim")).unwrap());

        assert_eq!(
            requests(&actions),
            vec![("leader", 7, GINGER, "Vim".to_string())]
        );
        assert_eq!(
            command_result_of(&outbound),
            (
                GuildCommand::Founder,
                "Vim".to_string(),
                GuildCommandResult::PlayerNoMoreInGuild
            )
        );
    }

    /// AC11: handing the guild to somebody who is not in it is `GuildPlayerNotInGuildS`.
    #[test]
    fn a_leadership_transfer_to_a_non_member_answers_player_not_in_guild_s() {
        let actions = teardown_refused(err::TARGET_NOT_IN_GUILD);

        let outbound =
            handled(dispatch_guild_action(&actions, player(), leader("Stranger")).unwrap());

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildPlayerNotInGuildS
        );
    }

    /// A guildless caller is `GuildPlayerNotInGuild` — the refusal WITHOUT a name in it, which is
    /// the one the client renders as "you are not in a guild".
    #[test]
    fn a_teardown_verb_from_a_guildless_caller_answers_player_not_in_guild() {
        let actions = teardown_refused(err::NOT_IN_GUILD);

        let outbound = handled(
            dispatch_guild_action(&actions, player(), ClientOpcodeMessage::CMSG_GUILD_LEAVE)
                .unwrap(),
        );

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildPlayerNotInGuild
        );
    }

    /// An unclassified teardown refusal still answers, and a lost transport still propagates — the
    /// same two rules the create branch keeps, on the branch a player uses far more often.
    #[test]
    fn an_unclassified_teardown_refusal_answers_and_a_lost_transport_propagates() {
        let unknown = teardown_refused("something the module has not told the gateway about");
        let outbound = handled(
            dispatch_guild_action(&unknown, player(), ClientOpcodeMessage::CMSG_GUILD_LEAVE)
                .unwrap(),
        );
        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildInternal
        );

        let lost = teardown_refused("reducer transport disconnected");
        assert!(
            dispatch_guild_action(&lost, player(), ClientOpcodeMessage::CMSG_GUILD_DISBAND)
                .is_err(),
            "a lost transport must not be swallowed as a refusal"
        );
    }

    /// Before world entry there is no acting character, so a teardown opcode passes through rather
    /// than being evaluated against a placeholder guid.
    #[test]
    fn a_player_outside_the_world_passes_the_teardown_opcodes_through() {
        let actions = seeded();
        let player = GuildActionPlayer {
            self_guid: None,
            ..player()
        };

        let outcome =
            dispatch_guild_action(&actions, player, ClientOpcodeMessage::CMSG_GUILD_DISBAND)
                .unwrap();

        assert!(matches!(
            outcome,
            GuildActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_GUILD_DISBAND)
        ));
        assert!(requests(&actions).is_empty());
    }

    #[test]
    fn unrelated_opcodes_pass_through_to_the_next_dispatcher() {
        let actions = InMemoryGuildActions::default();

        let outcome = dispatch_guild_action(
            &actions,
            player(),
            ClientOpcodeMessage::CMSG_PING(CMSG_PING::default()),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            GuildActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_PING(_))
        ));
    }

    // --- Roster ---------------------------------------------------------------------------

    use crate::world::guild::GuildRosterEntry;
    use wow_world_messages::vanilla::{
        Area, Class, GuildMember_GuildMemberStatus, Level, SMSG_GUILD_ROSTER,
    };

    const VIM: u64 = 43;

    /// The Silver Hand's roster as the routing layer hands it over: the master online, one member
    /// offline, and every shard-resolved field DIFFERENT between the two — a level, a class and a
    /// zone that a hard-coded default could not produce by accident.
    fn silver_hand_roster() -> GuildRoster {
        use lyracore_shared::guild::DEFAULT_RANK_RIGHTS;
        GuildRoster {
            motd: "Raid at eight".into(),
            info_text: "Founded on the Sunday".into(),
            rank_rights: DEFAULT_RANK_RIGHTS.to_vec(),
            members: vec![
                GuildRosterEntry {
                    guid: GINGER,
                    rank: 0,
                    public_note: "founder".into(),
                    officer_note: "trusted".into(),
                    name: "Ginger".into(),
                    level: 60,
                    class: 2,    // Paladin
                    zone_id: 12, // Elwynn Forest
                    online: true,
                },
                GuildRosterEntry {
                    guid: VIM,
                    rank: 3,
                    public_note: "alt of Ginger".into(),
                    officer_note: String::new(),
                    name: "Vim".into(),
                    level: 24,
                    class: 8,    // Mage
                    zone_id: 14, // Durotar
                    online: false,
                },
            ],
        }
    }

    /// A store holding the Silver Hand's roster, with Ginger a member of it.
    fn with_roster() -> InMemoryGuildActions {
        InMemoryGuildActions {
            guilds: vec![silver_hand()],
            membership: vec![(GINGER, 3)],
            rosters: vec![(3, silver_hand_roster())],
            ..Default::default()
        }
    }

    fn roster_of(actions: &InMemoryGuildActions) -> SMSG_GUILD_ROSTER {
        let outbound = handled(
            dispatch_guild_action(actions, player(), ClientOpcodeMessage::CMSG_GUILD_ROSTER)
                .unwrap(),
        );
        match outbound.as_slice() {
            [Outbound::One(ServerOpcodeMessage::SMSG_GUILD_ROSTER(m))] => (**m).clone(),
            other => panic!(
                "expected exactly one SMSG_GUILD_ROSTER, got {} message(s)",
                other.len()
            ),
        }
    }

    fn motd(text: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_GUILD_MOTD(Box::new(
            wow_world_messages::vanilla::CMSG_GUILD_MOTD {
                message_of_the_day: text.into(),
            },
        ))
    }

    fn info_text(text: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_GUILD_INFO_TEXT(Box::new(
            wow_world_messages::vanilla::CMSG_GUILD_INFO_TEXT {
                guild_info: text.into(),
            },
        ))
    }

    fn set_public_note(target: &str, note: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_GUILD_SET_PUBLIC_NOTE(Box::new(
            wow_world_messages::vanilla::CMSG_GUILD_SET_PUBLIC_NOTE {
                player_name: target.into(),
                note: note.into(),
            },
        ))
    }

    fn set_officer_note(target: &str, note: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_GUILD_SET_OFFICER_NOTE(Box::new(
            wow_world_messages::vanilla::CMSG_GUILD_SET_OFFICER_NOTE {
                player_name: target.into(),
                note: note.into(),
            },
        ))
    }

    /// AC1/AC6: one roster packet listing every member, online and offline, each carrying the two
    /// notes realm-core holds for it.
    #[test]
    fn a_roster_request_lists_every_member_with_their_ranks_and_notes() {
        let m = roster_of(&with_roster());

        assert_eq!(m.members.len(), 2, "the offline member is listed too");
        assert_eq!(m.members[0].guid.guid(), GINGER);
        assert_eq!(m.members[0].rank, 0);
        assert_eq!(m.members[0].public_note, "founder");
        assert_eq!(m.members[0].officer_note, "trusted");
        assert_eq!(m.members[1].guid.guid(), VIM);
        assert_eq!(m.members[1].rank, 3);
        assert_eq!(m.members[1].public_note, "alt of Ginger");
        assert_eq!(m.members[1].officer_note, "");
    }

    /// AC2 at the seam: the shard-resolved half reaches the packet, per member and unmixed. What
    /// proves the fan-out RAN is the routing test against two shards holding different data
    /// (`world/guild_tests.rs`); this pins that nothing drops it on the way to the wire.
    #[test]
    fn each_entry_carries_its_own_name_level_class_and_area() {
        let m = roster_of(&with_roster());

        assert_eq!(m.members[0].name, "Ginger");
        assert_eq!(m.members[0].level, Level::new(60));
        assert_eq!(m.members[0].class, Class::Paladin);
        assert_eq!(m.members[0].area, Area::ElwynnForest);
        assert_eq!(m.members[1].name, "Vim");
        assert_eq!(m.members[1].level, Level::new(24));
        assert_eq!(m.members[1].class, Class::Mage);
        assert_eq!(m.members[1].area, Area::Durotar);
    }

    /// AC3: the online column is per member, and an offline one is listed rather than dropped.
    #[test]
    fn online_and_offline_members_report_their_own_status() {
        let m = roster_of(&with_roster());

        assert_eq!(m.members[0].status, GuildMember_GuildMemberStatus::Online);
        assert_eq!(
            m.members[1].status,
            GuildMember_GuildMemberStatus::Offline { time_offline: 0.0 }
        );
    }

    /// AC4: one `rights` entry per rank, in rank order — the client indexes it by a member's rank.
    #[test]
    fn rights_carries_one_entry_per_rank_in_rank_order() {
        use lyracore_shared::guild::{DEFAULT_RANK_RIGHTS, GUILD_RANK_COUNT};
        let m = roster_of(&with_roster());

        assert_eq!(m.rights.len(), GUILD_RANK_COUNT);
        assert_eq!(m.rights.as_slice(), &DEFAULT_RANK_RIGHTS);
    }

    /// AC5: the MOTD and the information text are the guild row's own, not blanks.
    #[test]
    fn the_roster_carries_the_guilds_motd_and_information_text() {
        let m = roster_of(&with_roster());

        assert_eq!(m.motd, "Raid at eight");
        assert_eq!(m.guild_info, "Founded on the Sunday");
    }

    /// AC7: a guildless caller is refused, not answered with an empty roster — the client leaves
    /// its panel open on silence.
    #[test]
    fn a_guildless_caller_is_refused_with_player_not_in_guild() {
        let actions = InMemoryGuildActions::default();

        let outbound = handled(
            dispatch_guild_action(&actions, player(), ClientOpcodeMessage::CMSG_GUILD_ROSTER)
                .unwrap(),
        );

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildPlayerNotInGuild
        );
    }

    /// A membership pointing at a guild the authority no longer has is the same fact from the
    /// client's side, and gets the same answer rather than an empty batch.
    #[test]
    fn a_membership_of_a_guild_that_is_gone_is_refused_the_same_way() {
        let actions = InMemoryGuildActions {
            membership: vec![(GINGER, 3)],
            ..Default::default()
        };

        let outbound = handled(
            dispatch_guild_action(&actions, player(), ClientOpcodeMessage::CMSG_GUILD_ROSTER)
                .unwrap(),
        );

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildPlayerNotInGuild
        );
    }

    /// A roster read that cannot reach its database propagates rather than looking like a caller
    /// who is in no guild.
    #[test]
    fn a_failed_roster_read_propagates_rather_than_refusing() {
        let actions = InMemoryGuildActions {
            read_error: Some("realm-core unreachable".into()),
            ..with_roster()
        };

        assert!(
            dispatch_guild_action(&actions, player(), ClientOpcodeMessage::CMSG_GUILD_ROSTER)
                .is_err()
        );
    }

    // ---- Guild chat (T5) ----

    /// AC1/AC2: a valid `/g` line reaches the durable layer with the caller's own guid and text,
    /// and answers with NOTHING — success has no `SMSG_GUILD_COMMAND_RESULT` (the line's own
    /// relay, fanned to every other online member plus the sender's echo, is the only feedback;
    /// see `world::tests` for the wire-level assertion of that fan-out).
    #[test]
    fn a_valid_chat_line_requests_delivery_and_answers_with_nothing() {
        let actions = InMemoryGuildActions::default();

        let outbound = guild_chat_action(&actions, GINGER, "for the Alliance!".into()).unwrap();

        assert_eq!(
            actions.chat_requests.lock().unwrap().as_slice(),
            &[(GINGER, "for the Alliance!".to_string())]
        );
        assert!(outbound.is_empty());
    }

    /// AC5: a caller with no guild answers `GuildPlayerNotInGuild` and delivers nothing else.
    #[test]
    fn a_chat_line_from_a_guildless_caller_answers_player_not_in_guild() {
        let actions = InMemoryGuildActions {
            chat_error: Some(err::NOT_IN_GUILD.to_string()),
            ..Default::default()
        };

        let outbound = guild_chat_action(&actions, GINGER, "hello?".into()).unwrap();

        assert_eq!(
            command_result_of(&outbound),
            (
                GuildCommand::Quit,
                String::new(),
                GuildCommandResult::GuildPlayerNotInGuild
            )
        );
    }

    /// Any OTHER gameplay refusal (empty message, not in world, …) is silently dropped, exactly
    /// like say/yell/party.
    #[test]
    fn an_unclassified_chat_refusal_is_dropped_rather_than_answered() {
        let actions = InMemoryGuildActions {
            chat_error: Some("empty message".to_string()),
            ..Default::default()
        };

        let outbound = guild_chat_action(&actions, GINGER, String::new()).unwrap();

        assert!(outbound.is_empty());
    }

    /// A lost reducer transport still propagates — chat is not exempt from the family's own Fatal
    /// classification.
    #[test]
    fn a_lost_reducer_transport_propagates_out_of_guild_chat_too() {
        let actions = InMemoryGuildActions {
            chat_error: Some("reducer transport disconnected".to_string()),
            ..Default::default()
        };

        let error = match guild_chat_action(&actions, GINGER, "hi".into()) {
            Err(error) => error,
            Ok(_) => panic!("a lost transport must not be swallowed as a refusal"),
        };

        assert!(format!("{error:#}").contains("reducer transport disconnected"));
    }
    /// AC1: the master's MOTD change reaches the durable layer with the caller's account/guid/text,
    /// and the caller — an online member — is answered directly with `SMSG_GUILD_EVENT(Motd)`
    /// carrying the new text.
    #[test]
    fn a_masters_motd_change_requests_the_write_and_answers_with_the_guild_event() {
        let actions = InMemoryGuildActions::default();

        let outbound =
            handled(dispatch_guild_action(&actions, player(), motd("Raid at 8pm")).unwrap());

        assert_eq!(
            actions.motd_requests.lock().unwrap().as_slice(),
            &[(7, GINGER, "Raid at 8pm".to_string())]
        );
        match outbound.as_slice() {
            [Outbound::One(ServerOpcodeMessage::SMSG_GUILD_EVENT(m))] => {
                assert_eq!(m.event, wow_world_messages::vanilla::GuildEvent::Motd);
                assert_eq!(m.event_descriptions, vec!["Raid at 8pm".to_string()]);
            }
            other => panic!(
                "expected one SMSG_GUILD_EVENT, got {} message(s)",
                other.len()
            ),
        }
    }

    /// AC4: an EMPTY motd is a valid write (it clears the MOTD) — not a second refusal path. The
    /// seam must request it and answer success exactly like a non-empty one.
    #[test]
    fn an_empty_motd_is_requested_and_answered_as_a_success_not_a_refusal() {
        let actions = InMemoryGuildActions::default();

        let outbound = handled(dispatch_guild_action(&actions, player(), motd("")).unwrap());

        assert_eq!(
            actions.motd_requests.lock().unwrap().as_slice(),
            &[(7, GINGER, String::new())]
        );
        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_GUILD_EVENT(_))]
        ));
    }

    /// AC2: a non-master's MOTD change is refused with `GuildPermissionsOrLeader`, and changes
    /// nothing else on this connection (the ONE reply is the refusal, no event).
    #[test]
    fn a_non_masters_motd_change_is_refused_with_permissions_or_leader() {
        let actions = InMemoryGuildActions {
            motd_error: Some(err::NOT_GUILD_MASTER.into()),
            ..Default::default()
        };

        let outbound = handled(dispatch_guild_action(&actions, player(), motd("Nope")).unwrap());

        assert_eq!(
            command_result_of(&outbound),
            (
                GuildCommand::Founder,
                String::new(),
                GuildCommandResult::GuildPermissionsOrLeader
            )
        );
    }

    /// AC5: info text behaves as the MOTD for gating and storage — same refusal mapping — but a
    /// SUCCESSFUL change never answers with an event; the acceptance criterion is explicit that
    /// there is no broadcast.
    #[test]
    fn info_text_gates_like_the_motd_but_a_success_carries_no_broadcast() {
        let actions = InMemoryGuildActions::default();

        let outbound =
            handled(dispatch_guild_action(&actions, player(), info_text("We raid Naxx")).unwrap());

        assert_eq!(
            actions.info_text_requests.lock().unwrap().as_slice(),
            &[(7, GINGER, "We raid Naxx".to_string())]
        );
        assert!(outbound.is_empty(), "info text must not broadcast");

        let refused = InMemoryGuildActions {
            info_text_error: Some(err::NOT_GUILD_MASTER.into()),
            ..Default::default()
        };
        let outbound =
            handled(dispatch_guild_action(&refused, player(), info_text("Nope")).unwrap());
        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildPermissionsOrLeader
        );
    }

    /// AC6: a public-note change (own or another's — the module's gate decides which; the seam only
    /// carries the typed name through) reaches the durable layer and answers with no broadcast on
    /// success, `GuildPermissionsOrLeader` when the module refuses it.
    #[test]
    fn public_note_requests_the_write_by_name_and_carries_no_broadcast_on_success() {
        let actions = InMemoryGuildActions::default();

        let outbound = handled(
            dispatch_guild_action(&actions, player(), set_public_note("Bob", "reliable tank"))
                .unwrap(),
        );

        assert_eq!(
            actions.public_note_requests.lock().unwrap().as_slice(),
            &[(7, GINGER, "Bob".to_string(), "reliable tank".to_string())]
        );
        assert!(outbound.is_empty());

        let refused = InMemoryGuildActions {
            public_note_error: Some(err::NOT_GUILD_MASTER.into()),
            ..Default::default()
        };
        let outbound = handled(
            dispatch_guild_action(&refused, player(), set_public_note("Carol", "x")).unwrap(),
        );
        assert_eq!(
            command_result_of(&outbound),
            (
                GuildCommand::Founder,
                "Carol".to_string(),
                GuildCommandResult::GuildPermissionsOrLeader
            )
        );
    }

    /// AC7: the officer note is master-only, same request/refusal shape as the public note.
    #[test]
    fn officer_note_requests_the_write_by_name_and_gates_on_the_master() {
        let actions = InMemoryGuildActions::default();

        let outbound = handled(
            dispatch_guild_action(
                &actions,
                player(),
                set_officer_note("Bob", "watch this one"),
            )
            .unwrap(),
        );

        assert_eq!(
            actions.officer_note_requests.lock().unwrap().as_slice(),
            &[(7, GINGER, "Bob".to_string(), "watch this one".to_string())]
        );
        assert!(outbound.is_empty());

        let refused = InMemoryGuildActions {
            officer_note_error: Some(err::NOT_GUILD_MASTER.into()),
            ..Default::default()
        };
        let outbound = handled(
            dispatch_guild_action(&refused, player(), set_officer_note("Dave", "x")).unwrap(),
        );
        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildPermissionsOrLeader
        );
    }

    /// An unresolved note target answers `GuildPlayerNotFoundS` — the gateway's own resolution
    /// failure, classified through the same vocabulary as the module's refusals.
    #[test]
    fn an_unresolved_note_target_answers_player_not_found() {
        let actions = InMemoryGuildActions {
            public_note_error: Some(err::TARGET_NOT_IN_GUILD.into()),
            ..Default::default()
        };

        let outbound = handled(
            dispatch_guild_action(&actions, player(), set_public_note("Nobody", "x")).unwrap(),
        );

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildPlayerNotFoundS
        );
    }

    /// A guildless actor is `GuildPlayerNotInGuild` — the module's `NOT_IN_GUILD` classified the
    /// same way for every setter in this family.
    #[test]
    fn a_guildless_actor_setting_the_motd_answers_not_in_guild() {
        let actions = InMemoryGuildActions {
            motd_error: Some(err::NOT_IN_GUILD.into()),
            ..Default::default()
        };

        let outbound = handled(dispatch_guild_action(&actions, player(), motd("x")).unwrap());

        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildPlayerNotInGuild
        );
    }

    /// An unclassified refusal still answers (never a silently dropped dialog), and a lost reducer
    /// transport is the one failure that propagates — mirroring `guild_create`'s own coverage, once
    /// for the setter family.
    #[test]
    fn setters_answer_unclassified_refusals_and_propagate_a_lost_transport() {
        let unclassified = InMemoryGuildActions {
            motd_error: Some("something the module has not told the gateway about".into()),
            ..Default::default()
        };
        let outbound = handled(dispatch_guild_action(&unclassified, player(), motd("x")).unwrap());
        assert_eq!(
            command_result_of(&outbound).2,
            GuildCommandResult::GuildInternal
        );

        let fatal = InMemoryGuildActions {
            motd_error: Some("reducer transport disconnected".into()),
            ..Default::default()
        };
        let error = match dispatch_guild_action(&fatal, player(), motd("x")) {
            Err(error) => error,
            Ok(_) => panic!("a lost transport must not be swallowed as a refusal"),
        };
        assert!(format!("{error:#}").contains("reducer transport disconnected"));
    }
}
