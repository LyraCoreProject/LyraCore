//! Guild family: founding a guild and the two screens that read one back all enter through
//! `dispatch_guild_action`. Every guild read and request the world session needs lives on
//! `GuildActionStore`; the routing behind those three methods (realm-core versus the player's own
//! shard) is `world::guild`'s job, so nothing here knows how many databases there are.

use super::super::*;
use crate::world::guild::GuildView;
use wow_world_messages::vanilla::{GuildCommand, GuildCommandResult};

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
        let target_guid = crate::world::guild::member_by_name(self, self_guid, target_name)?;
        crate::world::guild::run(
            self,
            account_id,
            self_guid,
            crate::world::guild::Op::Remove {
                target_guid,
                target_name: target_name.to_string(),
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

/// The shape every teardown verb shares: one durable request, then the wire's verdict either way. A
/// client that hears nothing leaves its confirmation dialog open, so a refusal is always answered.
fn teardown(
    command: GuildCommand,
    subject: &str,
    verb: &str,
    request: impl FnOnce() -> Result<()>,
) -> Result<GuildActionOutcome> {
    if let Err(e) = request() {
        if classify_guild_action_error(&e) == GuildActionErrorClass::Fatal {
            return Err(e);
        }
        log::debug!("world: guild {verb} refused: {e:#}");
        return Ok(GuildActionOutcome::Handled {
            outbound: command_result(command, subject, teardown_result_for(&e)),
        });
    }
    // Wire code 0 — vanilla's "no message/error", i.e. success. See `guild_create`.
    Ok(GuildActionOutcome::Handled {
        outbound: command_result(command, subject, GuildCommandResult::PlayerNoMoreInGuild),
    })
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
        /// The teardown verbs' requests: `(account_id, self_guid, subject)`. One log, because a
        /// dispatch test cares which verb ran and with what, and the verb is named by the test.
        teardown_requests: Mutex<Vec<(&'static str, u64, u64, String)>>,
        /// When set, every teardown verb refuses with this message — the durable layer's answer,
        /// which the seam's only job is to classify.
        teardown_error: Option<String>,
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
}
