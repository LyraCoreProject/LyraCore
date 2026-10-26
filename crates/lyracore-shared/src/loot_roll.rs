pub mod event_kind {
    pub const ROLL_START: u8 = 4;
    pub const ROLL_VOTE: u8 = 5;
    pub const ROLL_WON: u8 = 6;
    pub const MASTER_LIST: u8 = 7;
    pub const MONEY_SHARE: u8 = 8;
    pub const ALL_PASSED: u8 = 12;
}

// RollVote wire values: Pass = 0, Need = 1, Greed = 2.
pub mod vote_kind {
    pub const PASS: u8 = 0;
    pub const NEED: u8 = 1;
    pub const GREED: u8 = 2;
}

pub fn encode_start(
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    countdown_ms: u32,
    random_property_id: u32,
) -> String {
    format!("{corpse_guid},{slot},{item_entry},{countdown_ms},{random_property_id}")
}

pub fn decode_start(payload: &str) -> Option<(u64, u8, u32, u32, u32)> {
    let mut p = payload.split(',');
    let corpse_guid: u64 = p.next()?.parse().ok()?;
    let slot: u8 = p.next()?.parse().ok()?;
    let item_entry: u32 = p.next()?.parse().ok()?;
    let countdown_ms: u32 = p.next()?.parse().ok()?;
    let random_property_id = p.next().map(str::parse).transpose().ok()?.unwrap_or(0);
    if p.next().is_some() {
        return None;
    }
    Some((
        corpse_guid,
        slot,
        item_entry,
        countdown_ms,
        random_property_id,
    ))
}

// The voter guid is GroupEvent.other_guid, outside the payload.
pub fn encode_vote(
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    roll_number: u8,
    vote: u8,
    auto_pass: bool,
    random_property_id: u32,
) -> String {
    format!(
        "{corpse_guid},{slot},{item_entry},{roll_number},{vote},{},{random_property_id}",
        auto_pass as u8
    )
}

pub fn decode_vote(payload: &str) -> Option<(u64, u8, u32, u8, u8, bool, u32)> {
    let mut p = payload.split(',');
    let corpse_guid: u64 = p.next()?.parse().ok()?;
    let slot: u8 = p.next()?.parse().ok()?;
    let item_entry: u32 = p.next()?.parse().ok()?;
    let roll_number: u8 = p.next()?.parse().ok()?;
    let vote: u8 = p.next()?.parse().ok()?;
    let auto_pass = p.next()? == "1";
    let random_property_id = p.next().map(str::parse).transpose().ok()?.unwrap_or(0);
    if p.next().is_some() {
        return None;
    }
    Some((
        corpse_guid,
        slot,
        item_entry,
        roll_number,
        vote,
        auto_pass,
        random_property_id,
    ))
}

pub fn encode_won(
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    winning_roll: u8,
    winning_vote: u8,
    random_property_id: u32,
) -> String {
    format!("{corpse_guid},{slot},{item_entry},{winning_roll},{winning_vote},{random_property_id}")
}

pub fn decode_won(payload: &str) -> Option<(u64, u8, u32, u8, u8, u32)> {
    let mut p = payload.split(',');
    let corpse_guid: u64 = p.next()?.parse().ok()?;
    let slot: u8 = p.next()?.parse().ok()?;
    let item_entry: u32 = p.next()?.parse().ok()?;
    let winning_roll: u8 = p.next()?.parse().ok()?;
    let winning_vote: u8 = p.next()?.parse().ok()?;
    let random_property_id = p.next().map(str::parse).transpose().ok()?.unwrap_or(0);
    if p.next().is_some() {
        return None;
    }
    Some((
        corpse_guid,
        slot,
        item_entry,
        winning_roll,
        winning_vote,
        random_property_id,
    ))
}

pub fn encode_all_passed(
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    random_property_id: u32,
) -> String {
    format!("{corpse_guid},{slot},{item_entry},{random_property_id}")
}

pub fn decode_all_passed(payload: &str) -> Option<(u64, u8, u32, u32)> {
    let mut p = payload.split(',');
    let corpse_guid: u64 = p.next()?.parse().ok()?;
    let slot: u8 = p.next()?.parse().ok()?;
    let item_entry: u32 = p.next()?.parse().ok()?;
    let random_property_id: u32 = p.next()?.parse().ok()?;
    if p.next().is_some() {
        return None;
    }
    Some((corpse_guid, slot, item_entry, random_property_id))
}

pub fn encode_master_list(corpse_guid: u64, eligible: &[u64]) -> String {
    let guids: Vec<String> = eligible.iter().map(|g| g.to_string()).collect();
    format!("{corpse_guid}|{}", guids.join(","))
}

pub fn decode_master_list(payload: &str) -> Option<(u64, Vec<u64>)> {
    let (corpse_guid, rest) = payload.split_once('|')?;
    let corpse_guid: u64 = corpse_guid.parse().ok()?;
    let mut guids = Vec::new();
    for g in rest.split(',').filter(|s| !s.is_empty()) {
        guids.push(g.parse().ok()?);
    }
    (!guids.is_empty()).then_some((corpse_guid, guids))
}

pub fn encode_money_share(amount: u32) -> String {
    amount.to_string()
}

pub fn decode_money_share(payload: &str) -> Option<u32> {
    payload.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_properties_cross_events_and_old_events_stay_plain() {
        assert_eq!(
            decode_start(&encode_start(1, 2, 3, 4000, 117)),
            Some((1, 2, 3, 4000, 117))
        );
        assert_eq!(decode_start("1,2,3,4000"), Some((1, 2, 3, 4000, 0)));
        assert_eq!(
            decode_vote(&encode_vote(1, 2, 3, 99, 1, false, 117)),
            Some((1, 2, 3, 99, 1, false, 117))
        );
        assert_eq!(
            decode_won(&encode_won(1, 2, 3, 99, 1, 117)),
            Some((1, 2, 3, 99, 1, 117))
        );
    }

    #[test]
    fn start_round_trips() {
        let wire = encode_start(555, 2, 1234, 60_000, 0);
        assert_eq!(decode_start(&wire), Some((555, 2, 1234, 60_000, 0)));
    }

    #[test]
    fn start_fails_closed_on_garbage() {
        assert!(decode_start("").is_none());
        assert!(decode_start("1,2,3").is_none());
        assert!(decode_start("1,2,3,4,5,6").is_none());
        assert!(decode_start("x,2,3,4").is_none());
    }

    #[test]
    fn vote_round_trips_including_auto_pass() {
        let wire = encode_vote(555, 2, 1234, 87, vote_kind::NEED, false, 0);
        assert_eq!(
            decode_vote(&wire),
            Some((555, 2, 1234, 87, vote_kind::NEED, false, 0))
        );
        let auto = encode_vote(555, 2, 1234, 0, vote_kind::PASS, true, 0);
        assert_eq!(
            decode_vote(&auto),
            Some((555, 2, 1234, 0, vote_kind::PASS, true, 0))
        );
    }

    #[test]
    fn vote_kind_matches_the_real_wire_rollvote_ordering() {
        assert_eq!(vote_kind::PASS, 0);
        assert_eq!(vote_kind::NEED, 1);
        assert_eq!(vote_kind::GREED, 2);
    }

    #[test]
    fn won_round_trips() {
        let wire = encode_won(555, 2, 1234, 96, vote_kind::GREED, 0);
        assert_eq!(
            decode_won(&wire),
            Some((555, 2, 1234, 96, vote_kind::GREED, 0))
        );
    }

    #[test]
    fn master_list_round_trips() {
        let wire = encode_master_list(555, &[10, 20, 30]);
        assert_eq!(decode_master_list(&wire), Some((555, vec![10, 20, 30])));
    }

    #[test]
    fn master_list_fails_closed_on_garbage() {
        assert!(decode_master_list("").is_none());
        assert!(decode_master_list("555|").is_none());
        assert!(decode_master_list("555|1,x,3").is_none());
    }

    #[test]
    fn reserved_kind_range_does_not_collide_with_group_event_kinds() {
        use crate::group::event_kind as g;
        assert_eq!(g::LOOT_ROLL_RESERVED_START, event_kind::ROLL_START);
        const { assert!(event_kind::ROLL_START > crate::group::event_kind::DESTROYED) };
        assert_eq!(event_kind::ROLL_VOTE, event_kind::ROLL_START + 1);
        assert_eq!(event_kind::ROLL_WON, event_kind::ROLL_VOTE + 1);
        assert_eq!(event_kind::MASTER_LIST, event_kind::ROLL_WON + 1);
        assert_eq!(event_kind::MONEY_SHARE, event_kind::MASTER_LIST + 1);
    }

    #[test]
    fn all_group_and_loot_event_kinds_are_distinct_0_through_8() {
        use crate::group::event_kind as g;
        let mut kinds = vec![
            g::INVITE,
            g::LIST,
            g::DECLINE,
            g::DESTROYED,
            event_kind::ROLL_START,
            event_kind::ROLL_VOTE,
            event_kind::ROLL_WON,
            event_kind::MASTER_LIST,
            event_kind::MONEY_SHARE,
        ];
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds, (0u8..=8).collect::<Vec<u8>>());
    }

    #[test]
    fn all_passed_round_trips_and_fails_closed() {
        assert_eq!(
            decode_all_passed(&encode_all_passed(555, 2, 1234, 117)),
            Some((555, 2, 1234, 117))
        );
        assert!(decode_all_passed("555,2,1234").is_none());
        assert!(decode_all_passed("555,2,1234,0,9").is_none());
        assert!(decode_all_passed("x,2,1234,0").is_none());
    }

    #[test]
    fn money_share_round_trips() {
        let wire = encode_money_share(37);
        assert_eq!(decode_money_share(&wire), Some(37));
        let zero = encode_money_share(0);
        assert_eq!(decode_money_share(&zero), Some(0));
    }

    #[test]
    fn money_share_fails_closed_on_garbage() {
        assert!(decode_money_share("").is_none());
        assert!(decode_money_share("12x").is_none());
        assert!(decode_money_share("-1").is_none());
    }
}
