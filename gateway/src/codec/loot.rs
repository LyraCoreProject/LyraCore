//! Loot wire mapping: the RAW `SMSG_LOOT_RESPONSE` (gtker's typed `LootItem` is incomplete for
//! 1.12), the money-notify, and the loot-release ack. Pure code-motion out of `mod.rs`.

use super::*;
use lyracore_shared::loot_roll::vote_kind;

/// `SMSG_LOOT_RESPONSE` opcode (vanilla 5875). Pinned against gtker in the byte-match test below.
const SMSG_LOOT_RESPONSE_OPCODE: u16 = 0x0160;

/// One lootable item for the loot window: `(slot, item_id, count, display_id)`. `display_id` is the
/// item's `ItemDisplayInfo` id (the gateway joins it from `game_item_template`); the client also
/// queries the item (`CMSG_ITEM_QUERY_SINGLE`) for the name/tooltip (slice-1 path).
pub type LootItemView = (u8, u32, u32, u32);

/// Build a RAW `SMSG_LOOT_RESPONSE` (slice 3 money + slice 4 items). RAW because gtker's typed
/// `LootItem` is INCOMPLETE for vanilla 1.12 — it encodes only `index:u8 + item:u32 + ty:u8` (6
/// bytes), omitting the `count`/`display_id`/`random_suffix`/`random_property_id` (u32×4) the 5875
/// client reads per item (mangos sends 22 bytes/item). Sending items through the typed builder would
/// desync the client's parse. The ENVELOPE (guid u64, loot_method u8, gold u32, count u8) is
/// byte-identical to gtker's working money-only message — pinned by `loot_response_envelope_matches_gtker`
/// — so we reuse it verbatim and only hand-roll the per-item bytes in the correct vanilla layout.
/// Returns `(opcode, body)` for [`Outbound::Raw`](crate::world::Outbound::Raw).
pub fn build_loot_response_raw(guid: u64, money: u32, items: &[LootItemView]) -> (u16, Vec<u8>) {
    // The wire count is a u8; cap the slice so the count and the item bytes can never disagree (a corpse
    // holds far fewer than 255 — this is a guard against a silent `as u8` wrap, not a real limit).
    let items = &items[..items.len().min(255)];
    let mut body = Vec::with_capacity(14 + items.len() * 22);
    body.extend_from_slice(&guid.to_le_bytes()); // guid: u64 (full, not packed)
    body.push(1u8); // loot_method = Corpse (SMSG_LOOT_RESPONSE_LootMethod::Corpse as_int)
    body.extend_from_slice(&money.to_le_bytes()); // gold: u32
    body.push(items.len() as u8); // amount_of_items: u8
    for &(slot, item_id, count, display_id) in items {
        body.push(slot); // loot slot index: u8
        body.extend_from_slice(&item_id.to_le_bytes()); // item id (entry): u32
        body.extend_from_slice(&count.to_le_bytes()); // stack count: u32
        body.extend_from_slice(&display_id.to_le_bytes()); // ItemDisplayInfo id: u32
        body.extend_from_slice(&0u32.to_le_bytes()); // random_suffix: u32 (none)
        body.extend_from_slice(&0u32.to_le_bytes()); // random_property_id: u32 (none)
        body.push(0u8); // slot_type = TypeAllowLoot (as_int 0)
    }
    (SMSG_LOOT_RESPONSE_OPCODE, body)
}

/// Build `SMSG_LOOT_REMOVED` — clears one looted slot from the open loot window after its item is
/// taken (`CMSG_AUTOSTORE_LOOT_ITEM`), so the slot disappears without re-opening the window.
pub fn build_loot_removed(slot: u8) -> SMSG_LOOT_REMOVED {
    SMSG_LOOT_REMOVED { slot }
}

/// Build `SMSG_LOOT_MONEY_NOTIFY` — tells the client how much coin was just looted (slice 3).
pub fn build_loot_money_notify(amount: u32) -> SMSG_LOOT_MONEY_NOTIFY {
    SMSG_LOOT_MONEY_NOTIFY { amount }
}

/// Build `SMSG_LOOT_RELEASE_RESPONSE` — acks closing the loot window (slice 3). `unknown1 = 1` per
/// the reference emulators (mangos/cmangos/vmangos).
pub fn build_loot_release_response(guid: u64) -> SMSG_LOOT_RELEASE_RESPONSE {
    SMSG_LOOT_RELEASE_RESPONSE {
        guid: Guid::new(guid),
        unknown1: 1,
    }
}

// ===========================================================================================
//  GROUP LOOT METHODS (work-item 187 slices 2-4) — need/greed rolls + master looter. All four
//  messages below are gtker-TYPED vanilla-complete builders (verified present in the vendored
//  `wow_world_messages` — no raw-build needed, unlike the money/item `SMSG_LOOT_RESPONSE` above).
// ===========================================================================================

/// Vote-kind on the wire (`RollVote`) from the module's `vote_kind::*` byte — the two orderings are
/// IDENTICAL by design (`lyracore_shared::loot_roll` doc), so this is a direct `try_from`, never a
/// hand-written match table that could silently drift.
fn wire_vote(vote: u8) -> RollVote {
    RollVote::try_from(vote).unwrap_or(RollVote::Pass)
}

/// Build `SMSG_LOOT_START_ROLL` — the 60s need/greed/pass countdown window opening for one item.
pub fn build_loot_start_roll(
    corpse_guid: u64,
    loot_slot: u8,
    item_entry: u32,
    countdown_ms: u32,
) -> SMSG_LOOT_START_ROLL {
    SMSG_LOOT_START_ROLL {
        creature: Guid::new(corpse_guid),
        loot_slot: loot_slot as u32,
        item: item_entry,
        item_random_suffix: 0,
        item_random_property_id: 0,
        countdown_time: std::time::Duration::from_millis(countdown_ms as u64),
    }
}

/// The wire `roll_number` for one vote: vanilla has NO separate `auto_pass` field (that's a
/// TBC/WRATH-only wire addition — `wow_world_messages::vanilla::SMSG_LOOT_ROLL` genuinely lacks it,
/// confirmed against the vendored crate) — a PASS (manual OR auto-at-deadline; the two are
/// indistinguishable on the vanilla wire, `[V]`, no live client to confirm the exact convention)
/// is instead signaled by `roll_number > 127` per the field's own doc comment ("> 127: you passed").
/// `128` is the value cmangos/vmangos use. A NEED/GREED vote's real 1-100 roll passes through
/// unchanged. Pure — unit-tested.
pub(crate) fn wire_roll_number(vote: u8, rolled: u8) -> u8 {
    if vote == vote_kind::PASS {
        128
    } else {
        rolled
    }
}

/// Build `SMSG_LOOT_ROLL` — one member's vote landing (relayed to every eligible member so live
/// votes/roll numbers are visible party-wide, matching vanilla). `auto_pass` from the module's event
/// payload is folded into the wire `roll_number` (see `wire_roll_number`) rather than carried as a
/// separate field — the vanilla message has none.
pub fn build_loot_roll(
    corpse_guid: u64,
    loot_slot: u8,
    voter_guid: u64,
    item_entry: u32,
    rolled: u8,
    vote: u8,
    _auto_pass: bool,
) -> SMSG_LOOT_ROLL {
    SMSG_LOOT_ROLL {
        creature: Guid::new(corpse_guid),
        loot_slot: loot_slot as u32,
        player: Guid::new(voter_guid),
        item: item_entry,
        item_random_suffix: 0,
        item_random_property_id: 0,
        roll_number: wire_roll_number(vote, rolled),
        vote: wire_vote(vote),
    }
}

/// Build `SMSG_LOOT_ROLL_WON` — the roll resolved to a winner.
pub fn build_loot_roll_won(
    corpse_guid: u64,
    loot_slot: u8,
    item_entry: u32,
    winning_player: u64,
    winning_roll: u8,
    winning_vote: u8,
) -> SMSG_LOOT_ROLL_WON {
    SMSG_LOOT_ROLL_WON {
        looted_target: Guid::new(corpse_guid),
        loot_slot: loot_slot as u32,
        item: item_entry,
        item_random_suffix: 0,
        item_random_property_id: 0,
        winning_player: Guid::new(winning_player),
        winning_roll,
        // The tier that actually won — a greed-only contest reports Greed, not Need (187 review
        // finding #3; the module sends vote_kind::*, already wire-order-matching).
        vote: wire_vote(winning_vote),
    }
}

/// Build `SMSG_LOOT_MASTER_LIST` — the master looter's eligible-recipient list.
pub fn build_loot_master_list(eligible: &[u64]) -> SMSG_LOOT_MASTER_LIST {
    SMSG_LOOT_MASTER_LIST {
        guids: eligible.iter().map(|&g| Guid::new(g)).collect(),
    }
}
