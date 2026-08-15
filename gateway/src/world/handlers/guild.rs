//! Guild family: founding a guild and the two screens that read one back all enter through
//! `dispatch_guild_action`. Every guild read and request the world session needs lives on
//! `GuildActionStore`; the routing behind those three methods (realm-core versus the player's own
//! shard) is `world::guild`'s job, so nothing here knows how many databases there are.

use super::super::*;
use crate::world::guild::GuildView;
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
        let target_guid = crate::world::party::resolve_by_name(self, target_name)?
            .ok_or_else(|| anyhow::anyhow!("{}", lyracore_shared::guild::err::PLAYER_NOT_FOUND))?;
        crate::world::guild::run(
            self,
            account_id,
            self_guid,
            crate::world::guild::Op::SetPublicNote {
                target_guid,
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
        let target_guid = crate::world::party::resolve_by_name(self, target_name)?
            .ok_or_else(|| anyhow::anyhow!("{}", lyracore_shared::guild::err::PLAYER_NOT_FOUND))?;
        crate::world::guild::run(
            self,
            account_id,
            self_guid,
            crate::world::guild::Op::SetOfficerNote {
                target_guid,
                note: note.to_string(),
            },
        )
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

/// Map an MOTD/info-text/note refusal onto the wire's own result code — the same classified-string
/// contract [`create_result_for`] reads, extended with the family's other two errors:
/// [`lyracore_shared::guild::err::NOT_IN_GUILD`] (the actor is guildless, or a note's target is not
/// a fellow member) and [`lyracore_shared::guild::err::NOT_GUILD_MASTER`] (D3's only permission
/// check). [`lyracore_shared::guild::err::PLAYER_NOT_FOUND`] is not a module string — it is what the
/// gateway itself raises when a note target's name does not resolve — but it rides the same
/// classified vocabulary so this is still one lookup.
fn setter_result_for(error: &anyhow::Error) -> GuildCommandResult {
    use lyracore_shared::guild::err;
    let text = format!("{error:#}");
    if text.contains(err::NOT_GUILD_MASTER) {
        GuildCommandResult::GuildPermissionsOrLeader
    } else if text.contains(err::PLAYER_NOT_FOUND) {
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

/// `CMSG_GUILD_CREATE`. The durable request runs first; the reply is the wire's own verdict either
/// way, because a client that hears nothing leaves its create dialog open forever.
fn guild_create<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    name: &str,
) -> Result<GuildActionOutcome> {
    if let Err(e) = store.guild_create(account_id, self_guid, name) {
        if classify_guild_action_error(&e) == GuildActionErrorClass::Fatal {
            return Err(e);
        }
        log::debug!("world: guild create refused (account {account_id}): {e:#}");
        return Ok(GuildActionOutcome::Handled {
            outbound: command_result(GuildCommand::Create, name, create_result_for(&e)),
        });
    }
    // Success is result code 0 — the wire spells it `PlayerNoMoreInGuild`, but vanilla's own name
    // for that code is "no message/error". The client re-queries the guild off the back of it.
    Ok(GuildActionOutcome::Handled {
        outbound: command_result(
            GuildCommand::Create,
            name,
            GuildCommandResult::PlayerNoMoreInGuild,
        ),
    })
}

/// `CMSG_GUILD_MOTD`. Master-only (D3); an empty string is a valid write that CLEARS the MOTD, not a
/// refusal. On success the caller — themselves an online member — is answered directly with
/// `SMSG_GUILD_EVENT(Motd)`; the module also drops one `GuildEvent(MOTD)` row per OTHER member
/// (`set_guild_text`'s core), the durable half of "every online member sees it live" whose delivery
/// is the `game_guild_event` relay's job (the `game_group_event` shape, armed in
/// `stdb/subscriptions.rs`/`stdb/world_view.rs` — outside this seam's file ownership).
///
/// `GuildCommand` has no MOTD/notes-specific variant (vanilla's wire only defines
/// Create/Invite/Quit/Founder/Unknown19/Unknown20) — `Founder` is the closest of the six to "you are
/// not the guild founder/master", so every refusal in this family answers with it.
fn guild_set_motd<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    motd: &str,
) -> Result<GuildActionOutcome> {
    if let Err(e) = store.guild_set_motd(account_id, self_guid, motd) {
        if classify_guild_action_error(&e) == GuildActionErrorClass::Fatal {
            return Err(e);
        }
        log::debug!("world: guild motd refused (account {account_id}): {e:#}");
        return Ok(GuildActionOutcome::Handled {
            outbound: command_result(GuildCommand::Founder, "", setter_result_for(&e)),
        });
    }
    Ok(GuildActionOutcome::Handled {
        outbound: vec![Outbound::One(ServerOpcodeMessage::SMSG_GUILD_EVENT(
            Box::new(SMSG_GUILD_EVENT {
                event: WireGuildEvent::Motd,
                event_descriptions: vec![motd.to_string()],
            }),
        ))],
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
    if let Err(e) = store.guild_set_info_text(account_id, self_guid, text) {
        if classify_guild_action_error(&e) == GuildActionErrorClass::Fatal {
            return Err(e);
        }
        log::debug!("world: guild info text refused (account {account_id}): {e:#}");
        return Ok(GuildActionOutcome::Handled {
            outbound: command_result(GuildCommand::Founder, "", setter_result_for(&e)),
        });
    }
    Ok(GuildActionOutcome::Handled {
        outbound: Vec::new(),
    })
}

/// `CMSG_GUILD_SET_PUBLIC_NOTE`. The wire carries the target by NAME; an unresolved name answers
/// `GuildPlayerNotFoundS` the same way an unknown invite target would. A member may set their own
/// row; setting anyone else's is master-only (D3). No broadcast on success.
fn guild_set_public_note<St: GuildActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    target_name: &str,
    note: &str,
) -> Result<GuildActionOutcome> {
    if let Err(e) = store.guild_set_public_note(account_id, self_guid, target_name, note) {
        if classify_guild_action_error(&e) == GuildActionErrorClass::Fatal {
            return Err(e);
        }
        log::debug!("world: guild public note refused (account {account_id}): {e:#}");
        return Ok(GuildActionOutcome::Handled {
            outbound: command_result(GuildCommand::Founder, target_name, setter_result_for(&e)),
        });
    }
    Ok(GuildActionOutcome::Handled {
        outbound: Vec::new(),
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
    if let Err(e) = store.guild_set_officer_note(account_id, self_guid, target_name, note) {
        if classify_guild_action_error(&e) == GuildActionErrorClass::Fatal {
            return Err(e);
        }
        log::debug!("world: guild officer note refused (account {account_id}): {e:#}");
        return Ok(GuildActionOutcome::Handled {
            outbound: command_result(GuildCommand::Founder, target_name, setter_result_for(&e)),
        });
    }
    Ok(GuildActionOutcome::Handled {
        outbound: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::guild::{err, DEFAULT_RANK_NAMES};
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{CMSG_GUILD_CREATE, CMSG_GUILD_QUERY, CMSG_PING};

    /// Records the durable requests the seam makes and answers its reads from canned rows, so each
    /// dispatch branch is exercised without a database.
    #[derive(Default)]
    struct InMemoryGuildActions {
        create_requests: Mutex<Vec<(u64, u64, String)>>,
        create_error: Option<String>,
        guilds: Vec<GuildView>,
        membership: Vec<(u64, u64)>,
        read_error: Option<String>,
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

        fn guild_set_motd(&self, account_id: u64, self_guid: u64, motd: &str) -> Result<()> {
            self.motd_requests
                .lock()
                .unwrap()
                .push((account_id, self_guid, motd.to_string()));
            self.motd_error
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
            public_note_error: Some(err::PLAYER_NOT_FOUND.into()),
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
