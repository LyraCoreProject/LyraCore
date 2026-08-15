//! Guild family: founding a guild and the two screens that read one back all enter through
//! `dispatch_guild_action`. Every guild read and request the world session needs lives on
//! `GuildActionStore`; the routing behind those three methods (realm-core versus the player's own
//! shard) is `world::guild`'s job, so nothing here knows how many databases there are.

use super::super::*;
use crate::world::guild::{GuildRoster, GuildView};
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

    /// `guild_id`'s roster, ALREADY RENDERED: the authority's guids, ranks and notes with each
    /// member's name, level, class, area and online flag filled in from the shards. `None` = no
    /// such guild. The fan-out belongs to the routing layer, so nothing here knows how many
    /// databases answered.
    fn guild_roster(&self, guild_id: u64) -> Result<Option<GuildRoster>>;
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

    fn guild_roster(&self, guild_id: u64) -> Result<Option<GuildRoster>> {
        crate::world::guild::roster(self, guild_id)
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
        ClientOpcodeMessage::CMSG_GUILD_ROSTER => guild_roster(store, self_guid),
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
        /// Rosters keyed by guild id, ALREADY rendered — the shard fan-out is the routing layer's,
        /// and is pinned there (`world/guild_tests.rs`).
        rosters: Vec<(u64, GuildRoster)>,
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
}
