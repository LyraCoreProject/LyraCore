use lyracore_shared::faction;
use wow_world_messages::vanilla::CMSG_WHO;

const MAX_ZONES: usize = 10;
const MAX_SEARCH_STRINGS: usize = 4;
const NO_UPPER_BOUND_AT: u8 = 100;

pub struct WhoCandidate<'a> {
    pub name: &'a str,
    pub guild_name: &'a str,
    pub race: u8,
    pub class: u8,
    pub level: u8,
    pub zone_id: u32,
    pub zone_name: &'a str,
}

pub struct WhoFilter<'a> {
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

fn name_matches(filter: &str, value: &str) -> bool {
    filter.is_empty()
        || value
            .to_ascii_lowercase()
            .contains(&filter.to_ascii_lowercase())
}

fn search_strings_match(strings: &[String], candidate: &WhoCandidate) -> bool {
    let real_strings = strings.iter().filter(|s| !s.is_empty());
    let mut any_real = false;
    for s in real_strings {
        any_real = true;
        if name_matches(s, candidate.name)
            || name_matches(s, candidate.guild_name)
            || name_matches(s, candidate.zone_name)
        {
            return true;
        }
    }
    !any_real
}

pub fn matches(filter: &WhoFilter, candidate: &WhoCandidate) -> bool {
    faction::team_for_race(candidate.race) == filter.requester_team
        && candidate.level >= filter.min_level
        && candidate.level <= filter.max_level
        && filter.class_mask & 1u32.checked_shl(u32::from(candidate.class)).unwrap_or(0) != 0
        && filter.race_mask & 1u32.checked_shl(u32::from(candidate.race)).unwrap_or(0) != 0
        && (filter.zones.is_empty() || filter.zones.contains(&candidate.zone_id))
        && name_matches(filter.name, candidate.name)
        && name_matches(filter.guild_name, candidate.guild_name)
        && search_strings_match(filter.search_strings, candidate)
}

fn is_oversized(request: &CMSG_WHO) -> bool {
    request.zones.len() > MAX_ZONES || request.search_strings.len() > MAX_SEARCH_STRINGS
}

/// A wire maximum level of 100 or more means no upper bound.
fn effective_max_level(wire_max_level: u8) -> u8 {
    if wire_max_level >= NO_UPPER_BOUND_AT {
        u8::MAX
    } else {
        wire_max_level
    }
}

impl<'a> WhoFilter<'a> {
    pub fn from_request(requester_race: u8, request: &'a CMSG_WHO) -> Option<Self> {
        if is_oversized(request) {
            return None;
        }
        Some(Self {
            requester_team: faction::team_for_race(requester_race),
            min_level: request.minimum_level.as_int(),
            max_level: effective_max_level(request.maximum_level.as_int()),
            name: &request.player_name,
            guild_name: &request.guild_name,
            race_mask: request.race_mask,
            class_mask: request.class_mask,
            zones: &request.zones,
            search_strings: &request.search_strings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::faction::{TEAM_ALLIANCE, TEAM_HORDE};
    use wow_world_messages::vanilla::Level;

    fn baseline() -> (WhoFilter<'static>, WhoCandidate<'static>) {
        let filter = WhoFilter {
            requester_team: TEAM_ALLIANCE,
            min_level: 1,
            max_level: 60,
            name: "",
            guild_name: "",
            race_mask: 1 << 1,
            class_mask: 1 << 1,
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
    fn unknown_class_or_race_bits_do_not_match() {
        let (filter, mut candidate) = baseline();
        candidate.class = 32;
        assert!(!matches(&filter, &candidate));
        candidate.class = 1;
        candidate.race = 32;
        assert!(!matches(&filter, &candidate));
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
        candidate.race = 2;
        filter.requester_team = TEAM_ALLIANCE;
        assert!(!matches(&filter, &candidate));
    }

    #[test]
    fn a_horde_requester_matches_a_horde_candidate() {
        let (mut filter, mut candidate) = baseline();
        filter.requester_team = TEAM_HORDE;
        filter.race_mask = 1 << 2;
        candidate.race = 2;
        assert!(matches(&filter, &candidate));
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
        let request = CMSG_WHO {
            minimum_level: Level::new(1),
            maximum_level: Level::new(100),
            race_mask: 1 << 1,
            class_mask: 1 << 1,
            ..Default::default()
        };
        let filter = WhoFilter::from_request(1, &request).expect("a bounded request answers");
        let (_, mut candidate) = baseline();
        candidate.level = 150;
        assert!(matches(&filter, &candidate));
    }

    #[test]
    fn class_mask_is_a_bit_test() {
        let (mut filter, candidate) = baseline();
        filter.class_mask = 1 << 2;
        assert!(!matches(&filter, &candidate));
    }

    #[test]
    fn race_mask_is_a_bit_test() {
        let (mut filter, candidate) = baseline();
        filter.race_mask = 1 << 3;
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
    fn any_non_empty_search_string_matching_is_enough() {
        let (mut filter, candidate) = baseline();
        let strings = ["ging".to_string(), "nowhere".to_string()];
        filter.search_strings = &strings;
        assert!(
            matches(&filter, &candidate),
            "\"ging\" matches the name even though \"nowhere\" matches nothing"
        );
    }

    #[test]
    fn a_search_string_that_matches_nothing_at_all_excludes_the_row() {
        let (mut filter, candidate) = baseline();
        let strings = ["nowhere".to_string()];
        filter.search_strings = &strings;
        assert!(!matches(&filter, &candidate));
    }

    #[test]
    fn every_string_empty_matches_every_row() {
        let (mut filter, candidate) = baseline();
        let strings = [String::new(), String::new()];
        filter.search_strings = &strings;
        assert!(matches(&filter, &candidate));
    }

    #[test]
    fn effective_max_level_removes_the_upper_bound_at_100_and_above() {
        assert_eq!(effective_max_level(99), 99);
        assert_eq!(effective_max_level(100), u8::MAX);
        assert_eq!(effective_max_level(255), u8::MAX);
    }

    #[test]
    fn more_than_ten_zones_is_oversized() {
        let zones = |count| CMSG_WHO {
            zones: vec![0; count],
            ..Default::default()
        };
        assert!(WhoFilter::from_request(1, &zones(11)).is_none());
        assert!(
            WhoFilter::from_request(1, &zones(10)).is_some(),
            "exactly 10 zones still answers"
        );
    }

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
