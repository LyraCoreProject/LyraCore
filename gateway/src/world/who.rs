//! `/who`: a pure filter over [`presence::in_world_characters`], plus the `SMSG_WHO` response
//! builder `CMSG_WHO` (`social.rs`) drives. Behavior pinned against cm:MiscHandler.cpp:71-268.

use anyhow::Result;

use super::{presence, WorldStore};
use crate::codec;
use lyracore_shared::faction;
use wow_world_messages::vanilla::{CMSG_WHO, SMSG_WHO};

/// More than this many zones gets no answer (cm:MiscHandler.cpp:90-91).
const MAX_ZONES: usize = 10;
/// More than this many search strings gets no answer (cm:MiscHandler.cpp:107-108).
const MAX_SEARCH_STRINGS: usize = 4;
/// A maximum level at or above this means no upper bound (cm:MiscHandler.cpp:133-136).
const NO_UPPER_BOUND_AT: u8 = 100;

/// One in-world Character as the filter judges it. Borrowed, so [`matches`] can be exercised with
/// rows written by hand from the cited behavior, with no Store involved.
pub(crate) struct WhoCandidate<'a> {
    pub name: &'a str,
    /// Always empty until the guild workstream fills it; a non-empty guild filter then matches
    /// nobody, which is the correct answer today.
    pub guild_name: &'a str,
    pub race: u8,
    pub class: u8,
    pub level: u8,
    pub zone_id: u32,
    pub zone_name: &'a str,
}

/// `CMSG_WHO`'s filters, unpacked once per request.
pub(crate) struct WhoFilter<'a> {
    pub requester_team: u32,
    pub min_level: u8,
    pub max_level: u8,
    pub name: &'a str,
    pub guild_name: &'a str,
    pub race_mask: u32,
    pub class_mask: u32,
    pub zones: &'a [u32],
    pub search_strings: &'a [String],
}

/// Case-insensitive substring match; an empty filter matches everything (cm:MiscHandler.cpp:206-222).
fn name_matches(filter: &str, value: &str) -> bool {
    filter.is_empty()
        || value
            .to_ascii_lowercase()
            .contains(&filter.to_ascii_lowercase())
}

/// Does `candidate` pass every `/who` rule the requester sent? One clause per cited rule
/// (cm:MiscHandler.cpp:156-244).
pub(crate) fn matches(filter: &WhoFilter, candidate: &WhoCandidate) -> bool {
    faction::team_for_race(candidate.race) == filter.requester_team
        && candidate.level >= filter.min_level
        && candidate.level <= filter.max_level
        && filter.class_mask & (1 << candidate.class) != 0
        && filter.race_mask & (1 << candidate.race) != 0
        && (filter.zones.is_empty() || filter.zones.contains(&candidate.zone_id))
        && name_matches(filter.name, candidate.name)
        && name_matches(filter.guild_name, candidate.guild_name)
        && filter.search_strings.iter().all(|s| {
            name_matches(s, candidate.name)
                || name_matches(s, candidate.guild_name)
                || name_matches(s, candidate.zone_name)
        })
}

/// More than 10 zones or more than 4 search strings gets no answer at all
/// (cm:MiscHandler.cpp:90-91, cm:MiscHandler.cpp:107-108).
fn is_oversized(request: &CMSG_WHO) -> bool {
    request.zones.len() > MAX_ZONES || request.search_strings.len() > MAX_SEARCH_STRINGS
}

/// Build the `SMSG_WHO` reply for `request`, or `None` when the request itself is oversized (see
/// [`is_oversized`]). `requester_race` decides the team every listed Character must share. Source
/// is [`presence::in_world_characters`], so bots stay listed, as today.
pub(crate) fn respond<St: WorldStore + ?Sized>(
    store: &St,
    requester_race: u8,
    request: &CMSG_WHO,
) -> Result<Option<SMSG_WHO>> {
    if is_oversized(request) {
        return Ok(None);
    }
    let max_level = request.maximum_level.as_int();
    let filter = WhoFilter {
        requester_team: faction::team_for_race(requester_race),
        min_level: request.minimum_level.as_int(),
        max_level: if max_level >= NO_UPPER_BOUND_AT {
            u8::MAX
        } else {
            max_level
        },
        name: &request.player_name,
        guild_name: &request.guild_name,
        race_mask: request.race_mask,
        class_mask: request.class_mask,
        zones: &request.zones,
        search_strings: &request.search_strings,
    };
    let mut players = Vec::new();
    for row in presence::in_world_characters(store)? {
        let zone_name = store.zone_name(row.zone_id);
        let candidate = WhoCandidate {
            name: &row.name,
            guild_name: "",
            race: row.race,
            class: row.class,
            level: row.level,
            zone_id: row.zone_id,
            zone_name: &zone_name,
        };
        if matches(&filter, &candidate) {
            players.push(codec::WhoPlayerView {
                name: row.name,
                level: row.level,
                class: row.class,
                race: row.race,
                zone_id: row.zone_id,
            });
        }
    }
    Ok(Some(codec::build_who_response(&players)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::faction::{TEAM_ALLIANCE, TEAM_HORDE};

    /// A Human Warrior in Elwynn (zone 12), Alliance, level 10 — the baseline every rule test
    /// starts from and narrows by overriding one field.
    fn baseline() -> (WhoFilter<'static>, WhoCandidate<'static>) {
        let filter = WhoFilter {
            requester_team: TEAM_ALLIANCE,
            min_level: 1,
            max_level: 60,
            name: "",
            guild_name: "",
            race_mask: 1 << 1,  // Human
            class_mask: 1 << 1, // Warrior
            zones: &[],
            search_strings: &[],
        };
        let candidate = WhoCandidate {
            name: "Ginger",
            guild_name: "",
            race: 1,
            class: 1,
            level: 10,
            zone_id: 12,
            zone_name: "Elwynn Forest",
        };
        (filter, candidate)
    }

    #[test]
    fn a_baseline_candidate_matches() {
        let (filter, candidate) = baseline();
        assert!(matches(&filter, &candidate));
    }

    #[test]
    fn only_the_requesters_team_matches() {
        let (mut filter, mut candidate) = baseline();
        filter.requester_team = TEAM_HORDE;
        assert!(!matches(&filter, &candidate));
        candidate.race = 2; // Orc
        filter.requester_team = TEAM_ALLIANCE;
        assert!(!matches(&filter, &candidate));
    }

    #[test]
    fn level_range_excludes_below_minimum_and_above_maximum() {
        let (mut filter, mut candidate) = baseline();
        filter.min_level = 20;
        assert!(!matches(&filter, &candidate));
        filter.min_level = 1;
        filter.max_level = 5;
        assert!(!matches(&filter, &candidate));
        candidate.level = 5;
        assert!(matches(&filter, &candidate));
    }

    #[test]
    fn a_maximum_level_at_or_above_100_removes_the_upper_bound() {
        let (mut filter, mut candidate) = baseline();
        filter.max_level = u8::MAX; // `respond` maps a wire 100+ to this
        candidate.level = 60;
        assert!(matches(&filter, &candidate));
    }

    #[test]
    fn class_mask_is_a_bit_test() {
        let (mut filter, candidate) = baseline();
        filter.class_mask = 1 << 2; // Paladin only
        assert!(!matches(&filter, &candidate));
    }

    #[test]
    fn race_mask_is_a_bit_test() {
        let (mut filter, candidate) = baseline();
        filter.race_mask = 1 << 3; // Dwarf only
        assert!(!matches(&filter, &candidate));
    }

    #[test]
    fn a_non_empty_zone_list_requires_membership() {
        let (mut filter, candidate) = baseline();
        filter.zones = &[1, 8];
        assert!(!matches(&filter, &candidate));
        filter.zones = &[1, 12];
        assert!(matches(&filter, &candidate));
    }

    #[test]
    fn name_filter_is_a_case_insensitive_substring_and_empty_matches_all() {
        let (mut filter, candidate) = baseline();
        filter.name = "ing";
        assert!(matches(&filter, &candidate));
        filter.name = "GIN";
        assert!(matches(&filter, &candidate));
        filter.name = "zzz";
        assert!(!matches(&filter, &candidate));
    }

    #[test]
    fn a_non_empty_guild_filter_matches_nobody_before_the_guild_workstream() {
        let (mut filter, candidate) = baseline();
        filter.guild_name = "Anything";
        assert!(
            !matches(&filter, &candidate),
            "candidate.guild_name is always empty today"
        );
    }

    #[test]
    fn a_search_string_matches_the_character_name() {
        let (mut filter, candidate) = baseline();
        let strings = ["ing".to_string()];
        filter.search_strings = &strings;
        assert!(matches(&filter, &candidate));
    }

    #[test]
    fn a_search_string_matches_the_zone_name() {
        let (mut filter, candidate) = baseline();
        let strings = ["elwynn".to_string()];
        filter.search_strings = &strings;
        assert!(matches(&filter, &candidate));
    }

    #[test]
    fn every_search_string_must_match_something() {
        let (mut filter, candidate) = baseline();
        let strings = ["ging".to_string(), "nowhere".to_string()];
        filter.search_strings = &strings;
        assert!(!matches(&filter, &candidate));
    }

    /// **AC 5 (zones): oversized zone or string lists get no answer.**
    #[test]
    fn more_than_ten_zones_is_oversized() {
        assert!(is_oversized(&CMSG_WHO {
            zones: vec![0; 11],
            ..Default::default()
        }));
        assert!(
            !is_oversized(&CMSG_WHO {
                zones: vec![0; 10],
                ..Default::default()
            }),
            "exactly 10 zones still answers"
        );
    }

    /// **AC 5 (search strings): oversized zone or string lists get no answer.**
    #[test]
    fn more_than_four_search_strings_is_oversized() {
        assert!(is_oversized(&CMSG_WHO {
            search_strings: vec![String::new(); 5],
            ..Default::default()
        }));
        assert!(
            !is_oversized(&CMSG_WHO {
                search_strings: vec![String::new(); 4],
                ..Default::default()
            }),
            "exactly 4 search strings still answers"
        );
    }
}
