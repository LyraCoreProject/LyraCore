//! Quest math shared between the module (which GRANTS the reward) and the gateway (which DISPLAYS it
//! in `SMSG_QUESTGIVER_QUEST_COMPLETE`). Keeping the formula here means the two can never drift — the
//! number the completion popup shows is exactly the number the turn-in awards.

/// The XP reward for a quest of `quest_level`, used when its template carries no explicit `reward_xp`
/// (vanilla 1.12 has no RewXX column — it's derived). Simplified monotonic placeholder: `45*level + 40`
/// (saturating, so a garbage-high level can't wrap). Pure. Replace with the real per-level quest-XP
/// table when we ingest it (mirrors `xp::xp_to_next_level` being a documented stand-in).
pub fn xp_reward(quest_level: u32) -> u32 {
    quest_level.saturating_mul(45).saturating_add(40)
}

/// Quest RACE gate: `required_races` is a cmangos bitmask of allowed races (`0` = all races). Race
/// `race` (1-based, Human=1, Orc=2, …, the `UNIT_FIELD_BYTES_0` race byte) is allowed iff the mask is 0
/// or its bit `1 << (race-1)` is set. Shared by the module's accept gate AND the gateway's `!` status so
/// the icon and the reducer agree on who can take a quest. Pure.
pub fn race_allowed(required_races: u32, race: u8) -> bool {
    required_races == 0 || ((1..=32).contains(&race) && required_races & (1u32 << (race - 1)) != 0)
}

/// Quest CLASS gate: `required_classes` is a cmangos bitmask of allowed classes (`0` = all classes).
/// Class `class` (1-based, Warrior=1, Paladin=2, …) is allowed iff the mask is 0 or bit `1 << (class-1)`
/// is set. The class twin of [`race_allowed`]. Pure.
pub fn class_allowed(required_classes: u32, class: u8) -> bool {
    required_classes == 0
        || ((1..=32).contains(&class) && required_classes & (1u32 << (class - 1)) != 0)
}

/// [V] vanilla's max-level (60) quest XP→copper conversion factor. cmangos's real
/// `RewMoneyMaxLevel`/gold-at-cap table isn't reachable in this sandbox (no client/dump/live-node
/// access to confirm it) — this is a flat, documented placeholder: 6 copper per XP point the quest
/// would otherwise have granted. Shared by the module's turn-in payout
/// (`module/src/quest.rs::apply_turn_in_quest`) and the gateway's `max_level_money_reward` wire stub
/// (`gateway/src/codec/quest.rs::build_quest_query_response_raw`) so the quest-log preview number is
/// EXACTLY what a level-60 turn-in pays — replace both call sites together if a real coefficient is
/// ever sourced.
pub const MAX_LEVEL_XP_TO_COPPER: u32 = 6;

/// The level-cap money conversion for a quest that would have granted `xp` experience. Pure.
pub fn max_level_money_reward(xp: u32) -> u32 {
    xp.saturating_mul(MAX_LEVEL_XP_TO_COPPER)
}

/// Authentic vanilla quest XP — the same award the mangos-family cores are observed to pay (a
/// behavioural reference, not a derivation; the arithmetic below is deliberately not theirs): the reward is
/// `RewMoneyMaxLevel / 0.6` — the dump's populated per-quest field (ingested as
/// `QuestTemplate.reward_money_max_level`). Replaces the `xp_reward` placeholder, which awarded ~half
/// of this. Integer `ceil(mm*5/3)` on purpose — a float `/0.6` is inexact (0.6 has no exact binary
/// form) and mis-rounds at exact boundaries (270/0.6 → 450.0000…16 → ceil 451, wrong). Vanilla's max
/// quest level is 60, so cmangos's 61-65 divisor ladder is unreachable and omitted.
///
/// The over-level PENALTY (cmangos reduces the reward when `player_level > quest_level + 5`, down to
/// 10% for grey quests) is a deferred refinement: when added it must be applied IDENTICALLY at the
/// award (`module/src/quest.rs`) and the gateway preview so the completion popup always equals the
/// grant — the whole reason this math lives in one shared place. `reward_money_max_level == 0` (a
/// no-XP quest, ~18% of rows) returns 0. Pure.
pub fn quest_xp(reward_money_max_level: u32) -> u32 {
    if reward_money_max_level == 0 {
        return 0;
    }
    // `div_ceil(3)` IS `(n + 2) / 3` for unsigned n — same rounding, same value, no overflow
    // difference at u64 width. Kept as the named operation so the "round up" intent is explicit.
    (((reward_money_max_level as u64) * 5).div_ceil(3)) as u32
}

/// Work-item 194 (sharing): kinds carried in `game_group_event.kind` for `push_quest_to_party`'s
/// notifications. Reserved slots `10..=11` — the next free range after
/// `crate::group::event_kind::PARTY_CHAT` (9); see that module's doc for the full reserved-range
/// bookkeeping (loot-roll kinds `4..=8` live in the sibling `crate::loot_roll` file).
pub mod share_event_kind {
    /// An eligible party member receives the shared quest → the gateway relay opens
    /// `SMSG_QUESTGIVER_QUEST_DETAILS` for it (the sharer as "giver" — see module `GiverKind::Party`).
    /// Payload: the quest entry (decimal string).
    pub const QUEST_SHARE: u8 = 10;
    /// The SENDER's per-member feedback (one row per member, always) → `MSG_QUEST_PUSH_RESULT`.
    /// Payload: the [`super::share_result`] code (decimal string) — mirrors gtker's
    /// `wow_world_base::shared::quest_party_message_vanilla_tbc::QuestPartyMessage::as_int()` 1:1, so
    /// the gateway's decode is a straight `try_into()`, never a translation table.
    pub const QUEST_PUSH_RESULT: u8 = 11;
}

/// Work-item 194 (sharing): the `MSG_QUEST_PUSH_RESULT` wire codes (mirrors gtker's
/// `QuestPartyMessage::as_int()` exactly — `SharingQuest=0, CantTakeQuest=1, TooFar=4, LogFull=6,
/// HaveQuest=7, FinishQuest=8` — so the gateway's decode of a `QUEST_PUSH_RESULT` payload is a plain
/// `u8::try_into()`, never a translation table). Not every gtker variant is reachable from
/// `share_result` (this slice never produces `AcceptQuest`/`DeclineQuest`/`Busy` — there is no
/// asynchronous accept/decline round trip here, only the push + its immediate feedback).
pub mod share_result {
    pub const SHARING_QUEST: u8 = 0;
    pub const CANT_TAKE_QUEST: u8 = 1;
    pub const TOO_FAR: u8 = 4;
    pub const LOG_FULL: u8 = 6;
    pub const HAVE_QUEST: u8 = 7;
    pub const FINISH_QUEST: u8 = 8;
}

/// The pure per-member decision for `push_quest_to_party` (module `quest::evaluate_share` gathers the
/// live booleans; this fn just picks the outcome — unit-tested here without a `ReducerContext`).
/// Priority order (first match wins, mirrors vanilla's own precedence): offline/wrong-map/out-of-range
/// → `TOO_FAR`; already actively on the quest → `HAVE_QUEST`; already turned in (non-repeatable) →
/// `FINISH_QUEST`; quest log full → `LOG_FULL`; level/race/class gates unmet → `CANT_TAKE_QUEST`; else
/// → `SHARING_QUEST` (the offer actually lands).
#[allow(clippy::too_many_arguments)]
pub fn share_result(
    member_online: bool,
    same_map_instance: bool,
    in_range: bool,
    member_active: bool,
    member_finished: bool,
    log_full: bool,
    gates_ok: bool,
) -> u8 {
    if !member_online || !same_map_instance || !in_range {
        share_result::TOO_FAR
    } else if member_active {
        share_result::HAVE_QUEST
    } else if member_finished {
        share_result::FINISH_QUEST
    } else if log_full {
        share_result::LOG_FULL
    } else if !gates_ok {
        share_result::CANT_TAKE_QUEST
    } else {
        share_result::SHARING_QUEST
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xp_reward_is_monotonic_and_nonzero() {
        assert_eq!(xp_reward(1), 85);
        assert_eq!(xp_reward(2), 130);
        assert_eq!(xp_reward(5), 265);
        assert!(xp_reward(10) > xp_reward(5));
        assert!(xp_reward(u32::MAX) > 0);
    }

    #[test]
    fn quest_xp_is_authentic_and_exact() {
        // Verified against cmangos RewMoneyMaxLevel/0.6 (parsed from the dump): these are the exact
        // authentic values, ~2x the old xp_reward placeholder.
        assert_eq!(quest_xp(102), 170); // Kobold Camp Cleanup (ql2) — was xp_reward(2)=130
        assert_eq!(quest_xp(150), 250); // Investigate Echo Ridge (ql3)
        assert_eq!(quest_xp(270), 450); // Skirmish at Echo Ridge (ql5) — was xp_reward(5)=265
        assert_eq!(quest_xp(0), 0); // a no-XP quest awards nothing (falls through to xp_reward)
                                    // integer ceil never mis-rounds where a float /0.6 would
        assert_eq!(quest_xp(3), 5); // ceil(3*5/3)=5
        assert!(quest_xp(u32::MAX) > 0); // no overflow (u64 intermediate)
    }

    #[test]
    fn race_class_gates_respect_bitmask_and_zero_means_all() {
        // 0 = no restriction → anyone qualifies.
        assert!(race_allowed(0, 1));
        assert!(class_allowed(0, 1));
        // Northshire human chain: RequiredRaces 77 = 0x4D = Human(1)|Dwarf(4)|NightElf(8)|Gnome(64).
        assert!(race_allowed(77, 1)); // Human (bit 0)
        assert!(race_allowed(77, 3)); // Dwarf (bit 2)
        assert!(!race_allowed(77, 2)); // Orc (bit 1) — Horde, excluded
        assert!(!race_allowed(77, 5)); // Undead (bit 4) — excluded
                                       // Warrior-only quest (RequiredClasses 1 = bit 0).
        assert!(class_allowed(1, 1)); // Warrior
        assert!(!class_allowed(1, 2)); // Paladin
                                       // A 0 race/class id never matches a restrictive mask (defensive).
        assert!(!race_allowed(77, 0));
        assert!(!class_allowed(1, 0));
    }

    #[test]
    fn race_class_gates_pin_the_bit31_bit33_range_boundary() {
        // race=32 is the LAST valid bit position (1 << 31, the top bit of the u32 mask) — still in
        // range, and it's allowed when that top bit is set in the mask.
        let mask_bit31: u32 = 1 << 31;
        assert!(race_allowed(mask_bit31, 32));
        assert!(class_allowed(mask_bit31, 32));
        // race=33 would need `1 << 32`, which overflows a u32 shift — the `(1..=32)` range guard
        // must reject it BEFORE the shift executes, never panicking, always just "not allowed".
        assert!(!race_allowed(mask_bit31, 33));
        assert!(!class_allowed(mask_bit31, 33));
        // Same guard applies to u8::MAX, the far end of the race/class byte's range.
        assert!(!race_allowed(u32::MAX, u8::MAX));
        assert!(!class_allowed(u32::MAX, u8::MAX));
    }

    // ---- Level-cap payout (work-item 194(e)) ----

    #[test]
    fn max_level_money_reward_uses_the_documented_v_coefficient() {
        assert_eq!(MAX_LEVEL_XP_TO_COPPER, 6);
        assert_eq!(max_level_money_reward(0), 0);
        assert_eq!(max_level_money_reward(100), 600);
        assert_eq!(max_level_money_reward(u32::MAX), u32::MAX); // saturates, never wraps/panics
    }

    // ---- Sharing (work-item 194): share_event_kind distinctness + share_result priority ----

    /// `QUEST_SHARE`/`QUEST_PUSH_RESULT` (10/11) are the next free slots after
    /// `crate::group::event_kind::PARTY_CHAT` (9) and every loot-roll kind (4..=8) — widens the
    /// all-group-event-kinds distinctness pin from `0..=9` (loot_roll.rs) to `0..=11`.
    #[test]
    fn share_event_kinds_are_distinct_and_widen_the_range_to_11() {
        use crate::group::event_kind as g;
        use crate::loot_roll::event_kind as roll;
        let mut kinds = vec![
            g::INVITE,
            g::LIST,
            g::DECLINE,
            g::DESTROYED,
            roll::ROLL_START,
            roll::ROLL_VOTE,
            roll::ROLL_WON,
            roll::MASTER_LIST,
            roll::MONEY_SHARE,
            g::PARTY_CHAT,
            share_event_kind::QUEST_SHARE,
            share_event_kind::QUEST_PUSH_RESULT,
        ];
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds, (0u8..=11).collect::<Vec<u8>>());
        assert_eq!(share_event_kind::QUEST_SHARE, g::PARTY_CHAT + 1);
        assert_eq!(
            share_event_kind::QUEST_PUSH_RESULT,
            share_event_kind::QUEST_SHARE + 1
        );
    }

    /// `share_result`'s wire codes mirror gtker's `QuestPartyMessage::as_int()` exactly — a mismatch
    /// here would silently pick the wrong client-side dialog/toast.
    #[test]
    fn share_result_codes_mirror_the_gtker_quest_party_message_enum() {
        assert_eq!(share_result::SHARING_QUEST, 0);
        assert_eq!(share_result::CANT_TAKE_QUEST, 1);
        assert_eq!(share_result::TOO_FAR, 4);
        assert_eq!(share_result::LOG_FULL, 6);
        assert_eq!(share_result::HAVE_QUEST, 7);
        assert_eq!(share_result::FINISH_QUEST, 8);
    }

    /// `share_result`'s priority order: TOO_FAR beats every other reason (an offline/out-of-range
    /// member's OTHER state doesn't matter), then HAVE_QUEST, then FINISH_QUEST, then LOG_FULL, then
    /// CANT_TAKE_QUEST, and only every gate passing yields SHARING_QUEST.
    #[test]
    fn share_result_priority_order() {
        // The all-pass baseline succeeds.
        assert_eq!(
            share_result(true, true, true, false, false, false, true),
            share_result::SHARING_QUEST
        );
        // Out-of-range/offline/wrong-map beats EVERY other condition, even a simultaneous HAVE_QUEST.
        assert_eq!(
            share_result(false, true, true, true, true, true, false),
            share_result::TOO_FAR
        );
        assert_eq!(
            share_result(true, false, true, false, false, false, true),
            share_result::TOO_FAR
        );
        assert_eq!(
            share_result(true, true, false, false, false, false, true),
            share_result::TOO_FAR
        );
        // Already active beats finished/log-full/gates.
        assert_eq!(
            share_result(true, true, true, true, true, true, false),
            share_result::HAVE_QUEST
        );
        // Finished (non-repeatable) beats log-full/gates.
        assert_eq!(
            share_result(true, true, true, false, true, true, false),
            share_result::FINISH_QUEST
        );
        // Log-full beats gates.
        assert_eq!(
            share_result(true, true, true, false, false, true, false),
            share_result::LOG_FULL
        );
        // Gates unmet, nothing else wrong.
        assert_eq!(
            share_result(true, true, true, false, false, false, false),
            share_result::CANT_TAKE_QUEST
        );
    }
}
