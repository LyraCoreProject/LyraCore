//! Chat Channel dispatcher: join, leave, password, the member list and the owner query. Ops are
//! Durable Requests on Realm-core; their success notices come back on the Relay
//! (`stdb::world_view::channel_notice_appeared`). A Refusal rolls back its transaction, so this
//! file answers it to the actor alone. Channel speech is a `CMSG_MESSAGECHAT` kind and lives in
//! `chat.rs`.

use super::super::*;
use super::chat::{is_transport_failure, ChatActionPlayer, ChatActionStore, SpeakerFacts};
use lyracore_shared::channel::{channel_op, notice, ChannelRefusal};
use lyracore_shared::faction::team_for_race;

/// One channel op on its way to the `realm_channel_op` reducer. The Coordinator conveys every field
/// but the speaker's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChannelRequest {
    /// As the client typed it.
    pub(crate) channel_name: String,
    pub(crate) password: String,
    pub(crate) target_guid: u64,
    pub(crate) target_name: String,
    pub(crate) target_race: u8,
    pub(crate) target_ignores_actor: bool,
    pub(crate) speaker: SpeakerFacts,
}

/// How the Module answered one channel op. A timeout, transport or SDK failure stays an `Err` with
/// an unknown durable outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChannelOutcome {
    Done,
    Refused(ChannelRefusal),
}

/// One Chat Channel as the Realm-core cache holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChannelRoster {
    /// The stored spelling.
    pub(crate) name: String,
    /// Wire channel flags.
    pub(crate) flags: u8,
    /// 0 for none.
    pub(crate) owner_guid: u64,
    /// The owner's name from whichever World Shard holds the Character. Empty when unresolved.
    pub(crate) owner_name: String,
    /// `(guid, raw member flags)` in join order.
    pub(crate) members: Vec<(u64, u8)>,
}

/// A Character [`ChannelActionStore::online_character_by_name`] resolved: its guid, race and
/// canonical spelling, the identity a targeted op conveys to the Module once found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedTarget {
    pub(crate) guid: u64,
    pub(crate) race: u8,
    pub(crate) name: String,
}

pub(crate) trait ChannelActionStore: ChatActionStore {
    /// Durable Request on Realm-core. The Coordinator picks the database; handlers never do.
    fn channel_op(
        &self,
        actor_guid: u64,
        op: u8,
        request: ChannelRequest,
    ) -> Result<ChannelOutcome>;
    /// Durable Read of the Realm-core cache. `None` when `team` has no channel by that name.
    fn channel_roster(&self, team: u32, channel_name: &str) -> Result<Option<ChannelRoster>>;
    /// The first `session_online` Character named `name`, realm-wide. `None` when no online
    /// Character carries that name.
    fn online_character_by_name(&self, name: &str) -> Result<Option<ResolvedTarget>>;
    /// Does `owner_guid` have `other_guid` on its ignore list, read from wherever the owner's
    /// contact rows live?
    fn ignores(&self, owner_guid: u64, other_guid: u64) -> Result<bool>;
}

impl ChannelActionStore for crate::stdb::Coordinator {
    fn channel_op(
        &self,
        actor_guid: u64,
        op: u8,
        request: ChannelRequest,
    ) -> Result<ChannelOutcome> {
        crate::stdb::Coordinator::channel_op(self, actor_guid, op, request)
    }

    fn channel_roster(&self, team: u32, channel_name: &str) -> Result<Option<ChannelRoster>> {
        crate::stdb::Coordinator::channel_roster(self, team, channel_name)
    }

    fn online_character_by_name(&self, name: &str) -> Result<Option<ResolvedTarget>> {
        resolve_online_character(self, name)
    }

    fn ignores(&self, owner_guid: u64, other_guid: u64) -> Result<bool> {
        whisper::ignored_anywhere(self, owner_guid, other_guid)
    }
}

/// The read [`ChannelActionStore::online_character_by_name`] and its Fakes share: every op that
/// names a Character resolves it realm-wide and requires it online, the same shape whisper's
/// ONLINE gate uses (`whisper::run`). A channel can only name a Character presently reachable to
/// notify.
pub(crate) fn resolve_online_character<St: WorldStore + ?Sized>(
    store: &St,
    name: &str,
) -> Result<Option<ResolvedTarget>> {
    for guid in presence::resolve_all_by_name(store, name)? {
        if let Some(character) = presence::of(store, guid)? {
            if character.session_online {
                return Ok(Some(ResolvedTarget {
                    guid: character.guid,
                    race: character.race,
                    name: character.name,
                }));
            }
        }
    }
    Ok(None)
}

pub(crate) enum ChannelActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

/// What CHANNEL_OWNER names when a channel has no owner (vm:Channel.cpp:881-890).
const NO_OWNER: &str = "Nobody";
/// What CHANNEL_OWNER names when the owner's Character cannot be found (cm:Channel.cpp:817-824).
const OWNER_NOT_FOUND: &str = "PLAYER_NOT_FOUND";

/// Consume the channel opcodes and pass everything else on.
pub(crate) fn dispatch_channel_action<St: ChannelActionStore + ?Sized>(
    store: &St,
    player: ChatActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<ChannelActionOutcome> {
    let outbound = match msg {
        // An empty name is dropped (cm:ChannelHandler.cpp:66-67, cm:ChannelHandler.cpp:88-89).
        ClientOpcodeMessage::CMSG_JOIN_CHANNEL(join) if join.channel_name.is_empty() => Vec::new(),
        ClientOpcodeMessage::CMSG_JOIN_CHANNEL(join) => run_op(
            store,
            player,
            channel_op::JOIN,
            join.channel_name,
            join.channel_password,
        )?,
        ClientOpcodeMessage::CMSG_LEAVE_CHANNEL(leave) if leave.channel_name.is_empty() => {
            Vec::new()
        }
        ClientOpcodeMessage::CMSG_LEAVE_CHANNEL(leave) => run_op(
            store,
            player,
            channel_op::LEAVE,
            leave.channel_name,
            String::new(),
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_PASSWORD(password) => run_op(
            store,
            player,
            channel_op::PASSWORD,
            password.channel_name,
            password.channel_password,
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_SET_OWNER(set_owner) => run_targeted_op(
            store,
            player,
            channel_op::SET_OWNER,
            set_owner.channel_name,
            set_owner.new_owner,
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_MODERATOR(moderator) => run_targeted_op(
            store,
            player,
            channel_op::MODERATOR,
            moderator.channel_name,
            moderator.player_name,
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_UNMODERATOR(unmoderator) => run_targeted_op(
            store,
            player,
            channel_op::UNMODERATOR,
            unmoderator.channel_name,
            unmoderator.player_name,
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_MUTE(mute) => run_targeted_op(
            store,
            player,
            channel_op::MUTE,
            mute.channel_name,
            mute.player_name,
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_UNMUTE(unmute) => run_targeted_op(
            store,
            player,
            channel_op::UNMUTE,
            unmute.channel_name,
            unmute.player_name,
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_KICK(kick) => run_targeted_op(
            store,
            player,
            channel_op::KICK,
            kick.channel_name,
            kick.player_name,
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_BAN(ban) => run_targeted_op(
            store,
            player,
            channel_op::BAN,
            ban.channel_name,
            ban.player_name,
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_UNBAN(unban) => run_targeted_op(
            store,
            player,
            channel_op::UNBAN,
            unban.channel_name,
            unban.player_name,
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_INVITE(invite) => run_targeted_op(
            store,
            player,
            channel_op::INVITE,
            invite.channel_name,
            invite.player_name,
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_ANNOUNCEMENTS(announcements) => run_op(
            store,
            player,
            channel_op::ANNOUNCEMENTS,
            announcements.channel_name,
            String::new(),
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_MODERATE(moderate) => run_op(
            store,
            player,
            channel_op::MODERATE,
            moderate.channel_name,
            String::new(),
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_LIST(list) => {
            read_roster(store, player, list.channel_name, |roster| {
                Outbound::One(ServerOpcodeMessage::SMSG_CHANNEL_LIST(Box::new(
                    codec::channel::build_channel_list(roster.name, roster.flags, &roster.members),
                )))
            })?
        }
        ClientOpcodeMessage::CMSG_CHANNEL_OWNER(owner) => {
            let typed = owner.channel_name.clone();
            read_roster(store, player, owner.channel_name, |roster| {
                let owner_name = match (roster.owner_guid, roster.owner_name) {
                    (0, _) => NO_OWNER.to_string(),
                    (_, name) if name.is_empty() => OWNER_NOT_FOUND.to_string(),
                    (_, name) => name,
                };
                notice_outbound(codec::channel::ChannelNoticeView {
                    notice: notice::CHANNEL_OWNER,
                    channel_name: typed,
                    text: owner_name,
                    ..Default::default()
                })
            })?
        }
        other => return Ok(ChannelActionOutcome::PassThrough(other)),
    };
    Ok(ChannelActionOutcome::Handled { outbound })
}

/// The actor and Speaker Facts every channel op needs. `None` when the caller has no live entity
/// to act as (a session at Character Select, or between shards), the same silent drop every
/// channel op takes for that case.
fn actor_and_speaker<St: ChannelActionStore + ?Sized>(
    store: &St,
    player: ChatActionPlayer,
) -> Result<Option<(u64, SpeakerFacts)>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(None);
    };
    let Some(speaker) = store.speaker_facts(actor_guid)? else {
        return Ok(None);
    };
    Ok(Some((actor_guid, speaker)))
}

/// What a Refusal from [`submit_channel_op`] names in its notice: the channel as the client typed
/// it, and who or what to name as `subject_guid`/`target_name`. The actor for an untargeted op;
/// the resolved (or unresolved-sentinel) target for one that names a Character.
struct RefusalContext {
    channel_name: String,
    subject_guid: u64,
    target_name: String,
}

/// Submit one built `request` as `op` and translate the outcome. Success notices return on the
/// Relay; a Refusal is answered here, from `refusal`. Only a lost reducer transport is fatal;
/// every other failure is logged at debug and dropped, keeping the World Session up.
fn submit_channel_op<St: ChannelActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    actor_guid: u64,
    op: u8,
    request: ChannelRequest,
    refusal: RefusalContext,
) -> Result<Vec<Outbound>> {
    match store.channel_op(actor_guid, op, request) {
        Ok(ChannelOutcome::Done) => Ok(Vec::new()),
        Ok(ChannelOutcome::Refused(tag)) => Ok(vec![refusal_notice(
            tag,
            refusal.channel_name,
            refusal.subject_guid,
            refusal.target_name,
        )]),
        Err(error) if is_transport_failure(&error) => Err(error),
        Err(error) => {
            log::debug!("world: channel op {op} dropped (account {account_id}): {error:#}");
            Ok(Vec::new())
        }
    }
}

/// Run one op as the player: JOIN, LEAVE, PASSWORD, ANNOUNCEMENTS and MODERATE. None of these
/// name another Character.
fn run_op<St: ChannelActionStore + ?Sized>(
    store: &St,
    player: ChatActionPlayer,
    op: u8,
    channel_name: String,
    password: String,
) -> Result<Vec<Outbound>> {
    let Some((actor_guid, speaker)) = actor_and_speaker(store, player)? else {
        return Ok(Vec::new());
    };
    let request = ChannelRequest {
        channel_name: channel_name.clone(),
        password,
        target_guid: 0,
        target_name: String::new(),
        target_race: 0,
        target_ignores_actor: false,
        speaker,
    };
    submit_channel_op(
        store,
        player.account_id,
        actor_guid,
        op,
        request,
        RefusalContext {
            channel_name,
            subject_guid: actor_guid,
            target_name: String::new(),
        },
    )
}

/// A Store read behind [`run_targeted_op`] that must not end the World Session unless it fails
/// with a genuine transport loss. Anything else is logged at debug and answered as absent, the
/// same way an unresolved name is: this Gateway process cannot finish the op either way, and a
/// name lookup or an ignore-list read that a peer Shard cannot currently answer is no different
/// from a target this Gateway cannot see.
fn recoverable<T>(op: u8, account_id: u64, result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if is_transport_failure(&error) => Err(error),
        Err(error) => {
            log::debug!(
                "world: channel op {op} target read dropped (account {account_id}): {error:#}"
            );
            Ok(None)
        }
    }
}

/// Run one op that names another Character: SET_OWNER, MODERATOR, UNMODERATOR, MUTE, UNMUTE, KICK,
/// BAN, UNBAN and INVITE. The target is resolved realm-wide and must be online
/// (cm:Channel.cpp:192-199, cm:Channel.cpp:262-269, cm:Channel.cpp:348-355, cm:Channel.cpp:416-423,
/// cm:Channel.cpp:678-685). cmangos checks membership, then rights, then the target name, so an
/// unresolved name still reaches the Module: it goes out as `target_guid` 0, the sentinel no real
/// Character ever holds, and the op core runs its own NotMember, NotModerator or NotOwner Gate
/// first and only then answers PLAYER_NOT_FOUND. A target lookup that fails without losing
/// transport degrades the same way, as [`recoverable`] documents.
///
/// INVITE alone also reads whether a RESOLVED target ignores the actor; that read has no name to
/// fall back on, so a failure there (again, anything short of a transport loss) answers
/// PLAYER_NOT_FOUND directly and skips the Durable Request, since there is nothing further to
/// learn from one.
fn run_targeted_op<St: ChannelActionStore + ?Sized>(
    store: &St,
    player: ChatActionPlayer,
    op: u8,
    channel_name: String,
    typed_name: String,
) -> Result<Vec<Outbound>> {
    let Some((actor_guid, speaker)) = actor_and_speaker(store, player)? else {
        return Ok(Vec::new());
    };
    let resolved = recoverable(
        op,
        player.account_id,
        store.online_character_by_name(&typed_name),
    )?
    .flatten();
    let target = resolved.unwrap_or(ResolvedTarget {
        guid: 0,
        race: 0,
        name: typed_name,
    });
    let target_ignores_actor = if target.guid != 0 && op == channel_op::INVITE {
        let Some(ignores) = recoverable(
            op,
            player.account_id,
            store.ignores(target.guid, actor_guid),
        )?
        else {
            return Ok(vec![refusal_notice(
                ChannelRefusal::PlayerNotFound,
                channel_name,
                target.guid,
                target.name,
            )]);
        };
        ignores
    } else {
        false
    };
    let request = ChannelRequest {
        channel_name: channel_name.clone(),
        password: String::new(),
        target_guid: target.guid,
        target_name: target.name.clone(),
        target_race: target.race,
        target_ignores_actor,
        speaker,
    };
    submit_channel_op(
        store,
        player.account_id,
        actor_guid,
        op,
        request,
        RefusalContext {
            channel_name,
            subject_guid: target.guid,
            target_name: target.name,
        },
    )
}

/// Answer a read with `answer` when the player is a member of the named channel, else NOT_MEMBER
/// with the name the client typed (cm:Channel.cpp:446-456, cm:Channel.cpp:479-488).
fn read_roster<St: ChannelActionStore + ?Sized>(
    store: &St,
    player: ChatActionPlayer,
    channel_name: String,
    answer: impl FnOnce(ChannelRoster) -> Outbound,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let Some(speaker) = store.speaker_facts(actor_guid)? else {
        return Ok(Vec::new());
    };
    let roster = match store.channel_roster(team_for_race(speaker.race), &channel_name) {
        Ok(roster) => roster,
        Err(error) => {
            log::debug!(
                "world: channel read dropped (account {}): {error:#}",
                player.account_id
            );
            return Ok(Vec::new());
        }
    };
    let member = roster.filter(|roster| roster.members.iter().any(|&(guid, _)| guid == actor_guid));
    Ok(vec![match member {
        Some(roster) => answer(roster),
        None => refusal_notice(
            ChannelRefusal::NotMember,
            channel_name,
            actor_guid,
            String::new(),
        ),
    }])
}

/// The notice a Refusal answers to the actor alone, naming the channel as the client typed it.
/// PLAYER_ALREADY_MEMBER names `subject_guid`. The name notices carry `target_name`: the name the
/// client typed when the target could not be resolved, otherwise the target's canonical resolved
/// spelling, the same normalization cmangos applies through `normalizePlayerName`.
pub(super) fn refusal_notice(
    refusal: ChannelRefusal,
    channel_name: String,
    subject_guid: u64,
    target_name: String,
) -> Outbound {
    notice_outbound(codec::channel::ChannelNoticeView {
        notice: refusal.notice(),
        channel_name,
        subject_guid,
        text: target_name,
        ..Default::default()
    })
}

fn notice_outbound(view: codec::channel::ChannelNoticeView) -> Outbound {
    let (opcode, body) = codec::channel::notify(&view);
    Outbound::Raw { opcode, body }
}

#[cfg(test)]
mod tests {
    use super::super::chat::{ChatOutcome, RealmChatRequest, WhisperRequest, WhisperTargetFacts};
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        CMSG_CHANNEL_ANNOUNCEMENTS, CMSG_CHANNEL_BAN, CMSG_CHANNEL_INVITE, CMSG_CHANNEL_KICK,
        CMSG_CHANNEL_LIST, CMSG_CHANNEL_MODERATE, CMSG_CHANNEL_MODERATOR, CMSG_CHANNEL_MUTE,
        CMSG_CHANNEL_OWNER, CMSG_CHANNEL_PASSWORD, CMSG_CHANNEL_SET_OWNER, CMSG_CHANNEL_UNBAN,
        CMSG_CHANNEL_UNMODERATOR, CMSG_CHANNEL_UNMUTE, CMSG_JOIN_CHANNEL, CMSG_LEAVE_CHANNEL,
        CMSG_PING,
    };

    #[derive(Default)]
    struct InMemoryChannelActions {
        facts: Option<SpeakerFacts>,
        outcome: Option<Result<ChannelOutcome, String>>,
        roster: Option<ChannelRoster>,
        roster_failure: bool,
        /// What `online_character_by_name` resolves a typed name to. `None` answers
        /// PLAYER_NOT_FOUND, as an unknown or offline Character does.
        online: Option<ResolvedTarget>,
        /// `online_character_by_name` fails with this non-transport message instead of resolving.
        lookup_failure: Option<String>,
        /// What `ignores` answers for every pair.
        ignored: bool,
        /// `ignores` fails with this non-transport message instead of answering.
        ignore_failure: Option<String>,
        ignore_reads: Mutex<u32>,
        ops: Mutex<Vec<(u64, u8, ChannelRequest)>>,
        roster_reads: Mutex<Vec<(u32, String)>>,
    }

    impl ChatActionStore for InMemoryChannelActions {
        fn speaker_facts(&self, _speaker_guid: u64) -> Result<Option<SpeakerFacts>> {
            Ok(self.facts.clone())
        }

        fn realm_chat(
            &self,
            _speaker_guid: u64,
            _request: RealmChatRequest,
        ) -> Result<ChatOutcome> {
            Ok(ChatOutcome::Delivered)
        }

        fn set_away(&self, _speaker_guid: u64, _kind: u8, _message: String) -> Result<()> {
            Ok(())
        }

        fn whisper_target(
            &self,
            _speaker_guid: u64,
            _typed_name: &str,
        ) -> Result<Option<WhisperTargetFacts>> {
            Ok(None)
        }

        fn realm_whisper(
            &self,
            _speaker_guid: u64,
            _request: WhisperRequest,
        ) -> Result<ChatOutcome> {
            Ok(ChatOutcome::Delivered)
        }

        fn speaker_gm_level(&self, _speaker_guid: u64) -> Result<u8> {
            Ok(0)
        }
    }

    impl ChannelActionStore for InMemoryChannelActions {
        fn channel_op(
            &self,
            actor_guid: u64,
            op: u8,
            request: ChannelRequest,
        ) -> Result<ChannelOutcome> {
            self.ops.lock().unwrap().push((actor_guid, op, request));
            match &self.outcome {
                None => Ok(ChannelOutcome::Done),
                Some(Ok(outcome)) => Ok(*outcome),
                Some(Err(failure)) => Err(anyhow::anyhow!("{failure}")),
            }
        }

        fn channel_roster(&self, team: u32, channel_name: &str) -> Result<Option<ChannelRoster>> {
            self.roster_reads
                .lock()
                .unwrap()
                .push((team, channel_name.to_string()));
            if self.roster_failure {
                anyhow::bail!("realm-core database lyracore-realm is not connected");
            }
            Ok(self.roster.clone())
        }

        fn online_character_by_name(&self, _name: &str) -> Result<Option<ResolvedTarget>> {
            if let Some(failure) = &self.lookup_failure {
                anyhow::bail!("{failure}");
            }
            Ok(self.online.clone())
        }

        fn ignores(&self, _owner_guid: u64, _other_guid: u64) -> Result<bool> {
            *self.ignore_reads.lock().unwrap() += 1;
            if let Some(failure) = &self.ignore_failure {
                anyhow::bail!("{failure}");
            }
            Ok(self.ignored)
        }
    }

    const ACTOR: u64 = 42;

    fn orc() -> SpeakerFacts {
        SpeakerFacts {
            race: 2,
            chat_tag: 0,
            name: "Grunt".to_string(),
        }
    }

    fn store(outcome: Option<Result<ChannelOutcome, String>>) -> InMemoryChannelActions {
        InMemoryChannelActions {
            facts: Some(orc()),
            outcome,
            ..Default::default()
        }
    }

    fn player() -> ChatActionPlayer {
        ChatActionPlayer {
            account_id: 7,
            self_guid: Some(ACTOR),
        }
    }

    fn join(name: &str, password: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_JOIN_CHANNEL(Box::new(CMSG_JOIN_CHANNEL {
            channel_name: name.to_string(),
            channel_password: password.to_string(),
        }))
    }

    fn leave(name: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_LEAVE_CHANNEL(Box::new(CMSG_LEAVE_CHANNEL {
            channel_name: name.to_string(),
        }))
    }

    fn list(name: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_LIST(Box::new(CMSG_CHANNEL_LIST {
            channel_name: name.to_string(),
        }))
    }

    fn owner(name: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_OWNER(Box::new(CMSG_CHANNEL_OWNER {
            channel_name: name.to_string(),
        }))
    }

    fn set_owner(name: &str, target: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_SET_OWNER(Box::new(CMSG_CHANNEL_SET_OWNER {
            channel_name: name.to_string(),
            new_owner: target.to_string(),
        }))
    }

    fn moderator(name: &str, target: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_MODERATOR(Box::new(CMSG_CHANNEL_MODERATOR {
            channel_name: name.to_string(),
            player_name: target.to_string(),
        }))
    }

    fn unmoderator(name: &str, target: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_UNMODERATOR(Box::new(CMSG_CHANNEL_UNMODERATOR {
            channel_name: name.to_string(),
            player_name: target.to_string(),
        }))
    }

    fn mute(name: &str, target: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_MUTE(Box::new(CMSG_CHANNEL_MUTE {
            channel_name: name.to_string(),
            player_name: target.to_string(),
        }))
    }

    fn unmute(name: &str, target: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_UNMUTE(Box::new(CMSG_CHANNEL_UNMUTE {
            channel_name: name.to_string(),
            player_name: target.to_string(),
        }))
    }

    fn kick(name: &str, target: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_KICK(Box::new(CMSG_CHANNEL_KICK {
            channel_name: name.to_string(),
            player_name: target.to_string(),
        }))
    }

    fn ban(name: &str, target: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_BAN(Box::new(CMSG_CHANNEL_BAN {
            channel_name: name.to_string(),
            player_name: target.to_string(),
        }))
    }

    fn unban(name: &str, target: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_UNBAN(Box::new(CMSG_CHANNEL_UNBAN {
            channel_name: name.to_string(),
            player_name: target.to_string(),
        }))
    }

    fn invite(name: &str, target: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_INVITE(Box::new(CMSG_CHANNEL_INVITE {
            channel_name: name.to_string(),
            player_name: target.to_string(),
        }))
    }

    fn announcements(name: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_ANNOUNCEMENTS(Box::new(CMSG_CHANNEL_ANNOUNCEMENTS {
            channel_name: name.to_string(),
        }))
    }

    fn moderate(name: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_MODERATE(Box::new(CMSG_CHANNEL_MODERATE {
            channel_name: name.to_string(),
        }))
    }

    fn handled(outcome: ChannelActionOutcome) -> Vec<Outbound> {
        match outcome {
            ChannelActionOutcome::Handled { outbound } => outbound,
            ChannelActionOutcome::PassThrough(msg) => panic!("expected Handled, got {msg}"),
        }
    }

    /// The single raw packet `outbound` holds, as `(opcode, body)`.
    fn only_raw(outbound: Vec<Outbound>) -> (u16, Vec<u8>) {
        let mut outbound = outbound.into_iter();
        match (outbound.next(), outbound.next()) {
            (Some(Outbound::Raw { opcode, body }), None) => (opcode, body),
            _ => panic!("expected exactly one raw packet"),
        }
    }

    fn roster(members: &[(u64, u8)], owner_guid: u64, owner_name: &str) -> ChannelRoster {
        ChannelRoster {
            name: "Raiders".to_string(),
            flags: 0x01,
            owner_guid,
            owner_name: owner_name.to_string(),
            members: members.to_vec(),
        }
    }

    #[test]
    fn a_join_conveys_the_typed_name_the_password_and_the_speaker() {
        let store = store(None);
        let outbound =
            handled(dispatch_channel_action(&store, player(), join("Raiders", "hunter2")).unwrap());
        assert!(outbound.is_empty(), "YOU_JOINED returns on the Relay");
        assert_eq!(
            store.ops.lock().unwrap().as_slice(),
            &[(
                ACTOR,
                0,
                ChannelRequest {
                    channel_name: "Raiders".to_string(),
                    password: "hunter2".to_string(),
                    target_guid: 0,
                    target_name: String::new(),
                    target_race: 0,
                    target_ignores_actor: false,
                    speaker: orc(),
                }
            )]
        );
    }

    #[test]
    fn a_leave_and_a_password_change_run_their_ops() {
        let store = store(None);
        handled(dispatch_channel_action(&store, player(), leave("Raiders")).unwrap());
        let password =
            ClientOpcodeMessage::CMSG_CHANNEL_PASSWORD(Box::new(CMSG_CHANNEL_PASSWORD {
                channel_name: "Raiders".to_string(),
                channel_password: "sesame".to_string(),
            }));
        handled(dispatch_channel_action(&store, player(), password).unwrap());
        let ops = store.ops.lock().unwrap();
        assert_eq!(ops.len(), 2);
        assert_eq!((ops[0].1, ops[0].2.channel_name.as_str()), (1, "Raiders"));
        assert_eq!(
            (ops[1].1, ops[1].2.password.as_str()),
            (2, "sesame"),
            "PASSWORD carries the new password"
        );
    }

    #[test]
    fn an_empty_name_does_nothing() {
        let store = store(None);
        for msg in [join("", ""), leave("")] {
            let outbound = handled(dispatch_channel_action(&store, player(), msg).unwrap());
            assert!(outbound.is_empty());
        }
        assert!(store.ops.lock().unwrap().is_empty());
    }

    /// NOT_MEMBER 0x05 with the typed name and no payload.
    #[test]
    fn leaving_a_channel_you_are_not_on_answers_not_member_with_the_typed_name() {
        let store = store(Some(Ok(ChannelOutcome::Refused(ChannelRefusal::NotMember))));
        let outbound =
            handled(dispatch_channel_action(&store, player(), leave("rAiders")).unwrap());
        assert_eq!(
            only_raw(outbound),
            (
                0x0099,
                vec![0x05, b'r', b'A', b'i', b'd', b'e', b'r', b's', 0]
            )
        );
    }

    #[test]
    fn each_join_refusal_answers_its_notice() {
        for (refusal, code) in [
            (ChannelRefusal::InvalidName, 0x1B),
            (ChannelRefusal::WrongPassword, 0x04),
            (ChannelRefusal::Banned, 0x13),
        ] {
            let store = store(Some(Ok(ChannelOutcome::Refused(refusal))));
            let outbound =
                handled(dispatch_channel_action(&store, player(), join("1x", "")).unwrap());
            assert_eq!(
                only_raw(outbound),
                (0x0099, vec![code, b'1', b'x', 0]),
                "{refusal:?}"
            );
        }
    }

    /// PLAYER_ALREADY_MEMBER 0x17 names the joiner (cm:Channel.cpp:64-72).
    #[test]
    fn a_repeat_join_of_a_custom_channel_names_the_joiner() {
        let store = store(Some(Ok(ChannelOutcome::Refused(
            ChannelRefusal::PlayerAlreadyMember,
        ))));
        let outbound = handled(dispatch_channel_action(&store, player(), join("Rx", "")).unwrap());
        assert_eq!(
            only_raw(outbound),
            (0x0099, vec![0x17, b'R', b'x', 0, 42, 0, 0, 0, 0, 0, 0, 0])
        );
    }

    #[test]
    fn a_non_moderator_setting_the_password_answers_not_moderator() {
        let store = store(Some(Ok(ChannelOutcome::Refused(
            ChannelRefusal::NotModerator,
        ))));
        let password =
            ClientOpcodeMessage::CMSG_CHANNEL_PASSWORD(Box::new(CMSG_CHANNEL_PASSWORD {
                channel_name: "Rx".to_string(),
                channel_password: "x".to_string(),
            }));
        let outbound = handled(dispatch_channel_action(&store, player(), password).unwrap());
        assert_eq!(only_raw(outbound), (0x0099, vec![0x06, b'R', b'x', 0]));
    }

    #[test]
    fn a_lost_reducer_transport_ends_the_session() {
        let store = store(Some(Err(
            "realm_channel_op reducer transport disconnected: channel closed".to_string(),
        )));
        let error = dispatch_channel_action(&store, player(), join("Rx", ""))
            .err()
            .expect("transport loss is fatal");
        assert!(error.to_string().contains("transport disconnected"));
    }

    #[test]
    fn any_other_failure_drops_the_op_and_keeps_the_session() {
        let store = store(Some(Err(
            "realm_channel_op reducer timed out after 10s".to_string()
        )));
        let outbound = handled(dispatch_channel_action(&store, player(), join("Rx", "")).unwrap());
        assert!(outbound.is_empty());
    }

    #[test]
    fn an_actor_without_a_live_entity_sends_nothing() {
        let store = InMemoryChannelActions::default();
        let outbound = handled(dispatch_channel_action(&store, player(), join("Rx", "")).unwrap());
        assert!(outbound.is_empty());
        assert!(store.ops.lock().unwrap().is_empty());
    }

    /// cm:Channel.cpp:490-515: the stored name and flags, then each member with raw flag bits.
    /// An Orc (race 2) reads the Horde's channels.
    #[test]
    fn the_member_list_carries_every_member_with_raw_flag_bits() {
        let store = InMemoryChannelActions {
            roster: Some(roster(&[(9, 0x03), (ACTOR, 0x08)], 9, "Thrall")),
            ..store(None)
        };
        let outbound = handled(dispatch_channel_action(&store, player(), list("raiders")).unwrap());
        assert_eq!(
            store.roster_reads.lock().unwrap().as_slice(),
            &[(67, "raiders".to_string())]
        );
        let mut outbound = outbound.into_iter();
        let Some(Outbound::One(ServerOpcodeMessage::SMSG_CHANNEL_LIST(list))) = outbound.next()
        else {
            panic!("expected SMSG_CHANNEL_LIST");
        };
        assert!(outbound.next().is_none());
        assert_eq!(list.channel_name, "Raiders");
        assert_eq!(list.channel_flags.as_int(), 0x01);
        let members: Vec<(u64, u8)> = list
            .members
            .iter()
            .map(|member| (member.guid.guid(), member.member_flags.as_int()))
            .collect();
        assert_eq!(members, [(9, 0x03), (ACTOR, 0x08)]);
    }

    #[test]
    fn a_non_member_asking_for_the_list_or_the_owner_gets_not_member() {
        for msg in [list("Raiders"), owner("Raiders")] {
            let store = InMemoryChannelActions {
                roster: Some(roster(&[(9, 0x03)], 9, "Thrall")),
                ..store(None)
            };
            let outbound = handled(dispatch_channel_action(&store, player(), msg).unwrap());
            assert_eq!(
                only_raw(outbound),
                (0x0099, [&[0x05][..], b"Raiders\0"].concat())
            );
        }
        let store = store(None);
        let outbound = handled(dispatch_channel_action(&store, player(), list("Nowhere")).unwrap());
        assert_eq!(
            only_raw(outbound),
            (0x0099, [&[0x05][..], b"Nowhere\0"].concat()),
            "no channel at all"
        );
    }

    #[test]
    fn the_owner_query_names_the_owner() {
        let store = InMemoryChannelActions {
            roster: Some(roster(&[(9, 0x03), (ACTOR, 0)], 9, "Thrall")),
            ..store(None)
        };
        let outbound =
            handled(dispatch_channel_action(&store, player(), owner("Raiders")).unwrap());
        assert_eq!(
            only_raw(outbound),
            (0x0099, [&[0x0B][..], b"Raiders\0Thrall\0"].concat())
        );
    }

    /// vm:Channel.cpp:881-890.
    #[test]
    fn a_channel_without_an_owner_names_nobody() {
        let store = InMemoryChannelActions {
            roster: Some(roster(&[(ACTOR, 0)], 0, "")),
            ..store(None)
        };
        let outbound =
            handled(dispatch_channel_action(&store, player(), owner("Trade - City")).unwrap());
        assert_eq!(
            only_raw(outbound),
            (0x0099, [&[0x0B][..], b"Trade - City\0Nobody\0"].concat())
        );
    }

    #[test]
    fn a_failed_read_answers_nothing() {
        let store = InMemoryChannelActions {
            roster_failure: true,
            ..store(None)
        };
        let outbound = handled(dispatch_channel_action(&store, player(), list("Raiders")).unwrap());
        assert!(outbound.is_empty());
    }

    #[test]
    fn another_opcode_passes_through() {
        let store = store(None);
        let outcome = dispatch_channel_action(
            &store,
            player(),
            ClientOpcodeMessage::CMSG_PING(CMSG_PING::default()),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            ChannelActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_PING(_))
        ));
    }

    const TARGET: u64 = 9;
    const TARGET_RACE: u8 = 6; // Tauren

    fn resolved(ignored: bool) -> InMemoryChannelActions {
        InMemoryChannelActions {
            online: Some(ResolvedTarget {
                guid: TARGET,
                race: TARGET_RACE,
                name: "Thrall".to_string(),
            }),
            ignored,
            ..store(None)
        }
    }

    /// Every op that names a Character resolves it through `online_character_by_name` and conveys the
    /// resolved guid, race and canonical name. Only INVITE also reads `ignores`
    /// (cm:Channel.cpp:192-199 and the other target-lookup sites).
    #[test]
    fn every_targeted_op_resolves_the_target_and_conveys_its_op_code() {
        let cases: [(ClientOpcodeMessage, u8); 9] = [
            (set_owner("Raiders", "Thrall"), channel_op::SET_OWNER),
            (moderator("Raiders", "Thrall"), channel_op::MODERATOR),
            (unmoderator("Raiders", "Thrall"), channel_op::UNMODERATOR),
            (mute("Raiders", "Thrall"), channel_op::MUTE),
            (unmute("Raiders", "Thrall"), channel_op::UNMUTE),
            (kick("Raiders", "Thrall"), channel_op::KICK),
            (ban("Raiders", "Thrall"), channel_op::BAN),
            (unban("Raiders", "Thrall"), channel_op::UNBAN),
            (invite("Raiders", "Thrall"), channel_op::INVITE),
        ];
        for (msg, op) in cases {
            let store = resolved(true);
            let outbound = handled(dispatch_channel_action(&store, player(), msg).unwrap());
            assert!(outbound.is_empty(), "op {op}");
            let ops = store.ops.lock().unwrap();
            assert_eq!(ops.len(), 1, "op {op}");
            let (actor, got_op, request) = &ops[0];
            assert_eq!(*actor, ACTOR, "op {op}");
            assert_eq!(*got_op, op, "op {op}");
            assert_eq!(request.channel_name, "Raiders", "op {op}");
            assert_eq!(request.target_guid, TARGET, "op {op}");
            assert_eq!(request.target_name, "Thrall", "op {op}");
            assert_eq!(request.target_race, TARGET_RACE, "op {op}");
            assert_eq!(request.speaker, orc(), "op {op}");
            assert_eq!(
                request.target_ignores_actor,
                op == channel_op::INVITE,
                "only INVITE reads the target's ignore list: op {op}"
            );
        }
    }

    /// ANNOUNCEMENTS and MODERATE name no Character, so they run through the same untargeted path as
    /// JOIN, LEAVE and PASSWORD.
    #[test]
    fn announcements_and_moderate_carry_no_target() {
        let store = store(None);
        handled(dispatch_channel_action(&store, player(), announcements("Raiders")).unwrap());
        handled(dispatch_channel_action(&store, player(), moderate("Raiders")).unwrap());
        let ops = store.ops.lock().unwrap();
        assert_eq!(
            ops.iter()
                .map(|(_, op, request)| (*op, request.target_guid))
                .collect::<Vec<_>>(),
            [(channel_op::ANNOUNCEMENTS, 0), (channel_op::MODERATE, 0)]
        );
    }

    /// An unresolved or offline target still reaches the Module, as `target_guid` 0 with the
    /// typed name: cmangos checks membership and rights before the target name, so the Module
    /// runs those Gates first and PLAYER_NOT_FOUND is its own answer, not a Gateway shortcut.
    #[test]
    fn an_unresolved_target_reaches_the_module_as_guid_zero_with_the_typed_name() {
        let store = InMemoryChannelActions {
            outcome: Some(Ok(ChannelOutcome::Refused(ChannelRefusal::PlayerNotFound))),
            ..store(None) // `online` defaults to `None`.
        };
        let outbound =
            handled(dispatch_channel_action(&store, player(), kick("Raiders", "Ghost")).unwrap());
        assert_eq!(
            only_raw(outbound),
            (0x0099, [&[0x09][..], b"Raiders\0Ghost\0"].concat())
        );
        let ops = store.ops.lock().unwrap();
        assert_eq!(ops.len(), 1, "an unresolved name still reaches the Module");
        assert_eq!(ops[0].2.target_guid, 0);
        assert_eq!(ops[0].2.target_name, "Ghost");
    }

    /// A target lookup that fails without losing transport (a peer Shard that cannot currently
    /// vouch for absence) degrades the same way as an unresolved name: `target_guid` 0 reaches
    /// the Module, and the World Session stays up. Only a transport loss may end the session.
    #[test]
    fn a_failed_target_lookup_reaches_the_module_as_guid_zero_and_keeps_the_session() {
        let store = InMemoryChannelActions {
            lookup_failure: Some("realm-core database lyracore-realm is not connected".to_string()),
            outcome: Some(Ok(ChannelOutcome::Refused(ChannelRefusal::PlayerNotFound))),
            ..store(None)
        };
        let outbound =
            handled(dispatch_channel_action(&store, player(), kick("Raiders", "Ghost")).unwrap());
        assert_eq!(
            only_raw(outbound),
            (0x0099, [&[0x09][..], b"Raiders\0Ghost\0"].concat())
        );
        let ops = store.ops.lock().unwrap();
        assert_eq!(
            ops.len(),
            1,
            "a recoverable lookup failure still reaches the Module"
        );
        assert_eq!(ops[0].2.target_guid, 0);
    }

    /// An unresolved INVITE target carries `target_ignores_actor` false and never reads the
    /// ignore list at all: race 0 would otherwise pass the Alliance branch of the team check and
    /// the invite would wrongly go through instead of refusing PLAYER_NOT_FOUND.
    #[test]
    fn an_unresolved_invite_target_skips_the_ignore_read() {
        let store = InMemoryChannelActions {
            outcome: Some(Ok(ChannelOutcome::Refused(ChannelRefusal::PlayerNotFound))),
            ..store(None)
        };
        handled(dispatch_channel_action(&store, player(), invite("Raiders", "Ghost")).unwrap());
        let ops = store.ops.lock().unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].2.target_guid, 0);
        assert!(!ops[0].2.target_ignores_actor);
        assert_eq!(
            *store.ignore_reads.lock().unwrap(),
            0,
            "no target to check ignores for"
        );
    }

    /// A failed ignore-list read on INVITE degrades the same way: PLAYER_NOT_FOUND, no Durable
    /// Request, session kept alive.
    #[test]
    fn a_failed_ignore_read_answers_player_not_found_and_keeps_the_session() {
        let store = InMemoryChannelActions {
            ignore_failure: Some("realm-core database lyracore-realm is not connected".to_string()),
            ..resolved(false)
        };
        let outbound = handled(
            dispatch_channel_action(&store, player(), invite("Raiders", "Thrall")).unwrap(),
        );
        assert_eq!(
            only_raw(outbound),
            (0x0099, [&[0x09][..], b"Raiders\0Thrall\0"].concat())
        );
        assert!(store.ops.lock().unwrap().is_empty());
    }

    /// A transport-lost target lookup still ends the World Session; only the answer-and-continue
    /// behavior above is new.
    #[test]
    fn a_lost_transport_on_target_lookup_ends_the_session() {
        let store = InMemoryChannelActions {
            lookup_failure: Some(
                "realm_channel_op reducer transport disconnected: channel closed".to_string(),
            ),
            ..store(None)
        };
        let error = dispatch_channel_action(&store, player(), kick("Raiders", "Ghost"))
            .err()
            .expect("transport loss is fatal");
        assert!(error.to_string().contains("transport disconnected"));
    }

    /// PLAYER_ALREADY_MEMBER 0x17 names the resolved target, not the actor.
    #[test]
    fn player_already_member_names_the_resolved_target() {
        let store = InMemoryChannelActions {
            outcome: Some(Ok(ChannelOutcome::Refused(
                ChannelRefusal::PlayerAlreadyMember,
            ))),
            ..resolved(false)
        };
        let outbound = handled(
            dispatch_channel_action(&store, player(), invite("Raiders", "Thrall")).unwrap(),
        );
        assert_eq!(
            only_raw(outbound),
            (
                0x0099,
                [&[0x17][..], b"Raiders\0", &TARGET.to_le_bytes()].concat()
            )
        );
    }

    /// PLAYER_NOT_BANNED 0x16 carries the resolved target's canonical name.
    #[test]
    fn player_not_banned_names_the_resolved_target() {
        let store = InMemoryChannelActions {
            outcome: Some(Ok(ChannelOutcome::Refused(ChannelRefusal::PlayerNotBanned))),
            ..resolved(false)
        };
        let outbound =
            handled(dispatch_channel_action(&store, player(), unban("Raiders", "thrall")).unwrap());
        assert_eq!(
            only_raw(outbound),
            (0x0099, [&[0x16][..], b"Raiders\0", b"Thrall\0"].concat())
        );
    }
}
