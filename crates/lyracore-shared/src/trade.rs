//! The module↔gateway TRADE wire contract (#120) — event-kind codes for the Trade Session status
//! relay. Cross-boundary constants live HERE, both crates import: a module-side renumber becomes a
//! compile-visible edit on the gateway side instead of a runtime drift. Same precedent as
//! `group::event_kind`.

/// Trade-event kinds (`game_trade_event.kind`) — which `SMSG_TRADE_STATUS` variant the gateway
/// relays. One byte per status the module actually emits; NOT the vanilla `TradeStatus`
/// discriminants (the gateway owns that wire mapping, and most vanilla variants are never
/// module-emitted).
pub mod event_kind {
    /// You are proposed a trade (`other_guid` = the initiator) → `TradeStatus::BeginTrade`.
    /// The client answers with `CMSG_BEGIN_TRADE`, which opens the window on both sides.
    pub const BEGIN_TRADE: u8 = 0;
    /// The proposal was answered — both parties' windows open → `TradeStatus::OpenWindow`.
    pub const OPEN_WINDOW: u8 = 1;
    /// The Trade Session ended without a Trade Commit (explicit cancel, logout) →
    /// `TradeStatus::TradeCanceled`.
    pub const TRADE_CANCELED: u8 = 2;
    /// Initiator refusal: you or the target are already in a Trade Session → `TradeStatus::Busy`.
    pub const BUSY: u8 = 3;
    /// Initiator refusal: no such partner (absent, mid-transfer, not a player, or yourself) →
    /// `TradeStatus::NoTarget`.
    pub const NO_TARGET: u8 = 4;
    /// Initiator refusal: partner beyond the 10 yd interaction range (or another partition) →
    /// `TradeStatus::TargetToFar` (the wire library's spelling).
    pub const TARGET_TO_FAR: u8 = 5;
    /// Initiator refusal: cross-faction trade → `TradeStatus::WrongFaction`.
    pub const WRONG_FACTION: u8 = 6;
    /// Initiator refusal: you are dead → `TradeStatus::YouDead`.
    pub const YOU_DEAD: u8 = 7;
    /// Initiator refusal: the target is dead → `TradeStatus::TargetDead`.
    pub const TARGET_DEAD: u8 = 8;
    /// Your OWN offer, echoed back after a mutation (payload = [`super::encode_offer`]) →
    /// `SMSG_TRADE_STATUS_EXTENDED` with `self_player = true` (#121).
    pub const OFFER_SELF: u8 = 9;
    /// Your PARTNER's offer after their mutation (same payload grammar) →
    /// `SMSG_TRADE_STATUS_EXTENDED` with `self_player = false` (#121).
    pub const OFFER_PARTNER: u8 = 10;
    /// Initiator notice: the target has you ignored — proposal declined →
    /// `TradeStatus::IgnoreYou` (#123).
    pub const IGNORE_YOU: u8 = 11;
}

/// One offered item as the trade window renders it — resolved by the MODULE at write time (the
/// roster payload-carry pattern: instance + template joined in the same transaction as the offer
/// change, so the gateway relay never races a read). Field order is the payload grammar below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferSlot {
    /// 0..=5 traded; 6 = the Will-Not-Be-Traded Slot (shown, never committed).
    pub trade_slot: u8,
    pub entry: u32,
    pub display_id: u32,
    pub stack_count: u32,
    pub enchantment: u32,
    pub durability: u32,
    pub max_durability: u32,
}

/// Encode one side's whole offer for an `OFFER_*` event payload:
/// `gold|slot,entry,display,stack,enchant,dur,maxdur;slot,...` — occupied slots only, `gold` in
/// copper. All-numeric fields, so no delimiter stripping is needed (unlike `group::encode_roster`).
pub fn encode_offer(gold: u32, slots: &[OfferSlot]) -> String {
    let body: Vec<String> = slots
        .iter()
        .map(|s| {
            format!(
                "{},{},{},{},{},{},{}",
                s.trade_slot,
                s.entry,
                s.display_id,
                s.stack_count,
                s.enchantment,
                s.durability,
                s.max_durability
            )
        })
        .collect();
    format!("{gold}|{}", body.join(";"))
}

/// Decode an `OFFER_*` payload back to `(gold, slots)`. Fails closed: any malformed field drops
/// the WHOLE payload (a half-decoded trade window is worse than a stale one).
pub fn decode_offer(payload: &str) -> Option<(u32, Vec<OfferSlot>)> {
    let (gold, body) = payload.split_once('|')?;
    let gold: u32 = gold.parse().ok()?;
    let mut slots = Vec::new();
    for part in body.split(';').filter(|p| !p.is_empty()) {
        let f: Vec<&str> = part.split(',').collect();
        if f.len() != 7 {
            return None;
        }
        slots.push(OfferSlot {
            trade_slot: f[0].parse().ok()?,
            entry: f[1].parse().ok()?,
            display_id: f[2].parse().ok()?,
            stack_count: f[3].parse().ok()?,
            enchantment: f[4].parse().ok()?,
            durability: f[5].parse().ok()?,
            max_durability: f[6].parse().ok()?,
        });
    }
    Some((gold, slots))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offer payload round-trips with every window-visible field intact (the #121 "real stack
    /// count, durability, enchant" AC rides these fields), an empty offer is legal (gold-only or
    /// cleared window), and malformed payloads fail closed as a whole.
    #[test]
    fn offer_payload_round_trips_and_fails_closed_on_malformed_input() {
        let slots = vec![
            OfferSlot {
                trade_slot: 0,
                entry: 2589,
                display_id: 7026,
                stack_count: 20,
                enchantment: 0,
                durability: 0,
                max_durability: 0,
            },
            OfferSlot {
                trade_slot: 6,
                entry: 6948,
                display_id: 6418,
                stack_count: 1,
                enchantment: 2564,
                durability: 34,
                max_durability: 40,
            },
        ];
        let payload = encode_offer(1_2345, &slots);
        assert_eq!(decode_offer(&payload), Some((1_2345, slots)));

        assert_eq!(decode_offer(&encode_offer(0, &[])), Some((0, Vec::new())));

        assert_eq!(decode_offer(""), None, "no delimiter");
        assert_eq!(decode_offer("abc|"), None, "non-numeric gold");
        assert_eq!(decode_offer("5|1,2,3"), None, "truncated slot");
        assert_eq!(
            decode_offer("5|1,2,3,4,5,6,x"),
            None,
            "one bad field drops the whole payload"
        );
    }
}
