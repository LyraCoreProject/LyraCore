use lyracore_shared::calendar::civil_from_days;
use lyracore_shared::game_event::{
    DARKMOON_FAIRE_ELWYNN, DARKMOON_FAIRE_ELWYNN_BUILDING, DARKMOON_FAIRE_EVENTS,
    DARKMOON_FAIRE_MULGORE, DARKMOON_FAIRE_MULGORE_BUILDING,
};

use super::{GameEventState, DAY_SECS};

/// The state the Faire's schedule gives `event_entry` at `now_secs`, or `None` for an event that
/// is not one of the Faire's.
pub(super) fn state(event_entry: u32, now_secs: i64) -> Option<GameEventState> {
    DARKMOON_FAIRE_EVENTS.contains(&event_entry).then(|| {
        if event_at(now_secs) == Some(event_entry) {
            GameEventState::Active
        } else {
            GameEventState::Inactive
        }
    })
}

/// vmangos `DarkmoonFaire::GetDarkmoonState` on the UTC date; docs/schema.md#game-events.
fn event_at(now_secs: i64) -> Option<u32> {
    let days = now_secs.div_euclid(DAY_SECS);
    let (_, month, day) = civil_from_days(days);
    let day = i64::from(day);
    // 1970-01-01 was a Thursday; 0 is Sunday, as in C `tm_wday`.
    let weekday = (days + 4).rem_euclid(7);
    let first_weekday = (weekday - (day - 1)).rem_euclid(7);
    let first_monday = (8 - first_weekday) % 7 + 1;
    let (building, open) = if month % 2 == 0 {
        (DARKMOON_FAIRE_ELWYNN_BUILDING, DARKMOON_FAIRE_ELWYNN)
    } else {
        (DARKMOON_FAIRE_MULGORE_BUILDING, DARKMOON_FAIRE_MULGORE)
    };
    if day + 3 < first_monday {
        None
    } else if day < first_monday {
        Some(building)
    } else if day < first_monday + 7 {
        Some(open)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-12-07 00:00 UTC, the first Monday of December 2026.
    const DECEMBER_FIRST_MONDAY: i64 = 1_796_601_600;
    // 2027-01-04 00:00 UTC, the first Monday of January 2027.
    const JANUARY_FIRST_MONDAY: i64 = 1_799_020_800;
    // 2027-02-01 00:00 UTC, a Monday.
    const FEBRUARY_FIRST: i64 = 1_801_440_000;

    #[test]
    fn the_faire_builds_for_three_days_then_opens_for_a_week_from_the_first_monday() {
        let friday = DECEMBER_FIRST_MONDAY - 3 * DAY_SECS;
        let next_monday = DECEMBER_FIRST_MONDAY + 7 * DAY_SECS;
        assert_eq!(event_at(friday - 1), None);
        assert_eq!(event_at(friday), Some(DARKMOON_FAIRE_ELWYNN_BUILDING));
        assert_eq!(
            event_at(DECEMBER_FIRST_MONDAY - 1),
            Some(DARKMOON_FAIRE_ELWYNN_BUILDING)
        );
        assert_eq!(event_at(DECEMBER_FIRST_MONDAY), Some(DARKMOON_FAIRE_ELWYNN));
        assert_eq!(event_at(next_monday - 1), Some(DARKMOON_FAIRE_ELWYNN));
        assert_eq!(event_at(next_monday), None);
    }

    #[test]
    fn the_faire_alternates_between_elwynn_and_mulgore_by_month() {
        let new_year = JANUARY_FIRST_MONDAY - 3 * DAY_SECS;
        assert_eq!(event_at(new_year - 1), None);
        assert_eq!(event_at(new_year), Some(DARKMOON_FAIRE_MULGORE_BUILDING));
        assert_eq!(event_at(JANUARY_FIRST_MONDAY), Some(DARKMOON_FAIRE_MULGORE));
        assert_eq!(event_at(FEBRUARY_FIRST), Some(DARKMOON_FAIRE_ELWYNN));
    }

    #[test]
    fn a_faire_opening_on_the_first_builds_on_no_day_of_the_month_before() {
        assert_eq!(event_at(FEBRUARY_FIRST - 3 * DAY_SECS), None);
        assert_eq!(event_at(FEBRUARY_FIRST - 1), None);
        assert_eq!(event_at(FEBRUARY_FIRST), Some(DARKMOON_FAIRE_ELWYNN));
    }

    #[test]
    fn only_the_event_the_schedule_names_runs_and_other_events_are_not_the_faires() {
        use GameEventState::*;
        assert_eq!(
            state(DARKMOON_FAIRE_ELWYNN, DECEMBER_FIRST_MONDAY),
            Some(Active)
        );
        for other in [
            DARKMOON_FAIRE_ELWYNN_BUILDING,
            DARKMOON_FAIRE_MULGORE,
            DARKMOON_FAIRE_MULGORE_BUILDING,
        ] {
            assert_eq!(state(other, DECEMBER_FIRST_MONDAY), Some(Inactive));
        }
        assert_eq!(state(13, DECEMBER_FIRST_MONDAY), None);
    }
}
