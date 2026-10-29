use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use anyhow::{ensure, Result};
use lyracore_conformance::FeatureLedger;
use serde::Deserialize;

#[path = "../support/opcodes.rs"]
mod opcodes;

#[path = "../support/scratch.rs"]
mod scratch;

const INITIAL_UNCLAIMED: &str = "CMSG_ACCEPT_TRADE
CMSG_ACTIVATETAXIEXPRESS
CMSG_ADD_FRIEND
CMSG_ADD_IGNORE
CMSG_AREATRIGGER
CMSG_AREA_SPIRIT_HEALER_QUERY
CMSG_AREA_SPIRIT_HEALER_QUEUE
CMSG_ATTACKSTOP
CMSG_ATTACKSWING
CMSG_AUCTION_LIST_BIDDER_ITEMS
CMSG_AUCTION_LIST_ITEMS
CMSG_AUCTION_LIST_OWNER_ITEMS
CMSG_AUCTION_PLACE_BID
CMSG_AUCTION_REMOVE_ITEM
CMSG_AUCTION_SELL_ITEM
CMSG_AUTH_SESSION
CMSG_AUTOBANK_ITEM
CMSG_AUTOEQUIP_ITEM
CMSG_AUTOEQUIP_ITEM_SLOT
CMSG_AUTOSTORE_BAG_ITEM
CMSG_AUTOSTORE_BANK_ITEM
CMSG_BANKER_ACTIVATE
CMSG_BATTLEFIELD_JOIN
CMSG_BATTLEFIELD_LIST
CMSG_BATTLEFIELD_PORT
CMSG_BATTLEMASTER_HELLO
CMSG_BATTLEMASTER_JOIN
CMSG_BEGIN_TRADE
CMSG_BINDER_ACTIVATE
CMSG_BOOTME
CMSG_BUG
CMSG_BUSY_TRADE
CMSG_BUYBACK_ITEM
CMSG_BUY_BANK_SLOT
CMSG_BUY_ITEM
CMSG_BUY_ITEM_IN_SLOT
CMSG_BUY_STABLE_SLOT
CMSG_CANCEL_AURA
CMSG_CANCEL_AUTO_REPEAT_SPELL
CMSG_CANCEL_CAST
CMSG_CANCEL_CHANNELLING
CMSG_CANCEL_GROWTH_AURA
CMSG_CANCEL_TRADE
CMSG_CAST_SPELL
CMSG_CHANNEL_ANNOUNCEMENTS
CMSG_CHANNEL_BAN
CMSG_CHANNEL_INVITE
CMSG_CHANNEL_KICK
CMSG_CHANNEL_LIST
CMSG_CHANNEL_MODERATE
CMSG_CHANNEL_MODERATOR
CMSG_CHANNEL_MUTE
CMSG_CHANNEL_OWNER
CMSG_CHANNEL_PASSWORD
CMSG_CHANNEL_SET_OWNER
CMSG_CHANNEL_UNBAN
CMSG_CHANNEL_UNMODERATOR
CMSG_CHANNEL_UNMUTE
CMSG_CHAR_CREATE
CMSG_CHAR_DELETE
CMSG_CHAR_ENUM
CMSG_CHAR_RENAME
CMSG_CHAT_IGNORED
CMSG_CLEAR_TRADE_ITEM
CMSG_COMPLETE_CINEMATIC
CMSG_CREATURE_QUERY
CMSG_DBLOOKUP
CMSG_DEL_FRIEND
CMSG_DEL_IGNORE
CMSG_DESTROYITEM
CMSG_DUEL_ACCEPTED
CMSG_DUEL_CANCELLED
CMSG_EMOTE
CMSG_FAR_SIGHT
CMSG_FORCE_MOVE_ROOT_ACK
CMSG_FORCE_MOVE_UNROOT_ACK
CMSG_FORCE_RUN_BACK_SPEED_CHANGE_ACK
CMSG_FORCE_RUN_SPEED_CHANGE_ACK
CMSG_FORCE_SWIM_BACK_SPEED_CHANGE_ACK
CMSG_FORCE_SWIM_SPEED_CHANGE_ACK
CMSG_FORCE_TURN_RATE_CHANGE_ACK
CMSG_FORCE_WALK_SPEED_CHANGE_ACK
CMSG_FRIEND_LIST
CMSG_GAMEOBJECT_QUERY
CMSG_GAMEOBJ_USE
CMSG_GET_MAIL_LIST
CMSG_GMSURVEY_SUBMIT
CMSG_GMTICKET_CREATE
CMSG_GMTICKET_DELETETICKET
CMSG_GMTICKET_GETTICKET
CMSG_GMTICKET_SYSTEMSTATUS
CMSG_GMTICKET_UPDATETEXT
CMSG_GROUP_ACCEPT
CMSG_GROUP_ASSISTANT_LEADER
CMSG_GROUP_CHANGE_SUB_GROUP
CMSG_GROUP_DECLINE
CMSG_GROUP_DISBAND
CMSG_GROUP_INVITE
CMSG_GROUP_RAID_CONVERT
CMSG_GROUP_SET_LEADER
CMSG_GROUP_SWAP_SUB_GROUP
CMSG_GROUP_UNINVITE
CMSG_GROUP_UNINVITE_GUID
CMSG_GUILD_ACCEPT
CMSG_GUILD_ADD_RANK
CMSG_GUILD_CREATE
CMSG_GUILD_DECLINE
CMSG_GUILD_DEL_RANK
CMSG_GUILD_DEMOTE
CMSG_GUILD_DISBAND
CMSG_GUILD_INFO
CMSG_GUILD_INFO_TEXT
CMSG_GUILD_INVITE
CMSG_GUILD_LEADER
CMSG_GUILD_LEAVE
CMSG_GUILD_MOTD
CMSG_GUILD_PROMOTE
CMSG_GUILD_QUERY
CMSG_GUILD_RANK
CMSG_GUILD_REMOVE
CMSG_GUILD_ROSTER
CMSG_GUILD_SET_OFFICER_NOTE
CMSG_GUILD_SET_PUBLIC_NOTE
CMSG_IGNORE_TRADE
CMSG_INITIATE_TRADE
CMSG_INSPECT
CMSG_ITEM_NAME_QUERY
CMSG_ITEM_QUERY_SINGLE
CMSG_ITEM_TEXT_QUERY
CMSG_JOIN_CHANNEL
CMSG_LEARN_TALENT
CMSG_LEAVE_BATTLEFIELD
CMSG_LEAVE_CHANNEL
CMSG_LIST_INVENTORY
CMSG_LOGOUT_CANCEL
CMSG_LOGOUT_REQUEST
CMSG_LOOT_MASTER_GIVE
CMSG_LOOT_METHOD
CMSG_LOOT_RELEASE
CMSG_LOOT_ROLL
CMSG_MAIL_CREATE_TEXT_ITEM
CMSG_MAIL_DELETE
CMSG_MAIL_MARK_AS_READ
CMSG_MAIL_RETURN_TO_SENDER
CMSG_MAIL_TAKE_ITEM
CMSG_MAIL_TAKE_MONEY
CMSG_MEETINGSTONE_INFO
CMSG_MEETINGSTONE_JOIN
CMSG_MEETINGSTONE_LEAVE
CMSG_MESSAGECHAT
CMSG_MOUNTSPECIAL_ANIM
CMSG_MOVE_FALL_RESET
CMSG_MOVE_FEATHER_FALL_ACK
CMSG_MOVE_HOVER_ACK
CMSG_MOVE_KNOCK_BACK_ACK
CMSG_MOVE_NOT_ACTIVE_MOVER
CMSG_MOVE_SET_RAW_POSITION
CMSG_MOVE_SPLINE_DONE
CMSG_MOVE_TIME_SKIPPED
CMSG_MOVE_WATER_WALK_ACK
CMSG_NEXT_CINEMATIC_CAMERA
CMSG_NPC_TEXT_QUERY
CMSG_OFFER_PETITION
CMSG_OPEN_ITEM
CMSG_PAGE_TEXT_QUERY
CMSG_PETITION_BUY
CMSG_PETITION_QUERY
CMSG_PETITION_SHOWLIST
CMSG_PETITION_SHOW_SIGNATURES
CMSG_PETITION_SIGN
CMSG_PET_ABANDON
CMSG_PET_CANCEL_AURA
CMSG_PET_CAST_SPELL
CMSG_PET_SPELL_AUTOCAST
CMSG_PET_STOP_ATTACK
CMSG_PET_UNLEARN
CMSG_PLAYED_TIME
CMSG_PLAYER_LOGIN
CMSG_PLAYER_LOGOUT
CMSG_PUSHQUESTTOPARTY
CMSG_QUERY_TIME
CMSG_QUESTGIVER_CANCEL
CMSG_QUESTGIVER_COMPLETE_QUEST
CMSG_QUESTGIVER_HELLO
CMSG_QUESTGIVER_QUERY_QUEST
CMSG_QUESTGIVER_QUEST_AUTOLAUNCH
CMSG_QUESTGIVER_REQUEST_REWARD
CMSG_QUESTGIVER_STATUS_QUERY
CMSG_QUESTLOG_REMOVE_QUEST
CMSG_QUESTLOG_SWAP_QUEST
CMSG_QUEST_CONFIRM_ACCEPT
CMSG_QUEST_QUERY
CMSG_READ_ITEM
CMSG_RECLAIM_CORPSE
CMSG_REPAIR_ITEM
CMSG_REPOP_REQUEST
CMSG_REQUEST_ACCOUNT_DATA
CMSG_REQUEST_PARTY_MEMBER_STATS
CMSG_REQUEST_PET_INFO
CMSG_REQUEST_RAID_INFO
CMSG_RESET_INSTANCES
CMSG_RESURRECT_RESPONSE
CMSG_SELF_RES
CMSG_SELL_ITEM
CMSG_SEND_MAIL
CMSG_SETSHEATHED
CMSG_SET_ACTIONBAR_TOGGLES
CMSG_SET_ACTION_BUTTON
CMSG_SET_ACTIVE_MOVER
CMSG_SET_AMMO
CMSG_SET_FACTION_ATWAR
CMSG_SET_FACTION_INACTIVE
CMSG_SET_SELECTION
CMSG_SET_TARGET_OBSOLETE
CMSG_SET_TRADE_GOLD
CMSG_SET_TRADE_ITEM
CMSG_SET_WATCHED_FACTION
CMSG_SPIRIT_HEALER_ACTIVATE
CMSG_SPLIT_ITEM
CMSG_STABLE_PET
CMSG_STABLE_SWAP_PET
CMSG_STANDSTATECHANGE
CMSG_SUMMON_RESPONSE
CMSG_SWAP_INV_ITEM
CMSG_SWAP_ITEM
CMSG_TELEPORT_TO_UNIT
CMSG_TEXT_EMOTE
CMSG_TOGGLE_CLOAK
CMSG_TOGGLE_HELM
CMSG_TOGGLE_PVP
CMSG_TRAINER_BUY_SPELL
CMSG_TRAINER_LIST
CMSG_TURN_IN_PETITION
CMSG_TUTORIAL_CLEAR
CMSG_TUTORIAL_FLAG
CMSG_TUTORIAL_RESET
CMSG_UNACCEPT_TRADE
CMSG_UNLEARN_SKILL
CMSG_UNSTABLE_PET
CMSG_UPDATE_ACCOUNT_DATA
CMSG_USE_ITEM
CMSG_WARDEN_DATA
CMSG_WHO
CMSG_WHOIS
CMSG_WORLD_TELEPORT
CMSG_WRAP_ITEM
CMSG_ZONEUPDATE
MSG_AUCTION_HELLO
MSG_BATTLEGROUND_PLAYER_POSITIONS
MSG_CORPSE_QUERY
MSG_INSPECT_HONOR_STATS
MSG_LIST_STABLED_PETS
MSG_LOOKING_FOR_GROUP
MSG_MINIMAP_PING
MSG_MOVE_FALL_LAND
MSG_MOVE_JUMP
MSG_MOVE_SET_FACING
MSG_MOVE_SET_PITCH
MSG_MOVE_SET_RUN_MODE
MSG_MOVE_SET_WALK_MODE
MSG_MOVE_START_BACKWARD
MSG_MOVE_START_PITCH_DOWN
MSG_MOVE_START_PITCH_UP
MSG_MOVE_START_STRAFE_LEFT
MSG_MOVE_START_STRAFE_RIGHT
MSG_MOVE_START_SWIM
MSG_MOVE_START_TURN_LEFT
MSG_MOVE_START_TURN_RIGHT
MSG_MOVE_STOP
MSG_MOVE_STOP_PITCH
MSG_MOVE_STOP_STRAFE
MSG_MOVE_STOP_SWIM
MSG_MOVE_STOP_TURN
MSG_MOVE_WATER_WALK
MSG_PETITION_DECLINE
MSG_PETITION_RENAME
MSG_PVP_LOG_DATA
MSG_QUERY_NEXT_MAIL_TIME
MSG_QUEST_PUSH_RESULT
MSG_RAID_READY_CHECK
MSG_RAID_TARGET_UPDATE
MSG_RANDOM_ROLL
MSG_SAVE_GUILD_EMBLEM
MSG_TABARDVENDOR_ACTIVATE
MSG_TALENT_WIPE_CONFIRM
";
const INITIAL_OUT_OF_SCOPE: &[&str] = &[];
const EXCLUSIONS: &str = "conformance/opcodes-out-of-scope.toml";
const ALLOWLIST: &str = "conformance/opcodes-unclaimed.txt";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exclusions {
    out_of_scope: BTreeMap<String, Exclusion>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Exclusion {
    reason: String,
    source: Option<String>,
}

fn scope_source(root: &Path, opcode: &str, source: &str) -> bool {
    let path = Path::new(source);
    if source.starts_with("docs/adr/")
        && source.ends_with(".md")
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return std::fs::read_to_string(root.join(path)).is_ok_and(|text| {
            text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|word| word == opcode)
        });
    }
    let Some(reference) = source.strip_prefix("https://github.com/vmangos/core/blob/") else {
        return false;
    };
    let Some((sha, path)) = reference.split_once('/') else {
        return false;
    };
    wire_client::catalogue::ENTRIES.iter().all(|entry| {
        wire_client::catalogue::get(entry.id).is_ok_and(|golden| {
            matches!(&golden.trace().header.origin, wire_client::trace::Origin::Golden { provenance } if provenance.source_commit == sha)
        })
    })
        && path.starts_with("src/")
        && Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

fn scope_additions_have_sources(
    root: &Path,
    current: &BTreeMap<String, Exclusion>,
    previous: &BTreeMap<String, Exclusion>,
) -> Result<()> {
    for (name, entry) in current {
        if let Some(source) = &entry.source {
            ensure!(
                scope_source(root, name, source),
                "{name}: invalid scope source {source}"
            );
        }
        ensure!(
            previous.contains_key(name)
                || entry
                    .source
                    .as_ref()
                    .is_some_and(|source| scope_source(root, name, source)),
            "new out-of-scope opcode {name} needs an ADR or vmangos source at a pinned SHA"
        );
    }
    Ok(())
}

fn allowlist(source: &str) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for name in source.lines() {
        ensure!(
            !name.is_empty() && name.trim() == name,
            "invalid allowlist entry {name:?}"
        );
        ensure!(
            names.insert(name.into()),
            "duplicate allowlist entry {name}"
        );
    }
    Ok(names)
}

fn covers(
    variants: &BTreeMap<String, u16>,
    claimed: &BTreeSet<u16>,
    excluded: &BTreeMap<String, Exclusion>,
    unclaimed: &BTreeSet<String>,
) -> Result<()> {
    for (name, entry) in excluded {
        ensure!(
            variants.contains_key(name),
            "unknown out-of-scope opcode {name}"
        );
        ensure!(
            !entry.reason.trim().is_empty(),
            "{name} needs an out-of-scope reason"
        );
        ensure!(
            !claimed.contains(&variants[name]),
            "{name} is both claimed and out of scope"
        );
    }
    for name in unclaimed {
        ensure!(
            variants.contains_key(name),
            "unknown allowlisted opcode {name}"
        );
        ensure!(
            !claimed.contains(&variants[name]) && !excluded.contains_key(name),
            "remove {name} from the allowlist: it is claimed or out of scope"
        );
    }
    let missing: Vec<_> = variants
        .iter()
        .filter(|(name, opcode)| {
            !claimed.contains(opcode) && !excluded.contains_key(*name) && !unclaimed.contains(*name)
        })
        .map(|(name, _)| name)
        .collect();
    ensure!(
        missing.is_empty(),
        "opcodes need a Behavior or out-of-scope reason: {missing:?}"
    );
    Ok(())
}

fn only_shrinks(current: &BTreeSet<String>, previous: &BTreeSet<String>) -> Result<()> {
    let added: Vec<_> = current.difference(previous).collect();
    ensure!(added.is_empty(), "opcode allowlist grew: {added:?}");
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE")
        .args(args)
        .output()?;
    ensure!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

fn history(root: &Path, path: &str) -> Result<Vec<String>> {
    ensure!(
        matches!(path, ALLOWLIST | EXCLUSIONS),
        "opcode scope paths cannot be renamed"
    );
    ensure!(
        git(root, &["rev-parse", "--is-shallow-repository"])?.trim() == "false",
        "opcode allowance history requires a full checkout; run git fetch --unshallow"
    );
    let renames = git(
        root,
        &[
            "log",
            "--follow",
            "--find-renames",
            "--diff-filter=R",
            "--format=",
            "--name-status",
            "HEAD",
            "--",
            path,
        ],
    )?;
    ensure!(
        renames.trim().is_empty(),
        "opcode scope files cannot be renamed to reset history: {path}"
    );
    let commits = git(
        root,
        &[
            "log",
            "--full-history",
            "--diff-filter=AM",
            "--format=%H",
            "HEAD",
            "--",
            path,
        ],
    )?;
    ensure!(
        !commits.trim().is_empty(),
        "missing opcode scope history: {path}"
    );
    commits
        .lines()
        .map(|commit| git(root, &["show", &format!("{commit}:{path}")]))
        .collect()
}

#[test]
fn every_client_request_has_a_behavior_scope_decision_or_remaining_allowance() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let variants = opcodes::read_inventory(root)?;
    for (name, value) in opcodes::read(root)? {
        ensure!(
            variants.get(&name) == Some(&value),
            "core opcode differs from build 5875: {name}"
        );
    }
    let ledger = FeatureLedger::load(&root.join("conformance/features"))?;
    let claimed = ledger
        .behaviors()
        .iter()
        .flat_map(|behavior| &behavior.requests)
        .map(|opcode| u16::from_str_radix(&opcode[2..], 16))
        .collect::<std::result::Result<_, _>>()?;
    let excluded: Exclusions = toml::from_str(&std::fs::read_to_string(root.join(EXCLUSIONS))?)?;
    let unclaimed = allowlist(&std::fs::read_to_string(root.join(ALLOWLIST))?)?;
    covers(&variants, &claimed, &excluded.out_of_scope, &unclaimed)?;
    only_shrinks(&unclaimed, &allowlist(INITIAL_UNCLAIMED)?)?;
    let initial = INITIAL_OUT_OF_SCOPE
        .iter()
        .map(|name| (name.to_string(), exclusion("Initial scope")))
        .collect();
    scope_additions_have_sources(root, &excluded.out_of_scope, &initial)?;
    for source in history(root, ALLOWLIST)? {
        only_shrinks(&unclaimed, &allowlist(&source)?)?;
    }
    for source in history(root, EXCLUSIONS)? {
        let previous: Exclusions = toml::from_str(&source)?;
        scope_additions_have_sources(root, &excluded.out_of_scope, &previous.out_of_scope)?;
    }
    println!(
        "{} client requests: {} claimed, {} out of scope, {} allowlisted",
        variants.len(),
        variants
            .values()
            .filter(|opcode| claimed.contains(opcode))
            .count(),
        excluded.out_of_scope.len(),
        unclaimed.len()
    );
    Ok(())
}

#[test]
fn coverage_detects_missing_claims_stale_allowances_and_invalid_exclusions() {
    let variants = [("CMSG_ONE".into(), 1), ("CMSG_TWO".into(), 2)].into();
    let claimed = [1].into();
    let unclaimed = ["CMSG_TWO".into()].into();
    assert!(covers(&variants, &claimed, &BTreeMap::new(), &unclaimed).is_ok());
    assert!(covers(&variants, &BTreeSet::new(), &BTreeMap::new(), &unclaimed).is_err());
    assert!(covers(&variants, &[1, 2].into(), &BTreeMap::new(), &unclaimed).is_err());
    let excluded = [("CMSG_TWO".into(), exclusion("Tool-only request"))].into();
    assert!(covers(&variants, &claimed, &excluded, &unclaimed).is_err());
    assert!(covers(&variants, &claimed, &excluded, &BTreeSet::new()).is_ok());
    for exclusions in [
        [("CMSG_TWO".into(), exclusion(" "))].into(),
        [("CMSG_UNKNOWN".into(), exclusion("Tool-only"))].into(),
        [("CMSG_ONE".into(), exclusion("Tool-only"))].into(),
    ] {
        assert!(covers(&variants, &claimed, &exclusions, &unclaimed).is_err());
    }
    assert!(covers(
        &variants,
        &claimed,
        &BTreeMap::new(),
        &["CMSG_UNKNOWN".into()].into()
    )
    .is_err());
    assert!(allowlist("CMSG_ONE\nCMSG_ONE\n").is_err());
}

#[test]
fn allowances_cannot_grow_or_exchange_a_removed_opcode_for_another() {
    let previous = allowlist("CMSG_ONE\nCMSG_TWO\n").unwrap();
    assert!(only_shrinks(&allowlist("CMSG_ONE\n").unwrap(), &previous).is_ok());
    assert!(only_shrinks(&previous, &previous).is_ok());
    for current in ["CMSG_ONE\nCMSG_TWO\nCMSG_THREE\n", "CMSG_ONE\nCMSG_THREE\n"] {
        assert!(only_shrinks(&allowlist(current).unwrap(), &previous).is_err());
    }
    assert!(only_shrinks(&previous, &allowlist("CMSG_ONE\n").unwrap()).is_err());
}

fn exclusion(reason: &str) -> Exclusion {
    Exclusion {
        reason: reason.into(),
        source: None,
    }
}

#[test]
fn scope_additions_require_a_source_even_after_removal() -> Result<()> {
    let directory = scratch::Scratch::new("scope-source")?;
    let root = &*directory;
    std::fs::create_dir_all(root.join("docs/adr"))?;
    std::fs::write(
        root.join("docs/adr/scope.md"),
        "CMSG_ONE is a tool-only opcode.",
    )?;
    std::fs::write(
        root.join("docs/adr/unrelated.md"),
        "CMSG_ONE_MORE does not name the opcode.",
    )?;
    let mut current = BTreeMap::from([("CMSG_ONE".into(), exclusion("Tool-only"))]);
    assert!(scope_additions_have_sources(root, &current, &BTreeMap::new()).is_err());
    assert!(scope_additions_have_sources(root, &current, &current).is_ok());
    for source in ["docs/adr/scope.md",
        "https://github.com/vmangos/core/blob/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Server/Opcodes.cpp"] {
        current.get_mut("CMSG_ONE").unwrap().source = Some(source.into());
        assert!(scope_additions_have_sources(root, &current, &BTreeMap::new()).is_ok());
    }
    for source in [
        "docs/adr/missing.md",
        "docs/adr/unrelated.md",
        "https://github.com/vmangos/core/blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/src/game/Server/Opcodes.cpp",
        "docs/adr/../../Cargo.toml",
        "because",
        "https://github.com/vmangos/core/blob/main/src/game/Server/Opcodes.cpp",
    ] {
        current.get_mut("CMSG_ONE").unwrap().source = Some(source.into());
        assert!(scope_additions_have_sources(root, &current, &BTreeMap::new()).is_err());
    }
    Ok(())
}

#[test]
fn renaming_or_recreating_scope_files_does_not_reset_history() -> Result<()> {
    let root = scratch::Scratch::new("history-test")?;
    std::fs::create_dir_all(root.join("conformance"))?;
    git(&root, &["init", "--quiet"])?;
    let commit = || -> Result<()> {
        git(&root, &["add", "."])?;
        let result = Command::new("git")
            .current_dir(&*root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .args([
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ])
            .output()?;
        ensure!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        Ok(())
    };
    std::fs::write(root.join(ALLOWLIST), "CMSG_ONE\nCMSG_TWO\n")?;
    std::fs::write(root.join(EXCLUSIONS), "[out_of_scope]\n")?;
    commit()?;
    assert_eq!(history(&root, ALLOWLIST)?.len(), 1);
    std::fs::write(root.join(ALLOWLIST), "CMSG_ONE\n")?;
    commit()?;
    std::fs::remove_file(root.join(ALLOWLIST))?;
    commit()?;
    std::fs::write(root.join(ALLOWLIST), "CMSG_ONE\nCMSG_TWO\n")?;
    commit()?;
    let current = allowlist(&std::fs::read_to_string(root.join(ALLOWLIST))?)?;
    assert!(history(&root, ALLOWLIST)?.iter().any(|old| only_shrinks(
        &current,
        &allowlist(old).unwrap()
    )
    .is_err()));
    for path in [ALLOWLIST, EXCLUSIONS] {
        let renamed = format!("{path}.renamed");
        git(&root, &["mv", path, &renamed])?;
        std::fs::write(
            root.join(&renamed),
            "Entirely new contents hide the rename from Git.\n",
        )?;
        commit()?;
        assert!(history(&root, &renamed).is_err());
    }
    assert!(history(&root, "unknown.txt").is_err());
    Ok(())
}
