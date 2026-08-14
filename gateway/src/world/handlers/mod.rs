//! Per-domain dispatch handlers and the deeper action dispatchers that own a complete protocol
//! family, plus the couple of helpers shared by two handlers.

use super::*;

// Per-family dispatch handlers — code-motion of the former dispatch match arms (bodies verbatim,
// incl. in `handle_combat` the two session-fatal `is_desync_error` early-exits on ATTACKSWING/STOP).
// Each returns `Ok(None)` once it consumes its opcode, else `Ok(Some(msg))` to pass the message on.

mod bank;
mod cast;
mod char;
mod combat;
mod item;
mod loot;
mod mail;
mod query;
mod quest;
mod trade;
mod trainer;
mod vendor;

pub(crate) use bank::handle_bank;
pub(crate) use cast::{dispatch_cast, CastOutcome, CastPlayer, CastStore, CastTransition};
pub(crate) use char::handle_char;
pub(crate) use combat::handle_combat;
pub(crate) use item::{dispatch_item_action, ItemActionOutcome, ItemActionPlayer, ItemActionStore};
pub(crate) use loot::handle_loot;
pub(crate) use mail::handle_mail;
pub(crate) use query::handle_query;
pub(crate) use quest::handle_quest;
pub(crate) use trade::handle_trade;
pub(crate) use trainer::handle_trainer;
pub(crate) use vendor::handle_vendor;

/// Rebuild + push the buyback-tab view: a synthesized ITEM object per ring entry (fabricated
/// guid 0x4090…|slot — a client-only object, never a real instance) and ONE raw VALUES update
/// carrying all 12 VendorBuyback INV_SLOT pointers + BUYBACK_PRICE/TIMESTAMP arrays (the price/
/// timestamp arrays are gtker-walled past slot 0 → the shared raw encoder). Cleared slots write
/// guid 0 / price 0, so ring shifts and evictions render correctly without tracking prior state.
fn push_buyback_view<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    self_guid: u64,
    skip_if_empty: bool,
) -> Result<()> {
    let ring = store.buyback_ring(self_guid);
    log::debug!("buyback view: guid={self_guid} ring_len={}", ring.len());
    // Login replay of an EMPTY ring is a no-op by construction (the client's descriptor fields
    // start zeroed) — skipping keeps the login sequence byte-identical for ring-less players.
    // In-session callers always push (a ring that just BECAME empty must clear the tab).
    if skip_if_empty && ring.is_empty() {
        return Ok(());
    }
    let mut mask = codec::update_mask::UpdateMaskValues::new();
    for i in 0..12u16 {
        let (fab_guid, price) = match ring.get(i as usize) {
            Some(&(entry, count, price)) => {
                let fab_guid = 0x4090_0000_0000_0000u64 | u64::from(i);
                let view = codec::ItemInstanceView {
                    guid: fab_guid,
                    entry,
                    owner_guid: self_guid,
                    slot: 69 + i as u8,
                    stack_count: count,
                    durability: 0,
                    max_durability: 0,
                    container_slots: 0,
                };
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                        codec::build_item_create_object(&view),
                    ))),
                )?;
                (fab_guid, price)
            }
            None => (0, 0),
        };
        // PLAYER_FIELD_INV_SLOT guid pair for VendorBuyback slot 69+i (base 486, 2 words/slot);
        // BUYBACK_PRICE_1 = 1226, BUYBACK_TIMESTAMP_1 = 1238 (5875 indices via gtker impls).
        mask.set_u64(486 + (69 + i) * 2, fab_guid);
        mask.set_u32(1226 + i, price);
        mask.set_u32(1238 + i, 0);
    }
    let (opcode, body) = codec::build_values_update_raw(self_guid, &mask);
    send(tx, Outbound::Raw { opcode, body })
}

/// Open the bank window for `banker_guid`. Single chokepoint for `CMSG_BANKER_ACTIVATE` and the
/// BANKER gossip option, so the two entry points cannot drift apart.
fn send_show_bank(tx: &SessionTx, banker_guid: u64) -> Result<()> {
    send(
        tx,
        Outbound::One(ServerOpcodeMessage::SMSG_SHOW_BANK(codec::build_show_bank(
            banker_guid,
        ))),
    )
}

/// The quest menu for `giver` (creature OR gameobject guid — `quest_giver_evals` resolves either)
/// against `self_guid`: vanilla "instant quest" (mangos `SendPreparedQuest`) opens a
/// SINGLE menu-worthy quest's screen DIRECTLY (accept details for a new quest, the reward screen for a
/// finished turn-in, the "not done yet" request-items screen for one in progress); a giver with
/// MULTIPLE quests shows the list instead. Shared by `CMSG_QUESTGIVER_HELLO` (a
/// creature giver) and `CMSG_GAMEOBJ_USE` on a `go_type::QUESTGIVER` gameobject (a GO giver) — the two
/// client interactions converge on the exact same window, so this is the single chokepoint that keeps
/// them from drifting apart (mirrors `filtered_gossip_options`'s HELLO/SELECT_OPTION rationale).
fn send_questgiver_menu<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    giver: u64,
    self_guid: u64,
) -> Result<()> {
    let evals = store.quest_giver_evals(giver, self_guid)?;
    let menu = codec::quest_menu_items(&evals);
    let single = if menu.len() == 1 {
        store.quest_detail(menu[0].quest_id)?
    } else {
        None
    };
    if let Some(detail) = single {
        let turn_in = evals
            .iter()
            .find(|e| e.quest_id == detail.quest_id && e.role == codec::ROLE_END && e.active);
        if turn_in.is_none() {
            let (opcode, body) = codec::build_quest_details_raw(giver, &detail);
            send(tx, Outbound::Raw { opcode, body })?;
            return Ok(());
        }
        let out = match turn_in {
            Some(e) if e.complete => ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(Box::new(
                codec::build_offer_reward(giver, &detail),
            )),
            Some(_) => ServerOpcodeMessage::SMSG_QUESTGIVER_REQUEST_ITEMS(Box::new(
                codec::build_request_items(giver, &detail, false),
            )),
            None => unreachable!("new quests are handled by the raw DETAILS branch above"),
        };
        send(tx, Outbound::One(out))?;
    } else {
        send(
            tx,
            Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_LIST(Box::new(
                codec::build_quest_list(giver, "Greetings.", &evals),
            ))),
        )?;
    }
    Ok(())
}
