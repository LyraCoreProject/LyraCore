#!/usr/bin/env python3
"""Create the initial Behavior files from the pinned client and vmangos opcode tables.

Run once from the repository root. The generated files are hand-maintained after creation.
"""

import pathlib
import re
import subprocess
import tomllib
import urllib.request
import argparse

ROOT = pathlib.Path(__file__).resolve().parents[2]
FEATURES = ROOT / "conformance/features"
SHA = "d9173eca3ea08dd985c52dabbcc826898d1d0b77"
RAW = f"https://raw.githubusercontent.com/vmangos/core/{SHA}/"

GROUPS = {
    "connection/character_management": "CMSG_CHAR_CREATE CMSG_CHAR_ENUM CMSG_CHAR_DELETE CMSG_CHAR_RENAME",
    "connection/world_entry": "CMSG_AUTH_SESSION CMSG_PLAYER_LOGIN MSG_MOVE_WORLDPORT_ACK CMSG_PLAYER_LOGOUT CMSG_LOGOUT_REQUEST CMSG_LOGOUT_CANCEL",
    "connection/account_preferences": "CMSG_REQUEST_ACCOUNT_DATA CMSG_UPDATE_ACCOUNT_DATA CMSG_TUTORIAL_FLAG CMSG_TUTORIAL_CLEAR CMSG_TUTORIAL_RESET CMSG_SET_ACTIONBAR_TOGGLES",
    "connection/client_security": "CMSG_WARDEN_DATA",
    "queries/world_queries": "CMSG_ITEM_QUERY_SINGLE CMSG_ITEM_QUERY_MULTIPLE CMSG_ITEM_NAME_QUERY CMSG_GAMEOBJECT_QUERY CMSG_CREATURE_QUERY CMSG_NPC_TEXT_QUERY CMSG_QUERY_TIME",
    "queries/readable_pages": "CMSG_PAGE_TEXT_QUERY CMSG_READ_ITEM",
    "social/who_search": "CMSG_WHO",
    "social/friends_and_ignores": "CMSG_FRIEND_LIST CMSG_ADD_FRIEND CMSG_DEL_FRIEND CMSG_ADD_IGNORE CMSG_DEL_IGNORE",
    "groups/party_membership": "CMSG_GROUP_INVITE CMSG_GROUP_ACCEPT CMSG_GROUP_DECLINE CMSG_GROUP_CANCEL CMSG_GROUP_UNINVITE CMSG_GROUP_UNINVITE_GUID CMSG_GROUP_DISBAND",
    "groups/party_leadership": "CMSG_GROUP_SET_LEADER CMSG_LOOT_METHOD",
    "groups/raid_management": "CMSG_GROUP_RAID_CONVERT CMSG_GROUP_CHANGE_SUB_GROUP CMSG_GROUP_SWAP_SUB_GROUP CMSG_GROUP_ASSISTANT_LEADER MSG_RAID_TARGET_UPDATE MSG_RAID_READY_CHECK CMSG_REQUEST_RAID_INFO",
    "groups/party_status": "CMSG_REQUEST_PARTY_MEMBER_STATS MSG_MINIMAP_PING MSG_RANDOM_ROLL",
    "guilds/guild_membership": "CMSG_GUILD_CREATE CMSG_GUILD_INVITE CMSG_GUILD_ACCEPT CMSG_GUILD_DECLINE CMSG_GUILD_LEAVE CMSG_GUILD_REMOVE CMSG_GUILD_DISBAND",
    "guilds/guild_roster": "CMSG_GUILD_QUERY CMSG_GUILD_INFO CMSG_GUILD_ROSTER CMSG_GUILD_SET_PUBLIC_NOTE CMSG_GUILD_SET_OFFICER_NOTE CMSG_GUILD_MOTD CMSG_GUILD_INFO_TEXT",
    "guilds/guild_authority": "CMSG_GUILD_PROMOTE CMSG_GUILD_DEMOTE CMSG_GUILD_LEADER CMSG_GUILD_RANK CMSG_GUILD_ADD_RANK CMSG_GUILD_DEL_RANK",
    "guilds/guild_emblem": "MSG_TABARDVENDOR_ACTIVATE MSG_SAVE_GUILD_EMBLEM",
    "chat/chat_messages": "CMSG_MESSAGECHAT CMSG_CHAT_IGNORED",
    "chat/chat_emotes": "CMSG_EMOTE CMSG_TEXT_EMOTE",
    "channels/channel_membership": "CMSG_JOIN_CHANNEL CMSG_LEAVE_CHANNEL CMSG_CHANNEL_LIST CMSG_CHANNEL_INVITE CMSG_CHANNEL_KICK CMSG_CHANNEL_BAN CMSG_CHANNEL_UNBAN",
    "channels/channel_settings": "CMSG_CHANNEL_PASSWORD CMSG_CHANNEL_SET_OWNER CMSG_CHANNEL_OWNER CMSG_CHANNEL_MODERATOR CMSG_CHANNEL_UNMODERATOR CMSG_CHANNEL_MUTE CMSG_CHANNEL_UNMUTE CMSG_CHANNEL_ANNOUNCEMENTS CMSG_CHANNEL_MODERATE",
    "inventory/item_use": "CMSG_USE_ITEM CMSG_OPEN_ITEM CMSG_WRAP_ITEM",
    "inventory/bag_management": "CMSG_AUTOEQUIP_ITEM CMSG_AUTOSTORE_BAG_ITEM CMSG_SWAP_ITEM CMSG_SWAP_INV_ITEM CMSG_SPLIT_ITEM CMSG_AUTOEQUIP_ITEM_SLOT CMSG_DESTROYITEM",
    "inventory/bank_storage": "CMSG_BANKER_ACTIVATE CMSG_BUY_BANK_SLOT CMSG_AUTOSTORE_BANK_ITEM CMSG_AUTOBANK_ITEM",
    "inventory/ammunition": "CMSG_SET_AMMO",
    "inventory/equipment_display": "CMSG_TOGGLE_HELM CMSG_TOGGLE_CLOAK",
    "inventory/item_inspection": "CMSG_INSPECT",
    "vendors/vendor_transactions": "CMSG_LIST_INVENTORY CMSG_SELL_ITEM CMSG_BUY_ITEM CMSG_BUY_ITEM_IN_SLOT CMSG_BUYBACK_ITEM CMSG_REPAIR_ITEM",
    "trade/direct_trade": "CMSG_INITIATE_TRADE CMSG_BEGIN_TRADE CMSG_BUSY_TRADE CMSG_IGNORE_TRADE CMSG_ACCEPT_TRADE CMSG_UNACCEPT_TRADE CMSG_CANCEL_TRADE CMSG_SET_TRADE_ITEM CMSG_CLEAR_TRADE_ITEM CMSG_SET_TRADE_GOLD",
    "mail/mailbox": "CMSG_GET_MAIL_LIST MSG_QUERY_NEXT_MAIL_TIME CMSG_MAIL_MARK_AS_READ CMSG_MAIL_DELETE",
    "mail/mail_delivery": "CMSG_SEND_MAIL CMSG_MAIL_RETURN_TO_SENDER CMSG_MAIL_TAKE_MONEY CMSG_MAIL_TAKE_ITEM CMSG_MAIL_CREATE_TEXT_ITEM CMSG_ITEM_TEXT_QUERY",
    "auctions/auction_house": "MSG_AUCTION_HELLO CMSG_AUCTION_SELL_ITEM CMSG_AUCTION_REMOVE_ITEM CMSG_AUCTION_LIST_ITEMS CMSG_AUCTION_LIST_OWNER_ITEMS CMSG_AUCTION_PLACE_BID CMSG_AUCTION_LIST_BIDDER_ITEMS",
    "movement/character_movement": "MSG_MOVE_START_FORWARD MSG_MOVE_START_BACKWARD MSG_MOVE_STOP MSG_MOVE_START_STRAFE_LEFT MSG_MOVE_START_STRAFE_RIGHT MSG_MOVE_STOP_STRAFE MSG_MOVE_JUMP MSG_MOVE_START_TURN_LEFT MSG_MOVE_START_TURN_RIGHT MSG_MOVE_STOP_TURN MSG_MOVE_START_PITCH_UP MSG_MOVE_START_PITCH_DOWN MSG_MOVE_STOP_PITCH MSG_MOVE_SET_RUN_MODE MSG_MOVE_SET_WALK_MODE MSG_MOVE_FALL_LAND MSG_MOVE_START_SWIM MSG_MOVE_STOP_SWIM MSG_MOVE_SET_FACING MSG_MOVE_SET_PITCH MSG_MOVE_HEARTBEAT CMSG_MOVE_FALL_RESET",
    "movement/movement_receipts": "CMSG_FORCE_RUN_SPEED_CHANGE_ACK CMSG_FORCE_RUN_BACK_SPEED_CHANGE_ACK CMSG_FORCE_SWIM_SPEED_CHANGE_ACK CMSG_FORCE_MOVE_ROOT_ACK CMSG_FORCE_MOVE_UNROOT_ACK CMSG_MOVE_KNOCK_BACK_ACK CMSG_MOVE_HOVER_ACK CMSG_MOVE_SPLINE_DONE CMSG_MOVE_TIME_SKIPPED CMSG_MOVE_FEATHER_FALL_ACK CMSG_MOVE_WATER_WALK_ACK CMSG_FORCE_WALK_SPEED_CHANGE_ACK CMSG_FORCE_SWIM_BACK_SPEED_CHANGE_ACK CMSG_FORCE_TURN_RATE_CHANGE_ACK",
    "movement/active_mover": "CMSG_SET_ACTIVE_MOVER CMSG_MOVE_NOT_ACTIVE_MOVER",
    "movement/summon_response": "CMSG_SUMMON_RESPONSE",
    "movement/mount_flourish": "CMSG_MOUNTSPECIAL_ANIM",
    "movement/remote_camera": "CMSG_FAR_SIGHT",
    "world/area_updates": "CMSG_AREATRIGGER CMSG_ZONEUPDATE",
    "world/cinematics": "CMSG_OPENING_CINEMATIC CMSG_NEXT_CINEMATIC_CAMERA CMSG_COMPLETE_CINEMATIC",
    "world/time_and_played": "CMSG_PLAYED_TIME",
    "world/bug_reports": "CMSG_BUG CMSG_GMSURVEY_SUBMIT",
    "world/instance_reset": "CMSG_RESET_INSTANCES",
    "combat/auto_attack": "CMSG_ATTACKSWING CMSG_ATTACKSTOP CMSG_SETSHEATHED CMSG_SET_SELECTION CMSG_SET_TARGET_OBSOLETE",
    "combat/duel_response": "CMSG_DUEL_ACCEPTED CMSG_DUEL_CANCELLED",
    "combat/death_and_resurrection": "CMSG_REPOP_REQUEST CMSG_RESURRECT_RESPONSE CMSG_RECLAIM_CORPSE MSG_CORPSE_QUERY CMSG_SPIRIT_HEALER_ACTIVATE CMSG_SELF_RES",
    "spells/cast_and_interrupt": "CMSG_CAST_SPELL CMSG_CANCEL_CAST CMSG_CANCEL_AURA CMSG_CANCEL_CHANNELLING CMSG_CANCEL_AUTO_REPEAT_SPELL CMSG_CANCEL_GROWTH_AURA CMSG_GAMEOBJ_USE",
    "talents/talent_learning": "CMSG_LEARN_TALENT MSG_TALENT_WIPE_CONFIRM",
    "skills/skill_unlearning": "CMSG_UNLEARN_SKILL",
    "actions/action_buttons": "CMSG_SET_ACTION_BUTTON",
    "actions/stand_state": "CMSG_STANDSTATECHANGE",
    "reputation/faction_preferences": "CMSG_SET_FACTION_ATWAR CMSG_SET_FACTION_INACTIVE CMSG_SET_WATCHED_FACTION",
    "pvp/pvp_flags_and_honor": "CMSG_TOGGLE_PVP MSG_INSPECT_HONOR_STATS",
    "pets/pet_commands": "CMSG_PET_SET_ACTION CMSG_PET_ACTION CMSG_PET_ABANDON CMSG_PET_CAST_SPELL CMSG_PET_STOP_ATTACK CMSG_PET_UNLEARN CMSG_PET_SPELL_AUTOCAST CMSG_PET_CANCEL_AURA CMSG_REQUEST_PET_INFO",
    "pets/pet_stables": "MSG_LIST_STABLED_PETS CMSG_STABLE_PET CMSG_UNSTABLE_PET CMSG_BUY_STABLE_SLOT CMSG_STABLE_REVIVE_PET CMSG_STABLE_SWAP_PET",
    "gossip/npc_dialogue": "CMSG_GOSSIP_HELLO CMSG_GOSSIP_SELECT_OPTION",
    "quests/quest_offers": "CMSG_QUEST_QUERY CMSG_QUESTGIVER_STATUS_QUERY CMSG_QUESTGIVER_HELLO CMSG_QUESTGIVER_QUERY_QUEST CMSG_QUESTGIVER_QUEST_AUTOLAUNCH CMSG_QUESTGIVER_CANCEL",
    "quests/quest_log": "CMSG_QUESTLOG_SWAP_QUEST CMSG_QUESTLOG_REMOVE_QUEST",
    "quests/quest_sharing": "CMSG_PUSHQUESTTOPARTY CMSG_QUEST_CONFIRM_ACCEPT MSG_QUEST_PUSH_RESULT",
    "quests/quest_completion": "CMSG_QUESTGIVER_COMPLETE_QUEST CMSG_QUESTGIVER_REQUEST_REWARD",
    "taxi/taxi_chains": "CMSG_ACTIVATETAXIEXPRESS",
    "trainers/trainer_purchases": "CMSG_TRAINER_LIST CMSG_TRAINER_BUY_SPELL",
    "inns/home_binding": "CMSG_BINDER_ACTIVATE",
    "petitions/guild_petitions": "MSG_PETITION_DECLINE MSG_PETITION_RENAME MSG_DELETE_GUILD_CHARTER CMSG_PETITION_SHOWLIST CMSG_PETITION_BUY CMSG_PETITION_SHOW_SIGNATURES CMSG_PETITION_SIGN CMSG_OFFER_PETITION CMSG_TURN_IN_PETITION CMSG_PETITION_QUERY",
    "lfg/meeting_stone": "CMSG_MEETINGSTONE_JOIN CMSG_MEETINGSTONE_LEAVE CMSG_MEETINGSTONE_INFO MSG_LOOKING_FOR_GROUP",
    "lfg/lfg_preferences": "CMSG_SET_LOOKING_FOR_GROUP",
    "loot/loot_collection": "CMSG_LOOT_MONEY CMSG_LOOT_RELEASE CMSG_LOOT_MASTER_GIVE CMSG_LOOT_ROLL",
    "battlegrounds/battleground_entry": "CMSG_BATTLEFIELD_LIST CMSG_BATTLEFIELD_JOIN CMSG_BATTLEFIELD_PORT CMSG_BATTLEMASTER_HELLO CMSG_BATTLEMASTER_JOIN CMSG_LEAVE_BATTLEFIELD",
    "battlegrounds/battleground_state": "MSG_PVP_LOG_DATA CMSG_AREA_SPIRIT_HEALER_QUERY CMSG_AREA_SPIRIT_HEALER_QUEUE MSG_BATTLEGROUND_PLAYER_POSITIONS",
    "support/gm_tickets": "CMSG_GMTICKET_CREATE CMSG_GMTICKET_UPDATETEXT CMSG_GMTICKET_GETTICKET CMSG_GMTICKET_DELETETICKET CMSG_GMTICKET_SYSTEMSTATUS",
}

OUT_OF_SCOPE = {
    "MSG_NULL_ACTION": "Reserved null action; vmangos marks it invalid.",
    "CMSG_BOOTME": "Developer disconnect command; the normal client does not send it during play.",
    "CMSG_DBLOOKUP": "GM lookup command; vmangos marks it unhandled.",
    "CMSG_WORLD_TELEPORT": "GM teleport command; normal play uses area and taxi travel.",
    "CMSG_TELEPORT_TO_UNIT": "GM teleport command; normal play uses area and taxi travel.",
    "CMSG_WHOIS": "GM account lookup command, outside player interaction coverage.",
    "CMSG_MOVE_SET_RAW_POSITION": "Debug coordinate setter, outside stock client movement.",
    "MSG_MOVE_WATER_WALK": "Obsolete movement message; vmangos marks it unhandled.",
    "MSG_UPDATE_GROUP_MEMBERS": "Server-origin group update; vmangos marks it unhandled as a client request.",
    "MSG_UPDATE_GUILD": "Server-origin guild update; vmangos marks it unhandled as a client request.",
    "MSG_CHANNEL_START": "Unused channel control message; vmangos marks it unhandled.",
    "MSG_CHANNEL_UPDATE": "Unused channel control message; vmangos marks it unhandled.",
    "MSG_SET_DUNGEON_DIFFICULTY": "Later client difficulty control; build 5875 has no player difficulty choice.",
    "MSG_DELAY_GHOST_TELEPORT": "Unused ghost relocation control; vmangos marks it unhandled.",
    "CMSG_AUTOEQUIP_GROUND_ITEM": "Ground-item storage request; normal 1.12.1 loot uses CMSG_AUTOSTORE_LOOT_ITEM.",
    "CMSG_AUTOSTORE_GROUND_ITEM": "Ground-item storage request; normal 1.12.1 loot uses CMSG_AUTOSTORE_LOOT_ITEM.",
    "CMSG_STORE_LOOT_IN_SLOT": "Unused loot-slot store request; normal 1.12.1 loot uses CMSG_AUTOSTORE_LOOT_ITEM.",
    "CMSG_SKILL_BUY_STEP": "Unused skill-point purchase request; 1.12.1 trainers use CMSG_TRAINER_BUY_SPELL.",
    "CMSG_SKILL_BUY_RANK": "Unused skill-point purchase request; 1.12.1 trainers use CMSG_TRAINER_BUY_SPELL.",
    "CMSG_AUTH_SRP6_BEGIN": "Old world SRP exchange; build 5875 uses CMSG_AUTH_SESSION.",
    "CMSG_AUTH_SRP6_PROOF": "Old world SRP exchange; build 5875 uses CMSG_AUTH_SESSION.",
    "CMSG_AUTH_SRP6_RECODE": "Old world SRP exchange; build 5875 uses CMSG_AUTH_SESSION.",
    "CMSG_UNLEARN_TALENTS": "Unused talent reset request; 1.12.1 uses MSG_TALENT_WIPE_CONFIRM.",
    "CMSG_UNLEARN_SPELL": "Unused spell removal request; vmangos marks it unhandled.",
    "CMSG_LUA_USAGE": "Client telemetry, not a player-visible gameplay request; vmangos marks it unhandled.",
    "CMSG_CHAT_FILTERED": "Unused chat-filter notification; vmangos marks it unhandled.",
}

def out_of_scope_reason(name, dispatch):
    if name in OUT_OF_SCOPE:
        return OUT_OF_SCOPE[name]
    if "_OBSOLETE" in name:
        return "Obsolete in build 5875; vmangos marks this request unhandled."
    if "CHEAT" in name or name.startswith(("CMSG_GM_", "MSG_GM_")):
        return "GM or debug command, outside normal client play."
    if name.startswith("MSG_MOVE_"):
        return "Server-origin or obsolete movement message; vmangos marks it unhandled."
    if name.startswith(("CMSG_DEBUG_", "CMSG_CREATE", "CMSG_DESTROYMONSTER", "CMSG_FORCEACTION")):
        return "Developer command; vmangos marks it unhandled."
    if name in dispatch and dispatch[name] in ("Unhandled", "Invalid"):
        return "No normal-play UI use identified; vmangos marks this request unhandled or invalid."
    raise AssertionError((name, dispatch.get(name)))

SCOPES = {
    "character_management": "A Realm Account can list, create, delete and rename its 1.12.1 Characters.",
    "world_entry": "A Character can authenticate, enter and leave the world with the expected transition packets.",
    "account_preferences": "The client can save account preferences and tutorial flags and receive them on request.",
    "client_security": "The client and Realm exchange the 1.12.1 Warden data required for normal sessions.",
    "world_queries": "The client can query item, creature, gameobject, NPC text and time data used by the world UI.",
    "readable_pages": "A Character can open readable item and page text and receive its complete text chain.",
    "who_search": "A Character can search the online Character list with 1.12.1 who filters.",
    "friends_and_ignores": "A Character can list and change friends and ignored Characters.",
    "party_membership": "Characters can invite, join, decline, remove and leave a party.",
    "party_leadership": "A party leader can hand over leadership and choose the loot method.",
    "raid_management": "A raid leader can organize subgroups, assistants, targets, ready checks and lockout information.",
    "party_status": "Party members can request member state, ping the map and roll a shared random number.",
    "guild_membership": "Characters can create, join, leave and dissolve a guild under its authority rules.",
    "guild_roster": "Guild members can query the roster, notes, message of the day and guild information.",
    "guild_authority": "Guild leaders can assign ranks, promote members and transfer leadership.",
    "guild_emblem": "A guild leader can preview and buy a tabard emblem.",
    "chat_messages": "Characters can send 1.12.1 chat messages and report ignored chat.",
    "chat_emotes": "Characters can use named and text emotes visible to nearby recipients.",
    "channel_membership": "Characters can join, list, invite and remove members of Chat Channels.",
    "channel_settings": "Chat Channel owners and moderators can change access, roles and notices.",
    "item_use": "A Character can use, open and wrap inventory items under their item rules.",
    "bag_management": "A Character can equip, move, split and destroy items across bags and equipment.",
    "bank_storage": "A Character can open the bank, buy slots and transfer items between bank and bags.",
    "ammunition": "A Character can select ammunition for ranged attacks.",
    "equipment_display": "A Character can toggle the visible helm and cloak preferences.",
    "item_inspection": "A Character can inspect another Character's visible equipment.",
    "vendor_transactions": "A Character can browse, buy, sell, buy back and repair items at a vendor.",
    "direct_trade": "Two Characters can negotiate and complete a direct item and money trade atomically.",
    "mailbox": "A Character can list, read and delete mailbox entries and query the next delivery time.",
    "mail_delivery": "A Character can send, return and collect Mail, including attachments, money and text items.",
    "auction_house": "A Character can list, post, bid on and cancel Auctions through an auctioneer.",
    "character_movement": "A Character can move, turn, jump, swim and fall while nearby clients see valid positions.",
    "movement_receipts": "The client acknowledges forced movement, speed, teleport and spline changes in order.",
    "active_mover": "A Character can select and report movement for its controlled unit.",
    "summon_response": "A Character can accept or decline a summon invitation.",
    "mount_flourish": "A mounted Character can play the mount flourish animation.",
    "remote_camera": "A Character can use a remote camera when a spell grants that view.",
    "area_updates": "Area trigger and zone updates apply the correct world transition rules.",
    "cinematics": "The client can advance and complete opening cinematic playback.",
    "time_and_played": "A Character can query its played time.",
    "bug_reports": "A Character can submit bug reports and post-support surveys.",
    "instance_reset": "A group leader can reset eligible Instances.",
    "auto_attack": "A Character can choose a target and start, stop or sheath auto attacks.",
    "duel_response": "Two Characters can accept or cancel a duel challenge.",
    "death_and_resurrection": "A dead Character can release, find, reclaim or resurrect its body.",
    "cast_and_interrupt": "A Character can cast and interrupt spells and cancel active auras.",
    "talent_learning": "A Character can spend and reset talent points with class and cost rules.",
    "skill_unlearning": "A Character can unlearn a profession skill when the rules permit it.",
    "action_buttons": "A Character can set and clear action button assignments.",
    "stand_state": "A Character can change its standing or sitting state.",
    "faction_preferences": "A Character can change war, inactive and watched faction preferences.",
    "pvp_flags_and_honor": "A Character can toggle its PvP flag and inspect honor statistics.",
    "pet_commands": "A Character can command, cast through, edit and dismiss its controlled pet.",
    "pet_stables": "A hunter can list, store, retrieve, revive and swap pets at a stable master.",
    "npc_dialogue": "A Character can open and choose a gossip option from an NPC.",
    "quest_offers": "A Character can query available quests and inspect their offered requirements.",
    "quest_log": "A Character can reorder and abandon quests in its quest log.",
    "quest_sharing": "A Character can share a quest with party members and receive their responses.",
    "quest_completion": "A Character can ask for quest completion and inspect the offered reward.",
    "taxi_chains": "A Character can book a multi-hop taxi journey through known flight nodes.",
    "trainer_purchases": "A Character can browse and buy spells from a trainer when eligible.",
    "home_binding": "A Character can bind its hearthstone at an innkeeper.",
    "guild_petitions": "Characters can buy, sign, offer and turn in a guild petition.",
    "meeting_stone": "Characters can join, inspect and leave meeting-stone group finding.",
    "lfg_preferences": "A Character can save looking-for-group preferences from the 1.12.1 client.",
    "loot_collection": "Eligible Characters can collect, assign and roll for loot from an open source.",
    "battleground_entry": "A Character can browse, join and leave battleground queues and instances.",
    "battleground_state": "A battleground participant can inspect score, positions and spirit-healer state.",
    "gm_tickets": "A Character can inspect, submit, update and delete a support ticket.",
}


def opcode_source():
    metadata = subprocess.check_output(["cargo", "metadata", "--locked", "--format-version", "1"], cwd=ROOT)
    import json
    packages = [p for p in json.loads(metadata)["packages"] if p["name"] == "wow_world_messages"]
    assert len(packages) == 1
    return pathlib.Path(packages[0]["manifest_path"]).parent / "src/world/vanilla/opcodes.rs"


def raw(path):
    return urllib.request.urlopen(RAW + path).read().decode()


def quoted(text):
    import json
    return json.dumps(text, ensure_ascii=False)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-dir", type=pathlib.Path, default=ROOT / "conformance")
    output = parser.parse_args().output_dir.resolve()
    client = opcode_source().read_text()
    enum = client.split("pub enum ClientOpcodeMessage {", 1)[1].split("\n}", 1)[0]
    names = set(re.findall(r"^    ((?:CMSG|MSG)_[A-Z0-9_]+)", enum, re.M))
    ids = {name: int(number, 16) for number, name in re.findall(r"0x([0-9A-Fa-f]{4}) => [^\n]*?Self::([A-Z0-9_]+)", client)}
    assert names <= ids.keys(), sorted(names - ids.keys())
    dispatch = raw("src/game/Server/Protocol/Opcodes.cpp")
    header = raw("src/game/Server/Protocol/Opcodes_1_12_1.h")
    all_ids = {name: int(number) for name, number in re.findall(r"^\s*((?:CMSG|MSG)_[A-Z0-9_]+)\s*=\s*(\d+)", header, re.M)}
    assert all_ids.keys() >= names
    assert all(ids[name] == all_ids[name] for name in names)
    handlers = dict(re.findall(r"DEFINE_HANDLER\(\s*((?:CMSG|MSG)_[A-Z0-9_]+),[^\n]*?&WorldSession::([A-Za-z0-9_]+)", dispatch))
    invalid = dict(re.findall(r"INVALID_PACKET\(\s*((?:CMSG|MSG)_[A-Z0-9_]+),\s*UnhandleReason::([A-Za-z0-9_]+)", dispatch))
    assert all_ids.keys() == handlers.keys() | invalid.keys()
    cmake = raw("src/game/CMakeLists.txt")
    paths = sorted(set(re.findall(r"(?:Handlers/[A-Za-z]+Handler|LFG/LFGHandler)\.cpp", cmake)))
    sources = {"src/game/" + path: raw("src/game/" + path) for path in paths}
    sources["src/game/Server/WorldSession.cpp"] = raw("src/game/Server/WorldSession.cpp")
    baseline = (ROOT / "conformance/tools/ledger-denominator-base.txt").read_text().splitlines()
    claimed = {int(opcode, 16) for name in baseline for opcode in tomllib.loads((FEATURES / name).read_text())["requests"]}
    assigned = {}
    for group, request_names in GROUPS.items():
        assert group.split("/")[1] in SCOPES, group
        for name in request_names.split():
            assert name in all_ids and name not in assigned, name
            assigned[name] = group
    assert all_ids.keys() >= assigned.keys()
    assert all_ids.keys() >= OUT_OF_SCOPE.keys()
    assert names <= set(assigned) | set(OUT_OF_SCOPE) | {name for name, number in ids.items() if number in claimed}
    assert not (set(OUT_OF_SCOPE) & set(assigned))
    out_of_scope = {name: out_of_scope_reason(name, invalid) for name in all_ids.keys() - assigned.keys() if all_ids[name] not in claimed}
    output.mkdir(parents=True, exist_ok=True)
    inventory = output / "client-requests-5875.tsv"
    assert not inventory.exists(), inventory
    inventory.write_text("".join(f"0x{number:04X} {name}\n" for name, number in sorted(all_ids.items(), key=lambda item: item[1])))
    unavailable = (ROOT / "module/src/native/unavailable.rs").read_text()
    unavailable_names = set(re.findall(r"ClientOpcodeMessage::([A-Z0-9_]+)", unavailable.split("fn unavailable(", 1)[0]))
    unavailable_numbers = {0x0205, 0x0207, 0x023E, 0x00F9, 0x0200, 0x0312, 0x0317, 0x005A}
    for group, request_names in GROUPS.items():
        system, behavior = group.split("/")
        requested = [name for name in request_names.split() if all_ids[name] not in claimed]
        if not requested:
            continue
        target = output / "features" / system / (behavior + ".toml")
        assert not target.exists(), target
        served = [name for name in requested if name in handlers]
        if served:
            first = served[0]
            symbol = handlers[first]
            matches = [path for path, source in sources.items() if re.search(r"WorldSession::" + re.escape(symbol) + r"\s*\(", source)]
            assert len(matches) == 1, (first, symbol, matches)
            source_path, source_symbol = matches[0], "WorldSession::" + symbol
        else:
            assert all(name in invalid for name in requested), group
            source_path, source_symbol = "src/game/Server/Protocol/Opcodes.cpp", "BuildOpcodeList"
        opcodes = ", ".join(quoted(f"0x{all_ids[name]:04X}") for name in requested)
        lines = ["schema_version = 1", f"id = {quoted(behavior)}", f"system = {quoted(system)}", f"scope = {quoted(SCOPES[behavior])}", f"requests = [{opcodes}]", "replies = []", "dependencies = []", 'required_rungs = ["tier1", "conformance_audit", "real_client"]', "scenarios = []", "deliberate_deviations = []", "prerequisites = []", "tests = []", "", "[vmangos]", f"commit = {quoted(SHA)}", f"path = {quoted(source_path)}", f"symbol = {quoted(source_symbol)}", "", "[real_client]", 'status = "unverified"']
        gaps = [name for name in requested if name in unavailable_names or all_ids[name] in unavailable_numbers]
        if gaps:
            lines += ["", "[[known_gaps]]", f"description = {quoted('The Module returns an unavailable response for ' + ', '.join(gaps) + '.')}", 'path = "module/src/native/unavailable.rs"', f"symbol = {quoted('responses' if any(name in unavailable_names for name in gaps) else 'raw_responses')}", 'evidence = "module/src/native/unavailable.rs; docs/benilla-compatibility.md#missing-gameplay-and-deliberate-responses"']
        if behavior == "pet_stables":
            lines += ["", "[[known_gaps]]", 'description = "Stable pet revival has no native request decoder or response."', 'path = "module/src/native/mod.rs"', 'symbol = "receive"', 'evidence = "Build-5875 CMSG_STABLE_REVIVE_PET is absent from ClientOpcodeMessage and module/src/native/unavailable.rs; docs/benilla-compatibility.md#missing-gameplay-and-deliberate-responses"']
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("\n".join(lines) + "\n")
    out = output / "out-of-scope-opcodes.toml"
    assert not out.exists(), out
    lines = ["schema_version = 1"]
    for name, reason in sorted(out_of_scope.items(), key=lambda entry: all_ids[entry[0]]):
        lines += ["", "[[opcode]]", f"name = {quoted(name)}", f"value = {quoted(f'0x{all_ids[name]:04X}')}", f"reason = {quoted(reason)}"]
    out.write_text("\n".join(lines) + "\n")
    print(f"Created {len(GROUPS)} Behavior groups; {len(out_of_scope)} out-of-scope opcodes")


if __name__ == "__main__":
    main()
