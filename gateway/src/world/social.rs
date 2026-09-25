//! Social-family dispatch: the /who panel, the friends/ignore lists, and party/group management,
//! carved out of `handle_query` in `world/mod.rs` — pure code-motion, same per-family `handle_*`
//! shape as the rest of the dispatch chain.

use super::party::PartyOutcome;
use super::{party, presence, send, who, Outbound, SessionTx, WorldConn, WorldState, WorldStore};
use crate::codec;
use anyhow::Result;
use lyracore_shared::group::{GroupRefusal, TARGET_ICON_LIST_REQUEST};
use lyracore_shared::social::ContactRefusal;
use wow_world_base::shared::friend_result_vanilla_tbc::FriendResult;
use wow_world_messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage};
use wow_world_messages::vanilla::{MSG_RAID_TARGET_UPDATE_Client, PartyOperation, PartyResult};
use wow_world_messages::Guid;

/// What one contact-list op answered. A [`ContactRefusal`] is a gameplay answer `SMSG_FRIEND_STATUS`
/// renders; a timeout, transport failure, or untagged reducer error stays `Err` and ends the
/// session, because the durable outcome is then unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContactOutcome {
    Done,
    Refused(ContactRefusal),
}

impl From<ContactRefusal> for ContactOutcome {
    fn from(refusal: ContactRefusal) -> Self {
        Self::Refused(refusal)
    }
}

/// Social family: /who, the friends/ignore lists, and party/group management — the social-pane
/// opcodes. Each arm consumes its opcode (`Ok(None)`) or passes the message on (`Ok(Some(msg))`),
/// like the other per-family handlers.
#[allow(clippy::too_many_lines)] // One arm per social and party opcode.
pub(super) fn handle_social<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
        // /who panel: filtered realm-wide (`who::respond`), same team only, every filter the
        // client sent applied. Silently dropped outside the world (no requester Character to read
        // a team from), for an unresolvable requester (never guess a team), and while this
        // session's `WHO_THROTTLE` cooldown is running (vm:MiscHandler.cpp:230).
        ClientOpcodeMessage::CMSG_WHO(request) => {
            if let Some(me) = self_guid(conn) {
                if conn.admit_who() {
                    if let Some(character) = store.character_by_guid(me)? {
                        if let Some((opcode, body)) = who::respond(store, character.race, &request)?
                        {
                            send(tx, Outbound::Raw { opcode, body })?;
                        }
                    }
                }
            }
        }
        // Friends / ignore list: opening the social pane's friends tab requests BOTH
        // lists off the one opcode — vanilla answers with SMSG_FRIEND_LIST + SMSG_IGNORE_LIST.
        // Silently dropped outside the world (no character to scope the lists to).
        ClientOpcodeMessage::CMSG_FRIEND_LIST => {
            if let WorldState::InWorld(iw) = &conn.state {
                let (friend_guids, ignored) = store.contact_lists(iw.self_guid)?;
                let own_team = store
                    .character_by_guid(iw.self_guid)?
                    .map_or(lyracore_shared::faction::TEAM_ALLIANCE, |character| {
                        lyracore_shared::faction::team_for_race(character.race)
                    });
                let friends = friend_views(store, own_team, &friend_guids);
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_FRIEND_LIST(Box::new(
                        codec::build_friend_list_response(&friends),
                    ))),
                )?;
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_IGNORE_LIST(Box::new(
                        codec::build_ignore_list_response(&ignored),
                    ))),
                )?;
            }
        }
        // Add a friend/ignore by typed name: resolved realm-wide the same way `/whisper`'s target
        // is, then the module re-validates self/duplicate/cap/team server-side. Either way the
        // client gets an SMSG_FRIEND_STATUS its system message reads the result code off.
        ClientOpcodeMessage::CMSG_ADD_FRIEND(c) => {
            let (result, guid, online) = resolve_add_contact(
                store,
                conn.account_id,
                self_guid(conn).unwrap_or(0),
                &c.name,
                false,
            )?;
            let (opcode, body) = codec::build_friend_status_raw(result, guid, online);
            send(tx, Outbound::Raw { opcode, body })?;
        }
        ClientOpcodeMessage::CMSG_ADD_IGNORE(c) => {
            let (result, guid, online) = resolve_add_contact(
                store,
                conn.account_id,
                self_guid(conn).unwrap_or(0),
                &c.name,
                true,
            )?;
            let (opcode, body) = codec::build_friend_status_raw(result, guid, online);
            send(tx, Outbound::Raw { opcode, body })?;
        }
        // Remove a friend/ignore by guid (the client already has it from the list row).
        ClientOpcodeMessage::CMSG_DEL_FRIEND(c) => {
            let (result, guid) = resolve_del_contact(
                store,
                conn.account_id,
                self_guid(conn).unwrap_or(0),
                c.guid.guid(),
                false,
            )?;
            let (opcode, body) = codec::build_friend_status_raw(result, guid, None);
            send(tx, Outbound::Raw { opcode, body })?;
        }
        ClientOpcodeMessage::CMSG_DEL_IGNORE(c) => {
            let (result, guid) = resolve_del_contact(
                store,
                conn.account_id,
                self_guid(conn).unwrap_or(0),
                c.guid.guid(),
                true,
            )?;
            let (opcode, body) = codec::build_friend_status_raw(result, guid, None);
            send(tx, Outbound::Raw { opcode, body })?;
        }
        // Party/group. The invite/uninvite names resolve gateway-side (the add_friend
        // convention); outcomes echo as SMSG_PARTY_COMMAND_RESULT. The cross-player packets
        // (SMSG_GROUP_INVITE/LIST/DECLINE/DESTROYED) ride the game_group_event relay.
        //
        // The op itself goes through `world::party`, which decides WHICH DATABASE
        // runs it — realm-core when the gateway is multi-database, the player's own shard otherwise.
        // Every arm below is otherwise unchanged, including which failures the client is told about.
        // Silently dropped outside the world: with no in-world character there is no `self_guid` to
        // act as, and none of these opcodes is reachable at character select.
        ClientOpcodeMessage::CMSG_GROUP_INVITE(c) => {
            let result = match (self_guid(conn), presence::resolve_by_name(store, &c.name)?) {
                (Some(me), Some(guid)) => party_result(party::run(
                    store,
                    conn.account_id,
                    me,
                    party::Op::Invite(guid),
                )?),
                _ => PartyResult::BadPlayerName,
            };
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(Box::new(
                    codec::build_party_command_result(PartyOperation::Invite, c.name, result),
                ))),
            )?;
        }
        ClientOpcodeMessage::CMSG_GROUP_ACCEPT => run_unanswered(store, conn, party::Op::Accept)?,
        ClientOpcodeMessage::CMSG_GROUP_DECLINE => run_unanswered(store, conn, party::Op::Decline)?,
        ClientOpcodeMessage::CMSG_GROUP_DISBAND => {
            run_answering_refusal(tx, store, conn, party::Op::Leave)?
        }
        ClientOpcodeMessage::CMSG_GROUP_UNINVITE(c) => {
            let result = match (self_guid(conn), presence::resolve_by_name(store, &c.name)?) {
                (Some(me), Some(guid)) => party_result(party::run(
                    store,
                    conn.account_id,
                    me,
                    party::Op::Uninvite(guid),
                )?),
                _ => PartyResult::BadPlayerName,
            };
            if result != PartyResult::Success {
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(Box::new(
                        codec::build_party_command_result(PartyOperation::Leave, c.name, result),
                    ))),
                )?;
            }
        }
        // `CMSG_LOOT_METHOD`: the leader sets the party's loot method/
        // threshold/master. No ack packet — vanilla itself sends none for this opcode (cmangos's
        // `HandleLootMethodOpcode` only calls `group->SendUpdate()`); the module's own reducer
        // echoes via the EXISTING `SMSG_GROUP_LIST` roster relay. A rejection (not the leader, bad
        // method/threshold/master) is per-action — log + ignore, matching group_accept/decline.
        ClientOpcodeMessage::CMSG_LOOT_METHOD(c) => {
            let op = party::Op::LootMethod {
                setting: c.loot_setting.as_int(),
                master: c.loot_master.guid(),
                threshold: c.loot_threshold.as_int(),
            };
            run_unanswered(store, conn, op)?;
        }
        ClientOpcodeMessage::CMSG_GROUP_RAID_CONVERT => raid_convert(tx, store, conn)?,
        ClientOpcodeMessage::CMSG_GROUP_SET_LEADER(c) => {
            run_unanswered(store, conn, party::Op::SetLeader(c.guid.guid()))?
        }
        ClientOpcodeMessage::CMSG_GROUP_ASSISTANT_LEADER(c) => run_unanswered(
            store,
            conn,
            party::Op::SetAssistant {
                target: c.guid.guid(),
                promote: c.set_assistant,
            },
        )?,
        // The raid frame's "Remove from group" names the member by guid, so no name lookup runs
        // (cm:GroupHandler.cpp:250-296). Naming yourself gets no answer (lines 255-260).
        ClientOpcodeMessage::CMSG_GROUP_UNINVITE_GUID(c)
            if self_guid(conn) != Some(c.guid.guid()) =>
        {
            run_answering_refusal(tx, store, conn, party::Op::Uninvite(c.guid.guid()))?
        }
        ClientOpcodeMessage::CMSG_GROUP_UNINVITE_GUID(_) => {}
        // `CMSG_GROUP_CHANGE_SUB_GROUP` / `CMSG_GROUP_SWAP_SUB_GROUP`: the leader or an Assistant
        // drags a raid member to another Subgroup, or swaps two. No ack packet: the
        // `SMSG_GROUP_LIST` roster relay is the client-visible result, and cmangos is silent for
        // every refusal here (cm:GroupHandler.cpp:492-525, cm:GroupHandler.cpp:901-944).
        ClientOpcodeMessage::CMSG_GROUP_CHANGE_SUB_GROUP(c) => {
            change_subgroup(store, conn, &c.name, c.group_number)?
        }
        ClientOpcodeMessage::CMSG_GROUP_SWAP_SUB_GROUP(c) => {
            swap_subgroup(store, conn, &c.name, &c.swap_with_name)?
        }
        // Group Broadcasts. Every member's packet rides the group event relay, the actor's too.
        ClientOpcodeMessage::MSG_RAID_READY_CHECK(c) => {
            let op = match c.answer {
                None => party::Op::ReadyCheckStart,
                Some(answer) => party::Op::ReadyCheckAnswer(answer.state),
            };
            run_group_broadcast(store, conn, op);
        }
        ClientOpcodeMessage::MSG_RAID_TARGET_UPDATE(c) => {
            run_group_broadcast(store, conn, target_icon_op(&c));
        }
        ClientOpcodeMessage::MSG_MINIMAP_PING(c) => {
            let op = party::Op::MinimapPing {
                x: c.position_x,
                y: c.position_y,
            };
            run_group_broadcast(store, conn, op);
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}

/// Run a leave or a kick by guid. Only a Refusal answers, as `SMSG_PARTY_COMMAND_RESULT(Leave, "",
/// result)`; an op that ran is heard through the relay.
fn run_answering_refusal<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &WorldConn,
    op: party::Op,
) -> Result<()> {
    let Some(me) = self_guid(conn) else {
        return Ok(());
    };
    match party::run(store, conn.account_id, me, op)? {
        PartyOutcome::Ran => Ok(()),
        PartyOutcome::Refused(refusal) => send(
            tx,
            Outbound::One(ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(Box::new(
                codec::build_party_command_result(
                    PartyOperation::Leave,
                    String::new(),
                    party_result_for(refusal),
                ),
            ))),
        ),
    }
}

/// The leader's "Convert to Raid". cmangos answers success with
/// `SMSG_PARTY_COMMAND_RESULT(Invite, "", Ok)` and every refusal with silence
/// (cm:GroupHandler.cpp:473-490); the raid list reaches every member through the LIST relay.
fn raid_convert<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &WorldConn,
) -> Result<()> {
    let Some(me) = self_guid(conn) else {
        return Ok(());
    };
    match party::run(store, conn.account_id, me, party::Op::RaidConvert)? {
        PartyOutcome::Ran => send(
            tx,
            Outbound::One(ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(Box::new(
                codec::build_party_command_result(
                    PartyOperation::Invite,
                    String::new(),
                    PartyResult::Success,
                ),
            ))),
        ),
        PartyOutcome::Refused(refusal) => {
            log::debug!(
                "world: group_raid_convert refused (account {}): {refusal:?}",
                conn.account_id
            );
            Ok(())
        }
    }
}

/// Run a party op whose outcome the client hears only through the relay. A Refusal is logged at
/// debug and sends nothing. cmangos sends nothing for set leader and set Assistant, whatever the
/// outcome (cm:GroupHandler.cpp:344-362, 527-545).
fn run_unanswered<St: WorldStore + ?Sized>(
    store: &St,
    conn: &WorldConn,
    op: party::Op,
) -> Result<()> {
    let Some(me) = self_guid(conn) else {
        return Ok(());
    };
    if let PartyOutcome::Refused(refusal) = party::run(store, conn.account_id, me, op)? {
        log::debug!(
            "world: {op:?} refused (account {}): {refusal:?}",
            conn.account_id
        );
    }
    Ok(())
}

/// `CMSG_GROUP_CHANGE_SUB_GROUP`: the leader or an Assistant moves `name` to `subgroup`. The name
/// resolves against the actor's OWN roster rather than realm-wide, so a namesake standing outside
/// the Raid cannot be reached — the same avoided-homonym rule cmangos applies to Swap Subgroup
/// alone (cm:GroupHandler.cpp:919-936), extended here to both opcodes.
fn change_subgroup<St: WorldStore + ?Sized>(
    store: &St,
    conn: &WorldConn,
    name: &str,
    subgroup: u8,
) -> Result<()> {
    let Some(me) = self_guid(conn) else {
        return Ok(());
    };
    match party::resolve_roster_member_by_name(store, me, name)? {
        Some(target) => run_unanswered(store, conn, party::Op::ChangeSubgroup { target, subgroup }),
        None => {
            log::debug!(
                "world: group_change_sub_group named a Character outside the caller's own \
                 roster (account {})",
                conn.account_id
            );
            Ok(())
        }
    }
}

/// `CMSG_GROUP_SWAP_SUB_GROUP`: the leader or an Assistant swaps `name` and `swap_with_name`. Same
/// roster-only name resolution as [`change_subgroup`] (cm:GroupHandler.cpp:901-944,
/// cm:GroupHandler.cpp:919-936), with both names resolved against the SAME roster read so the
/// pair cannot straddle two different snapshots of it.
fn swap_subgroup<St: WorldStore + ?Sized>(
    store: &St,
    conn: &WorldConn,
    name: &str,
    swap_with_name: &str,
) -> Result<()> {
    let Some(me) = self_guid(conn) else {
        return Ok(());
    };
    let (first, second) = party::resolve_roster_members_by_name(store, me, name, swap_with_name)?;
    match (first, second) {
        (Some(first), Some(second)) => {
            run_unanswered(store, conn, party::Op::SwapSubgroup { first, second })
        }
        _ => {
            log::debug!(
                "world: group_swap_sub_group named a Character outside the caller's own roster \
                 (account {})",
                conn.account_id
            );
            Ok(())
        }
    }
}

/// Run one Group Broadcast for the session's Character. cmangos answers every refusal of these
/// opcodes with silence, so a Refusal only logs. A transport failure only logs too: a broadcast
/// changes no roster and nothing waits on it, so a lost ping or roll must not end the session.
/// A throttled op sent again inside its cooldown is dropped the same way.
pub(super) fn run_group_broadcast<St: WorldStore + ?Sized>(
    store: &St,
    conn: &mut WorldConn,
    op: party::Op,
) {
    let Some(me) = self_guid(conn) else {
        return;
    };
    if !conn.admit_group_broadcast(op) {
        log::debug!(
            "world: group broadcast {op:?} dropped inside its cooldown (account {})",
            conn.account_id
        );
        return;
    }
    match party::run(store, conn.account_id, me, op) {
        Ok(PartyOutcome::Ran) => {}
        Ok(PartyOutcome::Refused(refusal)) => log::debug!(
            "world: group broadcast {op:?} refused (account {}): {refusal:?}",
            conn.account_id
        ),
        Err(error) => log::warn!(
            "world: group broadcast {op:?} lost (account {}): {error:#}",
            conn.account_id
        ),
    }
}

/// `MSG_RAID_TARGET_UPDATE` as a Target Icon op. `RequestIcons` is the list request, and
/// `Unknown0` to `Unknown8` are indexes 0 to 8. The Module refuses index 8.
fn target_icon_op(update: &MSG_RAID_TARGET_UPDATE_Client) -> party::Op {
    use MSG_RAID_TARGET_UPDATE_Client as Update;
    let (icon, target) = match *update {
        Update::RequestIcons => (TARGET_ICON_LIST_REQUEST, Guid::zero()),
        Update::Unknown0 { target } => (0, target),
        Update::Unknown1 { target } => (1, target),
        Update::Unknown2 { target } => (2, target),
        Update::Unknown3 { target } => (3, target),
        Update::Unknown4 { target } => (4, target),
        Update::Unknown5 { target } => (5, target),
        Update::Unknown6 { target } => (6, target),
        Update::Unknown7 { target } => (7, target),
        Update::Unknown8 { target } => (8, target),
    };
    party::Op::TargetIcon {
        icon,
        target: target.guid(),
    }
}

/// The session's in-world character guid, or `None` at character select. Party ops need it for two
/// reasons that only coincide on a single-database gateway: it is the CHARACTER realm-core acts as
/// (realm-core has no live entity to derive one from), and it is the character the module's
/// own `entity_by_owner` would have resolved on the shard plane. Reading it here, from the state the
/// gateway already authenticated for this socket, is what keeps the realm-core call trustworthy.
///
/// `pub(super)` because the WHISPER slice needs the same guid for the same reason: `realm_whisper`
/// takes the sender as an argument, so this accessor is the authorization of every realm-plane whisper
/// (see `world::whisper::run`). One accessor, so there is one place a mutation can substitute a
/// literal — and one place the end-to-end tests pin.
pub(super) fn self_guid(conn: &WorldConn) -> Option<u64> {
    match &conn.state {
        WorldState::InWorld(iw) => Some(iw.self_guid),
        _ => None,
    }
}

/// The `PartyResult` code one party outcome renders as.
fn party_result(outcome: PartyOutcome) -> PartyResult {
    match outcome {
        PartyOutcome::Ran => PartyResult::Success,
        PartyOutcome::Refused(refusal) => party_result_for(refusal),
    }
}

/// Map each [`GroupRefusal`] onto the vanilla `PartyResult` the client renders ("X is already in a
/// group" etc.). Vanilla has no code for an offline or self-named target, a stale invite, a raid
/// op in a Party, a temporarily unavailable actor, or either Subgroup move Refusal, since cmangos
/// answers a refused Change or Swap Subgroup with silence rather than a code, so all of those read
/// as BadPlayerName, a visible line that does not crash the client. The intent claim never reaches
/// a client; it is listed so a new Refusal cannot be forgotten here.
fn party_result_for(refusal: GroupRefusal) -> PartyResult {
    match refusal {
        GroupRefusal::AlreadyInGroup => PartyResult::AlreadyInGroup,
        GroupRefusal::GroupFull => PartyResult::GroupFull,
        GroupRefusal::NotLeader => PartyResult::NotLeader,
        GroupRefusal::NotInGroup => PartyResult::NotInGroup,
        GroupRefusal::TargetNotInGroup => PartyResult::TargetNotInGroup,
        GroupRefusal::WrongFaction => PartyResult::PlayerWrongFaction,
        GroupRefusal::ActorUnavailable
        | GroupRefusal::InviteSelf
        | GroupRefusal::NoSuchPlayer
        | GroupRefusal::TargetOffline
        | GroupRefusal::NoPendingInvite
        | GroupRefusal::InviterUnavailable
        | GroupRefusal::KickSelf
        | GroupRefusal::InvalidLootRules
        | GroupRefusal::IntentAlreadyClaimed
        | GroupRefusal::ActionSuppressed
        | GroupRefusal::NotRaid
        | GroupRefusal::TargetIsSelf
        | GroupRefusal::InvalidSubgroup
        | GroupRefusal::SubgroupFull
        | GroupRefusal::InvalidTargetIcon => PartyResult::BadPlayerName,
    }
}

/// The `FriendResult` code one contact Refusal renders as. The friends and ignore lists have
/// separate code families for the same three conditions, so the list decides the answer.
fn friend_result_for(refusal: ContactRefusal, is_ignore: bool) -> FriendResult {
    match (refusal, is_ignore) {
        (ContactRefusal::AddSelf, false) => FriendResult::SelfX,
        (ContactRefusal::AddSelf, true) => FriendResult::IgnoreSelf,
        (ContactRefusal::AlreadyOnList, false) => FriendResult::Already,
        (ContactRefusal::AlreadyOnList, true) => FriendResult::IgnoreAlready,
        (ContactRefusal::ListFull, false) => FriendResult::ListFull,
        (ContactRefusal::ListFull, true) => FriendResult::IgnoreFull,
        (ContactRefusal::NotOnList, true) => FriendResult::IgnoreNotFound,
        // The friend-list remove of a row that is not there, and an actor the module could not
        // reach, both read as the one "no such entry" line vanilla has.
        (ContactRefusal::NotOnList, false) | (ContactRefusal::ActorUnavailable, _) => {
            FriendResult::NotFound
        }
        // Ignore has no faction rule (only `resolve_add_contact`'s friend arm can produce this),
        // but the wire still has one code for it.
        (ContactRefusal::Enemy, _) => FriendResult::Enemy,
    }
}

/// The `SMSG_FRIEND_STATUS` trailing fields for a Realm Presence that is online — the ONLINE/
/// ADDED_ONLINE case. Away Status reads `None` off any Whereabouts but `InWorld` (a Character
/// between places carries no live `PLAYER_FLAGS` to read one from). `pub(crate)` because the
/// Account Claim Relay (`stdb::world_view`) shares it for the login/logout notice.
pub(crate) fn friend_online_fields(presence: &presence::RealmPresence) -> codec::FriendOnline {
    let away = match &presence.whereabouts {
        presence::Whereabouts::InWorld { away, .. } => *away,
        presence::Whereabouts::InTransit | presence::Whereabouts::Offline => {
            presence::AwayStatus::None
        }
    };
    codec::FriendOnline {
        away,
        zone_id: presence.zone_id,
        level: presence.level,
        class: presence.class,
    }
}

/// Hydrate friend guids into `FriendView`s for `CMSG_FRIEND_LIST`: online means `session_online`
/// and the same team as `own_team`, carrying level/class/zone/Away Status from Realm Presence
/// (cm:SocialMgr.cpp:108-139). A friend whose Realm Presence read fails — an unreachable Shard —
/// degrades that one row to offline rather than failing the whole list or ending the World
/// Session. Lives over the `WorldStore` seam, not behind any one Store's own `contact_lists`, so a
/// Store Fake exercises the exact composition a Coordinator serves.
pub(crate) fn friend_views<St: WorldStore + ?Sized>(
    store: &St,
    own_team: u32,
    friend_guids: &[u64],
) -> Vec<codec::FriendView> {
    friend_guids
        .iter()
        .map(|&guid| {
            let presence = presence::of(store, guid).unwrap_or(None);
            let online = presence.as_ref().is_some_and(|p| {
                p.session_online && lyracore_shared::faction::team_for_race(p.race) == own_team
            });
            let (away, level, class, zone_id) = match &presence {
                Some(p) if online => {
                    let away = match &p.whereabouts {
                        presence::Whereabouts::InWorld { away, .. } => *away,
                        presence::Whereabouts::InTransit | presence::Whereabouts::Offline => {
                            presence::AwayStatus::None
                        }
                    };
                    (away, p.level, p.class, p.zone_id)
                }
                _ => (presence::AwayStatus::None, 0, 0, 0),
            };
            codec::FriendView {
                guid,
                online,
                away,
                level,
                class,
                zone_id,
            }
        })
        .collect()
}

/// Resolve a typed contact name realm-wide, call the module's add reducer (`add_friend`/
/// `add_ignore`), and translate the outcome into what `SMSG_FRIEND_STATUS` needs: the result code,
/// the OTHER party's guid, and — ONLINE/ADDED_ONLINE only — the trailing presence fields. An
/// unknown name never reaches the module (guid 0, `NotFound`/`IgnoreNotFound`); everything else
/// (self/duplicate/cap/team) is the module's own typed Refusal.
fn resolve_add_contact<St: WorldStore + ?Sized>(
    store: &St,
    account_id: u64,
    actor_guid: u64,
    name: &str,
    is_ignore: bool,
) -> Result<(FriendResult, u64, Option<codec::FriendOnline>)> {
    let Some(target_guid) = presence::resolve_by_name(store, name)? else {
        let not_found = if is_ignore {
            FriendResult::IgnoreNotFound
        } else {
            FriendResult::NotFound
        };
        return Ok((not_found, 0, None));
    };
    if is_ignore {
        let outcome = store.add_ignore(account_id, actor_guid, target_guid)?;
        let result = match outcome {
            ContactOutcome::Done => FriendResult::IgnoreAdded,
            ContactOutcome::Refused(refusal) => friend_result_for(refusal, true),
        };
        return Ok((result, target_guid, None));
    }

    // Friends only: the module's Enemy Gate needs the target's race, and a successful add's
    // answer (ADDED_ONLINE vs ADDED_OFFLINE) needs its presence. An unreachable Shard must not
    // end the World Session over a friend add, so a Realm Presence read failure degrades to the
    // best-effort identity read instead — a local cache lookup that cannot itself fail this way.
    // NEVER guess a race: a wrong default (0 reads as Alliance) can refuse a real same-team
    // friend as an enemy, or wave a real enemy through.
    let target = presence::of(store, target_guid).unwrap_or_else(|error| {
        log::debug!("resolve_add_contact: Realm Presence read failed for {target_guid}: {error:#}");
        None
    });
    let target_race = match &target {
        Some(presence) => Some(presence.race),
        None => presence::character_identity_anywhere(store, target_guid)
            .unwrap_or(None)
            .map(|identity| identity.race),
    };
    let Some(target_race) = target_race else {
        // No connected Shard currently knows this Character at all, though the name resolved a
        // moment ago — treat it the same as an unresolved name rather than guess.
        return Ok((FriendResult::NotFound, 0, None));
    };
    let outcome = store.add_friend(account_id, actor_guid, target_guid, target_race)?;
    let (result, online) = match outcome {
        ContactOutcome::Done => {
            let actor_race = store
                .character_by_guid(actor_guid)?
                .map_or(0, |character| character.race);
            match target.as_ref().filter(|presence| {
                presence.session_online
                    && lyracore_shared::faction::same_team(actor_race, presence.race)
            }) {
                Some(presence) => (
                    FriendResult::AddedOnline,
                    Some(friend_online_fields(presence)),
                ),
                None => (FriendResult::AddedOffline, None),
            }
        }
        ContactOutcome::Refused(refusal) => (friend_result_for(refusal, false), None),
    };
    Ok((result, target_guid, online))
}

/// Call the module's remove reducer (`del_friend`/`del_ignore`) for `target_guid` and translate the
/// outcome into the `(FriendResult, guid)` pair `SMSG_FRIEND_STATUS` needs. A remove carries no
/// trailing presence fields, whatever the outcome.
fn resolve_del_contact<St: WorldStore + ?Sized>(
    store: &St,
    account_id: u64,
    actor_guid: u64,
    target_guid: u64,
    is_ignore: bool,
) -> Result<(FriendResult, u64)> {
    let outcome = if is_ignore {
        store.del_ignore(account_id, actor_guid, target_guid)?
    } else {
        store.del_friend(account_id, actor_guid, target_guid)?
    };
    let result = match outcome {
        ContactOutcome::Done if is_ignore => FriendResult::IgnoreRemoved,
        ContactOutcome::Done => FriendResult::Removed,
        ContactOutcome::Refused(refusal) => friend_result_for(refusal, is_ignore),
    };
    Ok((result, target_guid))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The six conditions vanilla has a code for keep their own code; the rest share the one
    /// visible fallback line rather than being swallowed.
    #[test]
    fn each_group_refusal_renders_its_own_party_result() {
        for refusal in GroupRefusal::ALL {
            let expected = match refusal {
                GroupRefusal::AlreadyInGroup => PartyResult::AlreadyInGroup,
                GroupRefusal::GroupFull => PartyResult::GroupFull,
                GroupRefusal::NotLeader => PartyResult::NotLeader,
                GroupRefusal::NotInGroup => PartyResult::NotInGroup,
                GroupRefusal::TargetNotInGroup => PartyResult::TargetNotInGroup,
                GroupRefusal::WrongFaction => PartyResult::PlayerWrongFaction,
                _ => PartyResult::BadPlayerName,
            };
            assert_eq!(party_result_for(refusal), expected, "{refusal:?}");
        }
        assert_eq!(party_result(PartyOutcome::Ran), PartyResult::Success);
    }

    /// The friends and ignore lists carry separate code families for the same conditions, so the
    /// same Refusal must not answer both lists with one code.
    #[test]
    fn each_contact_refusal_renders_the_code_of_the_list_it_names() {
        for refusal in ContactRefusal::ALL {
            let (friend, ignore) = match refusal {
                ContactRefusal::AddSelf => (FriendResult::SelfX, FriendResult::IgnoreSelf),
                ContactRefusal::AlreadyOnList => {
                    (FriendResult::Already, FriendResult::IgnoreAlready)
                }
                ContactRefusal::ListFull => (FriendResult::ListFull, FriendResult::IgnoreFull),
                ContactRefusal::NotOnList => (FriendResult::NotFound, FriendResult::IgnoreNotFound),
                ContactRefusal::ActorUnavailable => {
                    (FriendResult::NotFound, FriendResult::NotFound)
                }
                ContactRefusal::Enemy => (FriendResult::Enemy, FriendResult::Enemy),
            };
            assert_eq!(friend_result_for(refusal, false), friend, "{refusal:?}");
            assert_eq!(friend_result_for(refusal, true), ignore, "{refusal:?}");
        }
    }
}
