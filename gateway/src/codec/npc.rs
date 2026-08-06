//! NPC wire mapping: the `CMSG_CREATURE_QUERY` reply and the minimal generic gossip window
//! (gossip message + NPC-text round-trip). Pure code-motion out of `mod.rs`.

use super::*;

/// A creature template as the gateway reads it from `game_creature_template`, flattened for the
/// `CMSG_CREATURE_QUERY` reply (Tier 2 / NPCs). Decoupled from the module's table type.
#[derive(Clone, Debug, Default)]
pub struct CreatureView {
    pub entry: u32,
    pub name: String,
    pub subname: String,
    pub display_id: u32,
    pub creature_type: u32,
    pub creature_family: u8,
    pub type_flags: u32,
    pub rank: u32,
}

/// Build `SMSG_CREATURE_QUERY_RESPONSE` so the client renders the creature's name/subname instead
/// of "Unknown" (the creature analogue of the player name query).
pub fn build_creature_query_response(c: &CreatureView) -> SMSG_CREATURE_QUERY_RESPONSE {
    SMSG_CREATURE_QUERY_RESPONSE {
        creature_entry: c.entry,
        found: Some(SMSG_CREATURE_QUERY_RESPONSE_found {
            name1: c.name.clone(),
            name2: String::new(),
            name3: String::new(),
            name4: String::new(),
            sub_name: c.subname.clone(),
            type_flags: c.type_flags,
            creature_type: c.creature_type,
            creature_family: CreatureFamily::try_from(c.creature_family).unwrap_or_default(),
            creature_rank: c.rank,
            unknown0: 0,
            spell_data_id: 0,
            display_id: c.display_id,
            civilian: 0,
            racial_leader: 0,
        }),
    }
}

// ===========================================================================================
//  Gossip (rank 12, extended by work-item 217): the gateway answers CMSG_GOSSIP_HELLO with a greeting
//  window (title resolved via the CMSG_NPC_TEXT_QUERY round-trip) + either the NPC's IMPORTED menu
//  options (`game_gossip_option`, precedence) or the flag-derived vendor/innkeeper/Farewell synthesis
//  (fallback, byte-identical to the pre-217 behavior) — never both.
// ===========================================================================================

/// One imported gossip menu option (`game_gossip_option`, work-item 217), as the gateway reads it —
/// already sorted by `option_index` (the render/select order). Carries the RAW condition so the
/// dispatcher can filter with [`option_condition_holds`] (the codec stays pure; the store call for
/// quest status lives in `gateway::world`'s dispatch).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipOptionView {
    pub row_id: u32, // game_gossip_option PK — the STABLE option identity (283)
    pub icon: u32,
    pub text: String,
    pub action: u32,         // see `lyracore_shared::constants::gossip_option`
    pub action_menu_id: u32, // submenu target — stored, never navigated (217 scope)
    pub cond_type: u32,      // see `lyracore_shared::constants::gossip_condition`
    pub cond_value1: u32,
    pub cond_value2: u32,
}

/// Evaluate one option's visibility condition against the player's quest state — PURE (no store
/// access), so the dispatcher can call it identically at `CMSG_GOSSIP_HELLO` (render) and
/// `CMSG_GOSSIP_SELECT_OPTION` (re-derive the click) without risking the two disagreeing.
/// `quest_taken`/`quest_rewarded` are `stdb::quest_status(guid, cond_value1)` — a quest never seen by
/// the player is `(false, false)`. An unrecognized `cond_type` fails OPEN (shown), matching the
/// importer's own fail-open convention for a condition it can't classify.
pub fn option_condition_holds(cond_type: u32, quest_taken: bool, quest_rewarded: bool) -> bool {
    use lyracore_shared::constants::gossip_condition;
    match cond_type {
        gossip_condition::NONE => true,
        gossip_condition::QUEST_TAKEN => quest_taken,
        gossip_condition::QUEST_REWARDED => quest_rewarded,
        _ => true, // unrecognized → fail open, never silently hide an option
    }
}

/// The generic title text id sent in `SMSG_GOSSIP_MESSAGE`; the client round-trips it via
/// `CMSG_NPC_TEXT_QUERY`, which `build_npc_text_update` answers with the generic greeting.
///
/// Kept a LOW, in-range npc_text id. The first `u32` after the guid in `SMSG_GOSSIP_MESSAGE` is an
/// npc_text (gossip-menu text) id, not a free-form title field. SUSPECTED CAUSE (not yet client-
/// confirmed) of the McBride gossip-window bug: a synthetic high id (the old `0xFFFF_FF01`, top byte
/// `0xFF`) appears to be rejected as out-of-range by the 1.12.1 (5875) client, which then drops the
/// gossip packet (no `GOSSIP_SHOW`, window never opens). The headless wire-probe shares the gtker codec
/// and does NO DBC/text validation, so it decoded the packet fine — it can't reproduce the real
/// client's reject, so this stays UNVERIFIED until the render is observed (right-click McBride →
/// GOSSIP_SHOW). `build_npc_text_update` answers ANY id generically, so a small id (`1`) the client
/// accepts + queries is the candidate fix; the wire BYTES are unchanged, only this value. If the window
/// still won't open, the next suspect is the questgiver/gossip menu-path race, not this id.
pub const GOSSIP_GREETING_TEXT_ID: u32 = 1;

/// The generic greeting shown in the gossip window (until per-NPC text is imported).
const GOSSIP_GREETING: &str = "Greetings, traveler. I have nothing for you at this time.";

/// Build the `SMSG_GOSSIP_MESSAGE` reply to `CMSG_GOSSIP_HELLO`: echo the NPC `guid`, the per-NPC
/// title text id (from `game_gossip_menu` → fallback `GOSSIP_GREETING_TEXT_ID = 1`; the client
/// resolves it via `CMSG_NPC_TEXT_QUERY` → `build_npc_text_update`), and the `quests` section. A
/// gossip-FLAGGED questgiver (e.g. Marshal McBride) delivers its quests HERE. `quests` is empty for a
/// plain gossip NPC → byte-identical to before for those.
///
/// `imported` is the ALREADY-FILTERED (by the dispatcher, via [`option_condition_holds`]) option list
/// from `game_gossip_option`, in `option_index` order — work-item 217. `Some(nonempty)` → those
/// options render VERBATIM (a trailing "Farewell." is appended so every menu still has a close
/// option), taking full precedence over the flag-derived synthesis below. `None`/empty (nothing
/// imported for this creature) → today's fallback: synthesize "browse goods" (vendor, stock
/// presence) + "Make this inn your home." (innkeeper, npc_flags) + "Farewell.", BYTE-IDENTICAL to
/// the pre-217 behavior (see `codec::tests::gossip_message_fallback_is_byte_identical_to_pre_217`).
pub fn build_gossip_message(
    npc_guid: u64,
    title_text_id: u32,
    quests: Vec<QuestItem>,
    imported: Option<&[GossipOptionView]>,
    is_vendor: bool,
    is_innkeeper: bool,
) -> SMSG_GOSSIP_MESSAGE {
    // The `gossip_list_id` the client echoes in CMSG_GOSSIP_SELECT_OPTION is the option's POSITION —
    // the dispatcher re-derives the SAME order (imported: re-filter identically; fallback:
    // is_vendor/is_innkeeper) to recognise the pick.
    let gossips = match imported {
        Some(opts) if !opts.is_empty() => {
            let mut gossips: Vec<GossipItem> = opts
                .iter()
                .enumerate()
                .map(|(i, o)| GossipItem {
                    id: i as u32,
                    item_icon: o.icon as u8,
                    coded: false,
                    message: o.text.clone(),
                })
                .collect();
            gossips.push(GossipItem {
                id: gossips.len() as u32,
                item_icon: 0,
                coded: false,
                message: "Farewell.".to_string(),
            });
            gossips
        }
        _ => {
            // Fallback (pre-217, byte-identical): browse-goods (vendor), make-home (innkeeper), then
            // Farewell. A plain gossip NPC shows only Farewell.
            let mut gossips = Vec::new();
            if is_vendor {
                gossips.push(GossipItem {
                    id: gossips.len() as u32, // == GOSSIP_OPTION_VENDOR (0): vendor is always first when present
                    item_icon: 1,             // GOSSIP_ICON_VENDOR (the bag icon)
                    coded: false,
                    message: "I'd like to browse your goods.".to_string(),
                });
            }
            if is_innkeeper {
                gossips.push(GossipItem {
                    id: gossips.len() as u32,
                    item_icon: 0, // GOSSIP_ICON_CHAT (vanilla's innkeeper-bind option icon)
                    coded: false,
                    message: "Make this inn your home.".to_string(),
                });
            }
            gossips.push(GossipItem {
                id: gossips.len() as u32,
                item_icon: 0,
                coded: false,
                message: "Farewell.".to_string(),
            });
            gossips
        }
    };
    SMSG_GOSSIP_MESSAGE {
        guid: Guid::new(npc_guid),
        title_text_id,
        gossips,
        quests,
    }
}

/// The gossip menu index of the "browse goods" option — always slot 0 when present, so the
/// `CMSG_GOSSIP_SELECT_OPTION` dispatcher can recognize the vendor pick (`gossip_list_id == 0` on a
/// stocked NPC) and reply with the inventory window.
pub const GOSSIP_OPTION_VENDOR: u32 = 0;

/// The gossip menu index of the innkeeper "Make this inn your home." option — it follows the vendor
/// option, so it's slot 1 on an NPC that's also a vendor, else slot 0. The dispatcher re-derives this to
/// recognise the bind pick (matching the push order in `build_gossip_message`).
pub const fn gossip_option_innkeeper(is_vendor: bool) -> u32 {
    if is_vendor {
        1
    } else {
        0
    }
}

/// The full 8-slot weighted `npc_text` row (work-item 217), as the gateway reads it: each slot is
/// `(male, female, probability)`. `SMSG_NPC_TEXT_UPDATE` ships all 8 to the client, which does its OWN
/// weighted random pick — there is no server-side RNG here (see `build_npc_text_update`). An unused
/// slot is `("", "", 0.0)`. `stdb::npc_text_for_id` normalizes the pre-217 "legacy single-slot" case
/// (a row imported before the multi-slot table existed) so slot 0's probability reads `1.0` rather
/// than the raw `0.0` default — see that method's doc comment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NpcTextView {
    pub slots: [(String, String, f32); 8],
}

/// Build the `SMSG_NPC_TEXT_UPDATE` reply to `CMSG_NPC_TEXT_QUERY`. `Some(view)` (an imported row —
/// `stdb::npc_text_for_id`) ships its 8 real weighted slots verbatim; `None` (no row for this
/// `text_id`) falls back to the generic greeting string in slot 0 at probability 1.0, every other slot
/// silent — byte-identical to the pre-217 behavior.
pub fn build_npc_text_update(text_id: u32, view: Option<&NpcTextView>) -> SMSG_NPC_TEXT_UPDATE {
    let texts: [NpcTextUpdate; 8] = match view {
        Some(v) => core::array::from_fn(|i| {
            let (male, female, prob) = &v.slots[i];
            NpcTextUpdate {
                probability: *prob,
                texts: [male.clone(), female.clone()],
                ..NpcTextUpdate::default()
            }
        }),
        None => {
            let mut texts: [NpcTextUpdate; 8] = core::array::from_fn(|_| NpcTextUpdate::default());
            texts[0] = NpcTextUpdate {
                probability: 1.0,
                texts: [GOSSIP_GREETING.to_string(), GOSSIP_GREETING.to_string()],
                ..NpcTextUpdate::default()
            };
            texts
        }
    };
    SMSG_NPC_TEXT_UPDATE { text_id, texts }
}

// ===========================================================================================
//  Vendor (Tier 2) — the vendor inventory window. `SMSG_LIST_INVENTORY` is built RAW because the
//  vanilla (5875) layout is NOT in the gtker crate: the typed `SMSG_LIST_INVENTORY` is tbc/wrath
//  and carries an extra `ExtendedCost` u32 per item the 1.12 client doesn't read. The BUY/SELL
//  client opcodes ARE in the vanilla crate (CMSG_BUY_ITEM / CMSG_SELL_ITEM), handled in dispatch.
// ===========================================================================================

/// `SMSG_LIST_INVENTORY` opcode (vanilla 5875). RAW-encoded — see [`build_list_inventory_raw`].
pub const SMSG_LIST_INVENTORY_OPCODE: u16 = 0x019F;

/// One vendor stock line as the gateway reads it (the `game_npc_vendor` row joined with its
/// `game_item_template`), flattened for the RAW `SMSG_LIST_INVENTORY` body. `max_count` is the
/// per-slot stock limit (0 → unlimited, written as 0xFFFF_FFFF on the wire).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VendorItemView {
    pub item_entry: u32,
    pub display_id: u32,
    pub buy_price: u32,
    pub max_durability: u32,
    pub max_count: u32,
    /// The stack sold per purchase (080, cmangos BuyCount): water/food ×5, ammo ×200. ≥1.
    pub buy_count: u32,
}

/// Build a RAW `SMSG_LIST_INVENTORY` (vanilla 5875). RAW because gtker's typed message is the
/// tbc/wrath shape (an extra `ExtendedCost` u32 per item) the 1.12 client doesn't parse. The
/// vanilla body is (all little-endian):
///   - `vendor_guid: u64` (full, not packed)
///   - `count: u8` (= `items.len()`)
///   - per item, in `slot` order: `muid: u32` (1-based index), `item_entry: u32`,
///     `display_id: u32`, `max_count: u32` (0 stored → 0xFFFF_FFFF unlimited), `buy_price: u32`,
///     `max_durability: u32`, `buy_count: u32` (080: the template's per-purchase stack).
///
/// An empty stock is just the guid + a zero count. Returns `(opcode, body)` for
/// [`Outbound::Raw`](crate::world::Outbound::Raw).
pub fn build_list_inventory_raw(vendor_guid: u64, items: &[VendorItemView]) -> (u16, Vec<u8>) {
    // The wire count is a u8; cap the slice so the count and the item bytes can never disagree (a vanilla
    // vendor is well under 255 — this is a guard against a silent `as u8` wrap, not a real limit).
    let items = &items[..items.len().min(255)];
    let mut body = Vec::with_capacity(9 + items.len() * 28);
    body.extend_from_slice(&vendor_guid.to_le_bytes()); // vendor guid: u64 (full, not packed)
    body.push(items.len() as u8); // count: u8
    for (i, it) in items.iter().enumerate() {
        body.extend_from_slice(&((i as u32) + 1).to_le_bytes()); // muid: 1-based slot index
        body.extend_from_slice(&it.item_entry.to_le_bytes()); // item entry: u32
        body.extend_from_slice(&it.display_id.to_le_bytes()); // ItemDisplayInfo id: u32
                                                              // max_count: 0 (no stored limit) → 0xFFFF_FFFF = unlimited, matching mangos/cmangos.
        let max_count = if it.max_count == 0 {
            0xFFFF_FFFF
        } else {
            it.max_count
        };
        body.extend_from_slice(&max_count.to_le_bytes()); // max stock: u32
        body.extend_from_slice(&it.buy_price.to_le_bytes()); // buy price (copper): u32
        body.extend_from_slice(&it.max_durability.to_le_bytes()); // max durability: u32
        body.extend_from_slice(&it.buy_count.max(1).to_le_bytes()); // buy_count: u32 (080)
    }
    (SMSG_LIST_INVENTORY_OPCODE, body)
}
