// Howard Hinnant civil_from_days, with a March-based year.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468; // Days since 0000-03-01.
    let era = shifted.div_euclid(146_097); // One 400-year cycle.
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let march_year = year_of_era + era * 400;
    let day_of_march_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_march_year + 2) / 153; // Month 0 is March.
    let day = (day_of_march_year - (153 * march_month + 2) / 5 + 1) as u32;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    } as u32;
    let year = if month <= 2 {
        march_year + 1
    } else {
        march_year
    };
    (year, month, day)
}

/// Days since 1970-01-01 of a proleptic Gregorian date; Howard Hinnant days_from_civil.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let march_year = if month <= 2 { year - 1 } else { year };
    let era = march_year.div_euclid(400);
    let year_of_era = march_year - era * 400;
    let march_month = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let day_of_march_year = (153 * march_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_march_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates_convert_to_their_day_counts() {
        for (date, expected) in [
            ((1970, 1, 1), 0),
            ((1970, 3, 1), 59),
            ((2000, 2, 29), 11_016),
            ((1900, 2, 28), -25_509),
            ((1900, 3, 1), -25_508),
            ((1969, 12, 31), -1),
            ((2026, 1, 1), 20_454),
        ] {
            let (year, month, day) = date;
            assert_eq!(days_from_civil(year, month, day), expected, "{date:?}");
        }
    }

    #[test]
    fn known_dates_convert_from_their_day_counts() {
        for (days, expected) in [
            (0, (1970, 1, 1)),
            (59, (1970, 3, 1)),
            (11_016, (2000, 2, 29)),
            (-25_509, (1900, 2, 28)),
            (-25_508, (1900, 3, 1)),
            (-1, (1969, 12, 31)),
            (20_454, (2026, 1, 1)),
        ] {
            assert_eq!(civil_from_days(days), expected, "day count {days}");
        }
    }
}
