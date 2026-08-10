//! Social-family dispatch: the /who panel, the friends/ignore lists, and party/group management,
//! carved out of `handle_query` in `world/mod.rs` — pure code-motion, same per-family `handle_*`
//! shape as the rest of the dispatch chain.

use super::{party, send, Outbound, SessionTx, WorldConn, WorldState, WorldStore};
use crate::codec;
use anyhow::Result;
use wow_world_base::shared::friend_result_vanilla_tbc::FriendResult;
use wow_world_messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage};
use wow_world_messages::vanilla::{PartyOperation, PartyResult};

/// Social family: /who, the friends/ignore lists, and party/group management — the social-pane
/// opcodes. Each arm consumes its opcode (`Ok(None)`) or passes the message on (`Ok(Some(msg))`),
/// like the other per-family handlers.
pub(super) fn handle_social<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
        // /who panel: CMSG_WHO carries optional filters (level range, name, class/race masks, zones,
        // strings) — ignored for the first pass. We read ALL online characters and return them in
        // SMSG_WHO (capped at 49, the vanilla client's display limit). The client opens the social
        // window and lists every online player, which covers the primary grouping/social use-case.
        ClientOpcodeMessage::CMSG_WHO(_) => {
            let players = store.online_players()?;
            let resp = codec::build_who_response(&players);
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_WHO(Box::new(resp))),
            )?;
        }
        // Friends / ignore list: opening the social pane's friends tab requests BOTH
        // lists off the one opcode — vanilla answers with SMSG_FRIEND_LIST + SMSG_IGNORE_LIST.
        // Silently dropped outside the world (no character to scope the lists to).
        ClientOpcodeMessage::CMSG_FRIEND_LIST => {
            if let WorldState::InWorld(iw) = &conn.state {
                let (friends, ignored) = store.contact_lists(iw.self_guid)?;
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
        // Add a friend/ignore by typed name: resolved the same way `/whisper`'s target is (case-
        // insensitive), then the module re-validates self/duplicate/cap server-side. Either way the
        // client gets an SMSG_FRIEND_STATUS its system message reads the result code off.
        ClientOpcodeMessage::CMSG_ADD_FRIEND(c) => {
            let (result, guid) = resolve_add_contact(store, conn.account_id, self_guid(conn).unwrap_or(0), &c.name, false)?;
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_FRIEND_STATUS(Box::new(
                    codec::build_friend_status(result, guid),
                ))),
            )?;
        }
        ClientOpcodeMessage::CMSG_ADD_IGNORE(c) => {
            let (result, guid) = resolve_add_contact(store, conn.account_id, self_guid(conn).unwrap_or(0), &c.name, true)?;
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_FRIEND_STATUS(Box::new(
                    codec::build_friend_status(result, guid),
                ))),
            )?;
        }
        // Remove a friend/ignore by guid (the client already has it from the list row).
        ClientOpcodeMessage::CMSG_DEL_FRIEND(c) => {
            let (result, guid) = resolve_del_contact(store, conn.account_id, self_guid(conn).unwrap_or(0), c.guid.guid(), false)?;
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_FRIEND_STATUS(Box::new(
                    codec::build_friend_status(result, guid),
                ))),
            )?;
        }
        ClientOpcodeMessage::CMSG_DEL_IGNORE(c) => {
            let (result, guid) = resolve_del_contact(store, conn.account_id, self_guid(conn).unwrap_or(0), c.guid.guid(), true)?;
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_FRIEND_STATUS(Box::new(
                    codec::build_friend_status(result, guid),
                ))),
            )?;
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
            let result = match (self_guid(conn), party::resolve_by_name(store, &c.name)?) {
                (Some(me), Some(guid)) => {
                    match party::run(store, conn.account_id, me, party::Op::Invite(guid)) {
                        Ok(()) => PartyResult::Success,
                        Err(e) => party_result_for(&e.to_string()),
                    }
                }
                _ => PartyResult::BadPlayerName,
            };
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(Box::new(
                    codec::build_party_command_result(PartyOperation::Invite, c.name, result),
                ))),
            )?;
        }
        ClientOpcodeMessage::CMSG_GROUP_ACCEPT => {
            if let Some(me) = self_guid(conn) {
                if let Err(e) = party::run(store, conn.account_id, me, party::Op::Accept) {
                    log::debug!(
                        "world: group_accept ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
        }
        ClientOpcodeMessage::CMSG_GROUP_DECLINE => {
            if let Some(me) = self_guid(conn) {
                if let Err(e) = party::run(store, conn.account_id, me, party::Op::Decline) {
                    log::debug!(
                        "world: group_decline ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
        }
        ClientOpcodeMessage::CMSG_GROUP_DISBAND => {
            if let Some(me) = self_guid(conn) {
                if let Err(e) = party::run(store, conn.account_id, me, party::Op::Leave) {
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(Box::new(
                            codec::build_party_command_result(
                                PartyOperation::Leave,
                                String::new(),
                                party_result_for(&e.to_string()),
                            ),
                        ))),
                    )?;
                }
            }
        }
        ClientOpcodeMessage::CMSG_GROUP_UNINVITE(c) => {
            let result = match (self_guid(conn), party::resolve_by_name(store, &c.name)?) {
                (Some(me), Some(guid)) => {
                    match party::run(store, conn.account_id, me, party::Op::Uninvite(guid)) {
                        Ok(()) => PartyResult::Success,
                        Err(e) => party_result_for(&e.to_string()),
                    }
                }
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
            let setting = c.loot_setting.as_int();
            let threshold = c.loot_threshold.as_int();
            let master = c.loot_master.guid();
            if let Some(me) = self_guid(conn) {
                let op = party::Op::LootMethod {
                    setting,
                    master,
                    threshold,
                };
                if let Err(e) = party::run(store, conn.account_id, me, op) {
                    log::debug!(
                        "world: group_loot_method ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
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

/// Map the group reducers' Err strings onto the vanilla `PartyResult` codes the client renders
/// ("X is already in a group" etc.). Unknown reducer errors degrade to BadPlayerName — a visible,
/// non-crashing line — rather than being swallowed.
fn party_result_for(e: &str) -> PartyResult {
    // `contains`, not `==`: the reducer's Err string arrives wrapped in the SDK's error context.
    // The needles themselves are the lyracore-shared group contract — the module returns these exact
    // strings, so a reword is a one-place edit both sides see at compile time.
    use lyracore_shared::group::err as group_err;
    if e.contains(group_err::ALREADY_IN_GROUP) {
        PartyResult::AlreadyInGroup
    } else if e.contains(group_err::GROUP_FULL) {
        PartyResult::GroupFull
    } else if e.contains(group_err::NOT_LEADER) {
        PartyResult::NotLeader
    } else if e.contains(group_err::NOT_IN_GROUP) {
        PartyResult::NotInGroup
    } else if e.contains(group_err::TARGET_NOT_IN_GROUP) {
        PartyResult::TargetNotInGroup
    } else {
        PartyResult::BadPlayerName
    }
}

/// Resolve a typed contact name, call the module's add reducer (`add_friend`/`add_ignore`), and
/// translate the outcome into the `(FriendResult, guid)` pair `SMSG_FRIEND_STATUS` needs. An unknown
/// name never reaches the module (guid 0, `NotFound`) — everything else (self/duplicate/cap) is the
/// module's own rejection, string-matched here the same way `handle_trainer` parses buy failures.
fn resolve_add_contact<St: WorldStore + ?Sized>(
    store: &St,
    account_id: u64,
    actor_guid: u64,
    name: &str,
    is_ignore: bool,
) -> Result<(FriendResult, u64)> {
    let Some(target_guid) = store.character_guid_by_name(name)? else {
        return Ok((FriendResult::NotFound, 0));
    };
    let outcome = if is_ignore {
        store.add_ignore(account_id, actor_guid, target_guid)
    } else {
        store.add_friend(account_id, actor_guid, target_guid)
    };
    let result = match outcome {
        Ok(()) if is_ignore => FriendResult::IgnoreAdded,
        Ok(()) => {
            let online = store
                .character_presence(target_guid)?
                .map(|(online, ..)| online)
                .unwrap_or(false);
            if online {
                FriendResult::AddedOnline
            } else {
                FriendResult::AddedOffline
            }
        }
        Err(e) => {
            let es = e.to_string();
            if es.contains("cannot add yourself") {
                FriendResult::SelfX
            } else if es.contains("already added") {
                if is_ignore {
                    FriendResult::IgnoreAlready
                } else {
                    FriendResult::Already
                }
            } else if es.contains("list full") {
                if is_ignore {
                    FriendResult::IgnoreFull
                } else {
                    FriendResult::ListFull
                }
            } else {
                FriendResult::NotFound
            }
        }
    };
    Ok((result, target_guid))
}

/// Call the module's remove reducer (`del_friend`/`del_ignore`) for `target_guid` and translate the
/// outcome into the `(FriendResult, guid)` pair `SMSG_FRIEND_STATUS` needs.
fn resolve_del_contact<St: WorldStore + ?Sized>(
    store: &St,
    account_id: u64,
    actor_guid: u64,
    target_guid: u64,
    is_ignore: bool,
) -> Result<(FriendResult, u64)> {
    let outcome = if is_ignore {
        store.del_ignore(account_id, actor_guid, target_guid)
    } else {
        store.del_friend(account_id, actor_guid, target_guid)
    };
    let result = match outcome {
        Ok(()) if is_ignore => FriendResult::IgnoreRemoved,
        Ok(()) => FriendResult::Removed,
        Err(_) if is_ignore => FriendResult::IgnoreNotFound,
        Err(_) => FriendResult::NotFound,
    };
    Ok((result, target_guid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn party_result_for_maps_each_group_error_needle_and_falls_back() {
        // The needles are the lyracore-shared group contract — the module's Err strings
        // match these exact substrings, one PartyResult per reducer rejection reason.
        use lyracore_shared::group::err as group_err;
        assert_eq!(
            party_result_for(group_err::ALREADY_IN_GROUP),
            PartyResult::AlreadyInGroup
        );
        assert_eq!(
            party_result_for(group_err::GROUP_FULL),
            PartyResult::GroupFull
        );
        assert_eq!(
            party_result_for(group_err::NOT_LEADER),
            PartyResult::NotLeader
        );
        assert_eq!(
            party_result_for(group_err::NOT_IN_GROUP),
            PartyResult::NotInGroup
        );
        assert_eq!(
            party_result_for(group_err::TARGET_NOT_IN_GROUP),
            PartyResult::TargetNotInGroup
        );
        // `contains`, not `==`: the SDK wraps the reducer's raw Err string in error context, so the
        // needle must still hit through a wrapping prefix.
        assert_eq!(
            party_result_for(&format!("reducer call failed: {}", group_err::GROUP_FULL)),
            PartyResult::GroupFull
        );
        // An unrecognized error degrades to BadPlayerName (a visible line) rather than being swallowed.
        assert_eq!(
            party_result_for("some other error"),
            PartyResult::BadPlayerName
        );
    }
}
