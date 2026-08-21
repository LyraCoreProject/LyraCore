//! Proleptic-Gregorian calendar arithmetic for the realm clock.
//!
//! Both tiers read the same wall clock and must agree about what day it is: the gateway packs the
//! date into `SMSG_LOGIN_SETTIMESPEED`, and the module derives the weather season from it. A realm
//! whose sky and whose clock disagreed about the date would be harder to explain than either being
//! wrong on its own, so the conversion lives here once.
//!
//! Pure integer math with no clock read of its own — each caller supplies its own timestamp, and
//! the realm clock is UTC.

/// `(year, month 1..=12, day 1..=31)` for a count of days since 1970-01-01.
///
/// Howard Hinnant's `civil_from_days` (<http://howardhinnant.github.io/date_algorithms.html>). The
/// era arithmetic shifts the year to start in March so the leap day lands last and needs no special
/// case. Exact for every timestamp a realm can hold, including dates before 1970.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468; // days since 0000-03-01
    let era = shifted.div_euclid(146_097); // one 400-year cycle
    let day_of_era = shifted - era * 146_097; // 0..=146096
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // 0..=399
    let march_year = year_of_era + era * 400;
    let day_of_march_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_march_year + 2) / 153; // 0 is March
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Worked dates rather than the algorithm run backwards: the epoch itself, the era arithmetic's
    /// own March hinge, both leap-year rules (2000 is a leap year, 1900 is not), and a day before
    /// the epoch.
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
