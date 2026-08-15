//! Guild packets: the query response, the info panel, and the one error channel.

use lyracore_shared::guild::GUILD_RANK_COUNT;
use wow_world_messages::vanilla::{
    GuildCommand, GuildCommandResult, SMSG_GUILD_COMMAND_RESULT, SMSG_GUILD_INFO,
    SMSG_GUILD_QUERY_RESPONSE,
};

/// Build `SMSG_GUILD_QUERY_RESPONSE` for a guild the client asked about by id.
///
/// `rank_names` is a fixed `[String; 10]` on the wire, so it is filled positionally: a guild that
/// somehow carries fewer rank rows renders the missing slots empty rather than panicking here. The
/// invariant that it never happens is seeded module-side (`guild::seed_ranks`), which is where a
/// violation is a bug worth failing; a half-built packet at this boundary would only take the
/// client down.
///
/// Emblems are all-zero: the tabard designer is not part of this system.
pub fn build_guild_query_response(
    guild_id: u64,
    name: &str,
    rank_names: &[String],
) -> SMSG_GUILD_QUERY_RESPONSE {
    SMSG_GUILD_QUERY_RESPONSE {
        id: guild_id as u32,
        name: name.to_string(),
        rank_names: std::array::from_fn::<String, GUILD_RANK_COUNT, _>(|i| {
            rank_names.get(i).cloned().unwrap_or_default()
        }),
        emblem_style: 0,
        emblem_color: 0,
        border_style: 0,
        border_color: 0,
        background_color: 0,
    }
}

/// Build `SMSG_GUILD_INFO` — the guild-information panel: the name, the founding date split into
/// day/month/year, and how many characters are in it.
///
/// `amount_of_accounts_in_guild` reports the character count too. Accounts are not resolvable from
/// realm-core (it holds no character rows to join through), and vanilla's client only renders the
/// number, so reporting the characters is the honest approximation rather than a zero.
pub fn build_guild_info(name: &str, created_micros: i64, member_count: u32) -> SMSG_GUILD_INFO {
    let (created_day, created_month, created_year) = civil_date(created_micros);
    SMSG_GUILD_INFO {
        guild_name: name.to_string(),
        created_day,
        created_month,
        created_year,
        amount_of_characters_in_guild: member_count,
        amount_of_accounts_in_guild: member_count,
    }
}

/// Build `SMSG_GUILD_COMMAND_RESULT` — the ONLY guild error channel. A guild refusal is never a
/// system chat line.
///
/// `string` is the subject the client interpolates into its own localized text (the guild name for
/// a create, the player name for an invite). Success is `GuildCommandResult::PlayerNoMoreInGuild`,
/// which is the wire's code 0: vanilla's "no message/error", not a refusal.
pub fn build_guild_command_result(
    command: GuildCommand,
    subject: &str,
    result: GuildCommandResult,
) -> SMSG_GUILD_COMMAND_RESULT {
    SMSG_GUILD_COMMAND_RESULT {
        command,
        string: subject.to_string(),
        result,
    }
}

/// Unix-epoch micros → `(day, month, year)` in UTC, in the shape `SMSG_GUILD_INFO` wants: the day
/// and month are ZERO-BASED (vanilla's own encoding — the client adds one before rendering) and the
/// year is the full year.
///
/// Days-from-epoch to a civil date by Howard Hinnant's `civil_from_days`, which is exact for every
/// value in range and needs no date crate (the gateway has none).
fn civil_date(micros: i64) -> (u32, u32, u32) {
    let secs = micros.div_euclid(1_000_000);
    let days = secs.div_euclid(86_400);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };

    ((d - 1) as u32, (m - 1) as u32, y as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rank-name array is the fixed-size field the whole ten-rank invariant exists for: ten
    /// names in, ten names out, in rank order.
    #[test]
    fn the_query_response_carries_all_ten_rank_names_in_rank_order() {
        let ranks: Vec<String> = (0..10).map(|i| format!("Rank{i}")).collect();
        let m = build_guild_query_response(7, "The Silver Hand", &ranks);
        assert_eq!(m.id, 7);
        assert_eq!(m.name, "The Silver Hand");
        assert_eq!(m.rank_names[0], "Rank0");
        assert_eq!(m.rank_names[9], "Rank9");
    }

    /// A short rank list must not panic at packet build: it pads. The invariant is enforced where
    /// the rows are written, not by taking the client down.
    #[test]
    fn a_short_rank_list_pads_rather_than_panicking() {
        let m = build_guild_query_response(1, "Short", &["Only".to_string()]);
        assert_eq!(m.rank_names[0], "Only");
        assert!(m.rank_names[1..].iter().all(String::is_empty));
    }

    /// The founding date is what the info panel renders. Day and month are zero-based on the wire.
    #[test]
    fn the_info_panel_splits_the_founding_date_into_a_zero_based_day_and_month() {
        // 2026-08-15T00:00:00Z = 20680 days after the epoch.
        let micros = 20_680i64 * 86_400 * 1_000_000;
        let m = build_guild_info("The Silver Hand", micros, 3);
        assert_eq!(
            (m.created_day, m.created_month, m.created_year),
            (14, 7, 2026)
        );
        assert_eq!(m.amount_of_characters_in_guild, 3);
    }

    #[test]
    fn the_epoch_itself_is_the_first_of_january_1970() {
        assert_eq!(civil_date(0), (0, 0, 1970));
        assert_eq!(civil_date(-1), (30, 11, 1969)); // one microsecond earlier is 1969-12-31
    }
}
