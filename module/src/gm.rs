//! GM playtest dot-commands (work-item 223): `.speed/.god/.xprate/.level/.money/.heal/.kill/.tele`
//! from Say chat. The gateway's `CMSG_MESSAGECHAT` Say arm intercepts any message starting with `.`
//! BEFORE the normal chat relay/insert and forwards the raw text to this ONE generic reducer
//! (`gm_command`) — module-side parsing keeps the command set data-free and easily extended (a new
//! command is a new `match` arm here, no gateway/binding change). A nonzero `Character.gm_level`
//! authorizes the full set. Account-owned Alpha Test Tools authorize the classified subset, which
//! currently contains only `.speed`. The Gateway reads that Account authority from Realm-core on
//! every command and conveys it to the Home Shard; this Module remains the final Gate.
//!
//! `.kill` deliberately takes NO explicit target argument: the caller's live `WorldEntity.target_guid`
//! is already tracked server-side by `world::set_target` (`CMSG_SET_SELECTION`) every time the client
//! changes selection, so the reducer just reads its own caller's current target — no extra gateway
//! plumbing (and no guid-through-JSON hazard) needed.

use spacetimedb::{log, reducer, table, ReducerContext, Table};

use crate::helpers::{require_operator};
use crate::{game_character, game_world_entity};

// ===========================================================================================
//  `.xprate` config singleton [static] — module-only, NEVER gateway-subscribed (no binding work)
// ===========================================================================================

/// A single named world tunable, keyed by `key` (today just `"xprate"`). Module-only — no client
/// field maps to any row here, so it is never added to the gateway's subscription list and needs no
/// binding/parity work despite being a brand-new table (docs/danger-zones.md §1.6: a `String`
/// PRIMARY KEY is fine; only a `String` `#[default(...)]` on an END-appended column of an EXISTING
/// table is impossible — this is a FRESH table with no existing rows, so no column needs a default
/// at all). Public so `spacetime sql` can read/tune it directly, like `game_config`/`game_debug_readout`.
#[table(accessor = game_world_config, public)]
pub struct GmWorldConfig {
    #[primary_key]
    pub key: String,
    pub value: u32,
}

/// The `game_world_config` key for the `.xprate` basis-points multiplier (10000 = 1.0×, the same
/// scale `WorldEntity::run_speed_mult_bp` uses).
const XPRATE_KEY: &str = "xprate";
/// Missing row ⇒ 1× (Blizzlike) — read-with-default, so an untouched/fresh DB behaves exactly as if
/// no GM had ever run `.xprate`.
const XPRATE_DEFAULT_BP: u32 = 10_000;

/// Read the current `.xprate` basis-points multiplier (10000 = 1×). Missing row ⇒ the default.
pub(crate) fn xprate_bp(ctx: &ReducerContext) -> u32 {
    ctx.db
        .game_world_config()
        .key()
        .find(XPRATE_KEY.to_string())
        .map(|r| r.value)
        .unwrap_or(XPRATE_DEFAULT_BP)
}

// ===========================================================================================
//  `.tele` destination table [pure] — a small const list, not a DB table (nothing to migrate)
// ===========================================================================================

/// A named `.tele` destination. Case-insensitive name match in [`TeleSpot::parse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TeleSpot {
    Goldshire,
    Northshire,
    SentinelHill,
    WestfallCoast,
    /// The ORC start, on map 1 — and the only `.tele` that crosses a continent. Kalimdor lives on
    /// its own database, so this destination drives a full escrowed transfer rather than a
    /// same-map hop; see `coords`.
    ValleyOfTrials,
}

impl TeleSpot {
    /// Case-insensitive name → spot. `None` for an unrecognized name (the caller turns that into the
    /// "unknown location" `Err`). Pure.
    fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "goldshire" => Some(Self::Goldshire),
            "northshire" => Some(Self::Northshire),
            "sentinel_hill" | "sentinelhill" => Some(Self::SentinelHill),
            "westfall_coast" | "westfallcoast" => Some(Self::WestfallCoast),
            "valley" | "valleyoftrials" | "valley_of_trials" => Some(Self::ValleyOfTrials),
            _ => None,
        }
    }

    /// `(map_id, x, y, z, orientation)` — the SAME coordinates as `world::graveyard`'s hardcoded
    /// fallback consts (world_safe_locs 105/106, plus the [V]-tagged Sentinel Hill/Westfall Coast
    /// estimates). Duplicated here rather than shared (that `graveyard` submodule is private to
    /// `world.rs`) — matching this codebase's own convention: `seed.rs` ALSO independently row-seeds
    /// these same five points into `game_graveyard` (see its doc, "seed.rs also row-seeds these five").
    /// The first four are on map 0 (Eastern Kingdoms) — same-map moves through the existing
    /// `world::teleport_player` core. `ValleyOfTrials` is on map 1, which the shard map routes to
    /// another database, so that one is a CROSS-CONTINENT `.tele`: the same primitive drives it, but
    /// what follows is the full escrowed transfer (loading screen, import, attest, release), not a
    /// reposition. It was safe to add only once Kalimdor had imported content — the reason the
    /// Deadmines destination is still absent is unchanged (map 36 has no world data to land in).
    pub(crate) fn coords(self) -> (u32, f32, f32, f32, f32) {
        match self {
            // world_safe_locs 105 — Northshire Abbey.
            Self::Northshire => (0, -8935.33, -188.646, 80.4165, 2.72271),
            // world_safe_locs 106 — Goldshire.
            Self::Goldshire => (0, -9339.59, 171.73, 63.5258, 0.0),
            // world_safe_locs ≈80 [V] — Sentinel Hill (Westfall). Unverified estimate (no cmangos
            // dump in this sandbox); confirm against a real dump before relying on this live.
            Self::SentinelHill => (0, -10650.0, 1180.0, 34.0, 0.0),
            // world_safe_locs ≈81 [V] — Westfall coast. Same provenance caveat as SentinelHill.
            Self::WestfallCoast => (0, -11390.0, 1590.0, 6.0, 0.0),
            // The orc start position, straight out of `game_start_position` (race 2) rather than a
            // hand-estimated point — Durotar is imported, so this one is data-backed.
            Self::ValleyOfTrials => (1, -618.518, -4251.67, 38.718, 0.0),
        }
    }
}

// ===========================================================================================
//  Command parsing [pure] — module-side, data-free (the gateway hands over the raw dot-string)
// ===========================================================================================

/// One parsed `.command`, ready to dispatch. `God(None)` = toggle (no arg); `Speed`/`XpRate`/`Level`
/// carry their ALREADY-CLAMPED value (parsing clamps silently rather than rejecting an out-of-range
/// number — only a non-numeric/missing arg or an unknown command name is a hard `Err`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum GmCommand {
    /// Run-speed multiplier, clamped to `0.5..=10.0`.
    Speed(f32),
    /// `None` = toggle; `Some(v)` = the explicit `on`/`off` state.
    God(Option<bool>),
    /// XP-rate multiplier (whole number), clamped to `1..=20`.
    XpRate(u32),
    /// Target level, clamped to `1..=60`.
    Level(u32),
    /// Purse amount in copper (unclamped — any `u32` is a legal purse).
    Money(u32),
    Heal,
    Kill,
    /// Report the caller's own position (map/x/y/z/orientation) back as a system line — a read-only
    /// world-building lever (place NPCs/spawns at coords you're standing on). Reported via the Err
    /// channel since an accepted command has no success reply.
    Gps,
    Tele(TeleSpot),
    /// Grant `count` of item `entry` to the caller's bags (playtest lever — testing gear/stat
    /// changes without vendor runs). Count defaults to 1, clamped to `1..=20`.
    AddItem(u32, u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GmCommandClass {
    AlphaTestTool,
    FullGm,
}

impl GmCommand {
    fn class(self) -> GmCommandClass {
        match self {
            Self::Speed(_) => GmCommandClass::AlphaTestTool,
            _ => GmCommandClass::FullGm,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GmAuthority {
    None,
    AlphaTestTools,
    FullGm,
}

fn gm_authority(gm_level: u8, alpha_test_tools: bool) -> GmAuthority {
    if gm_level != 0 {
        GmAuthority::FullGm
    } else if alpha_test_tools {
        GmAuthority::AlphaTestTools
    } else {
        GmAuthority::None
    }
}

fn command_is_authorized(authority: GmAuthority, command: GmCommand) -> bool {
    match (authority, command.class()) {
        (GmAuthority::FullGm, _) | (GmAuthority::AlphaTestTools, GmCommandClass::AlphaTestTool) => {
            true
        }
        (GmAuthority::None | GmAuthority::AlphaTestTools, GmCommandClass::FullGm)
        | (GmAuthority::None, GmCommandClass::AlphaTestTool) => false,
    }
}

fn authority_accepts_command_name(authority: GmAuthority, text: &str) -> bool {
    match authority {
        GmAuthority::None => false,
        GmAuthority::AlphaTestTools => text.split_whitespace().next() == Some(".speed"),
        GmAuthority::FullGm => true,
    }
}

/// Parse a raw dot-command string (`".speed 3"`, `".god"`, …) into a [`GmCommand`]. Pure — no
/// `ReducerContext`, fully unit-testable. Args are space-separated (`split_whitespace`, so repeated
/// spaces are tolerant). An out-of-range numeric arg is silently CLAMPED into the command's valid
/// range (not rejected); a non-numeric/missing arg, an unrecognized `.tele` name, or an unknown
/// command name is a hard `Err` with a user-facing message (relayed back to the sender as a system
/// chat line by the gateway on a reducer `Err`).
pub(crate) fn parse_gm_command(text: &str) -> Result<GmCommand, String> {
    let body = text
        .strip_prefix('.')
        .ok_or_else(|| "not a command".to_string())?;
    let mut parts = body.split_whitespace();
    let cmd = parts.next().ok_or_else(|| "empty command".to_string())?;
    let rest: Vec<&str> = parts.collect();
    match cmd {
        "speed" => {
            let arg = rest
                .first()
                .ok_or_else(|| "usage: .speed <mult>".to_string())?;
            let mult: f32 = arg
                .parse()
                .ok()
                .filter(|v: &f32| v.is_finite())
                .ok_or_else(|| format!("bad speed value: {arg}"))?;
            Ok(GmCommand::Speed(mult.clamp(0.5, 10.0)))
        }
        "god" => match rest.first().map(|s| s.to_ascii_lowercase()) {
            None => Ok(GmCommand::God(None)),
            Some(s) if s == "on" => Ok(GmCommand::God(Some(true))),
            Some(s) if s == "off" => Ok(GmCommand::God(Some(false))),
            Some(other) => Err(format!("usage: .god [on|off], got '{other}'")),
        },
        "xprate" => {
            let arg = rest
                .first()
                .ok_or_else(|| "usage: .xprate <mult>".to_string())?;
            let mult: u32 = arg
                .parse()
                .map_err(|_| format!("bad xprate value: {arg}"))?;
            Ok(GmCommand::XpRate(mult.clamp(1, 20)))
        }
        "level" => {
            let arg = rest
                .first()
                .ok_or_else(|| "usage: .level <n>".to_string())?;
            let n: u32 = arg.parse().map_err(|_| format!("bad level value: {arg}"))?;
            Ok(GmCommand::Level(n.clamp(1, 60)))
        }
        "money" => {
            let arg = rest
                .first()
                .ok_or_else(|| "usage: .money <copper>".to_string())?;
            let copper: u32 = arg.parse().map_err(|_| format!("bad money value: {arg}"))?;
            Ok(GmCommand::Money(copper))
        }
        "heal" => Ok(GmCommand::Heal),
        "gps" | "pos" | "where" => Ok(GmCommand::Gps),
        "additem" => {
            let entry: u32 = rest
                .first()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "usage: .additem <item_entry> [count]".to_string())?;
            let count: u32 = rest
                .get(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(1)
                .clamp(1, 20);
            Ok(GmCommand::AddItem(entry, count))
        }
        "kill" => Ok(GmCommand::Kill),
        "tele" => {
            let arg = rest
                .first()
                .ok_or_else(|| "usage: .tele <name>".to_string())?;
            let spot = TeleSpot::parse(arg).ok_or_else(|| format!("unknown location: {arg}"))?;
            Ok(GmCommand::Tele(spot))
        }
        other => Err(format!("unknown command: .{other}")),
    }
}

// ===========================================================================================
//  Reducers
// ===========================================================================================

/// Grant/revoke GM level (operator-only CLI reducer, work-item 223 authz): `level` gates
/// [`gm_command`] (`0` = no access — the moderation-facing per-level distinctions beyond "has
/// access at all" are work-item 205's concern, not this one's). Looked up by character NAME
/// (case-insensitive, matching `send_whisper`'s convention) since an operator drives this via
/// `spacetime call`, which has no live guid to hand. `require_operator` gates it exactly like the
/// importer reducers — never player-callable.
#[reducer]
pub fn set_gm_level(ctx: &ReducerContext, character_name: String, level: u8) -> Result<(), String> {
    require_operator(ctx)?;
    let chars = ctx.db.game_character();
    // REFUSE verdict (issue #30): an in-transit character reads as absent, so this operator write
    // cannot land on a source copy the destination already serialized past.
    let mut c = crate::helpers::character_by_name(ctx, &character_name)
        .ok_or_else(|| format!("no player named {character_name}"))?;
    let guid = c.guid;
    c.gm_level = level;
    chars.guid().update(c);
    log::info!("set_gm_level: {character_name} (guid {guid}) -> gm_level {level}");
    Ok(())
}

/// The shared core behind the gateway's `gw_gm_command`. The Character's current GM level and the
/// Account authority conveyed by the Gateway meet at this final Module Gate.
pub(crate) fn apply_gm_command(
    ctx: &ReducerContext,
    caller: crate::WorldEntity,
    alpha_test_tools: bool,
    text: String,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut caller = caller;
    let gm_level = ctx
        .db
        .game_character()
        .guid()
        .find(caller.guid)
        .map(|c| c.gm_level)
        .unwrap_or(0);
    let authority = gm_authority(gm_level, alpha_test_tools);
    if !authority_accepts_command_name(authority, &text) {
        return Err("permission denied".to_string());
    }
    let cmd = parse_gm_command(&text)?;
    if !command_is_authorized(authority, cmd) {
        return Err("permission denied".to_string());
    }
    log::info!(
        "gm_command: guid {} ({authority:?}) -> {cmd:?}",
        caller.guid
    );
    match cmd {
        GmCommand::Speed(mult) => {
            caller.run_speed_mult_bp = (mult * 10_000.0).round() as u32;
            entities.guid().update(caller);
        }
        GmCommand::God(explicit) => {
            caller.godmode = explicit.unwrap_or(!caller.godmode);
            entities.guid().update(caller);
        }
        GmCommand::XpRate(mult) => {
            let cfg = ctx.db.game_world_config();
            let bp = mult * 10_000;
            cfg.key().delete(XPRATE_KEY.to_string()); // upsert
            cfg.insert(GmWorldConfig {
                key: XPRATE_KEY.to_string(),
                value: bp,
            });
        }
        GmCommand::Level(level) => {
            crate::stats::set_character_level(ctx, caller.guid, level)?;
        }
        GmCommand::Money(copper) => {
            let guid = caller.guid;
            caller.money = copper;
            entities.guid().update(caller);
            let chars = ctx.db.game_character();
            if let Some(mut c) = chars.guid().find(guid) {
                c.money = copper;
                chars.guid().update(c);
            }
        }
        GmCommand::Heal => {
            caller.health = caller.max_health;
            caller.power = caller.max_power;
            entities.guid().update(caller);
        }
        GmCommand::Gps => {
            // Read-only: report position via the Err channel (the only caller-facing text path — an
            // accepted command has no success reply). No mutation, so the Err rollback is a no-op.
            return Err(format!(
                "Position: map {} | x {:.2} y {:.2} z {:.2} | o {:.3}",
                caller.map_id, caller.x, caller.y, caller.z, caller.orientation
            ));
        }
        GmCommand::AddItem(entry, count) => {
            // Reuses the debug/quest grant path — bag-space + stack rules enforced there; an
            // unknown entry or full bags surface as the Err the sender sees as a system message.
            crate::items::grant_item(ctx, caller.guid, entry, count)?;
        }
        GmCommand::Kill => {
            let target_guid = caller.target_guid;
            if target_guid == 0 {
                return Err("no target selected".to_string());
            }
            let target = entities
                .guid()
                .find(target_guid)
                .ok_or_else(|| "target not found".to_string())?;
            if target.is_player() {
                return Err("cannot kill a player".to_string());
            }
            if !crate::combat::kill_creature(ctx, target_guid, Some(caller.guid)) {
                return Err("target is not a valid kill (already dead / invalid)".to_string());
            }
        }
        GmCommand::Tele(spot) => {
            let (map_id, x, y, z, o) = spot.coords();
            // `map_id` comes from the destination, so a cross-map `.tele` (ValleyOfTrials, map 1)
            // rides the same primitive and becomes a full escrowed transfer to whichever database
            // owns that map. Deadmines is still deliberately absent: map 36 has no imported world
            // data, so it would .tele into an empty room.
            crate::world::teleport_player(ctx, caller.guid, map_id, 0, x, y, z, o);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_test_tools_authorize_speed_only() {
        let authority = gm_authority(0, true);
        assert!(authority_accepts_command_name(authority, ".speed 3"));
        assert!(!authority_accepts_command_name(authority, ".tele goldshire"));
        assert!(!authority_accepts_command_name(authority, ".unknown"));
        assert!(command_is_authorized(authority, GmCommand::Speed(3.0)));
        for command in [
            GmCommand::God(None),
            GmCommand::XpRate(2),
            GmCommand::Level(10),
            GmCommand::Money(1),
            GmCommand::Heal,
            GmCommand::Kill,
            GmCommand::Gps,
            GmCommand::Tele(TeleSpot::Goldshire),
            GmCommand::AddItem(1, 1),
        ] {
            assert!(
                !command_is_authorized(authority, command),
                "Alpha Test Tools unexpectedly authorized {command:?}"
            );
        }
    }

    #[test]
    fn a_full_gm_keeps_every_command() {
        let authority = gm_authority(1, false);
        assert!(authority_accepts_command_name(authority, ".unknown"));
        for command in [
            GmCommand::Speed(3.0),
            GmCommand::God(None),
            GmCommand::XpRate(2),
            GmCommand::Level(10),
            GmCommand::Money(1),
            GmCommand::Heal,
            GmCommand::Kill,
            GmCommand::Gps,
            GmCommand::Tele(TeleSpot::Goldshire),
            GmCommand::AddItem(1, 1),
        ] {
            assert!(command_is_authorized(authority, command));
        }
    }

    #[test]
    fn an_ordinary_account_has_no_command_authority() {
        let authority = gm_authority(0, false);
        assert!(!authority_accepts_command_name(authority, ".speed 3"));
        assert!(!command_is_authorized(authority, GmCommand::Speed(3.0)));
        assert!(!command_is_authorized(authority, GmCommand::God(None)));
    }

    #[test]
    fn full_gm_authority_wins_when_the_account_also_has_alpha_test_tools() {
        assert_eq!(gm_authority(1, true), GmAuthority::FullGm);
    }

    // ---- Parsing: happy path, one per command ----

    #[test]
    fn parses_speed_in_range() {
        assert_eq!(parse_gm_command(".speed 3"), Ok(GmCommand::Speed(3.0)));
    }

    #[test]
    fn parses_god_toggle_and_explicit_states() {
        assert_eq!(parse_gm_command(".god"), Ok(GmCommand::God(None)));
        assert_eq!(parse_gm_command(".god on"), Ok(GmCommand::God(Some(true))));
        assert_eq!(
            parse_gm_command(".god off"),
            Ok(GmCommand::God(Some(false)))
        );
        assert_eq!(
            parse_gm_command(".god ON"),
            Ok(GmCommand::God(Some(true))),
            "case-insensitive"
        );
    }

    #[test]
    fn parses_xprate_in_range() {
        assert_eq!(parse_gm_command(".xprate 10"), Ok(GmCommand::XpRate(10)));
    }

    #[test]
    fn parses_level_in_range() {
        assert_eq!(parse_gm_command(".level 12"), Ok(GmCommand::Level(12)));
    }

    #[test]
    fn parses_money() {
        assert_eq!(parse_gm_command(".money 500"), Ok(GmCommand::Money(500)));
    }

    #[test]
    fn parses_heal_and_kill_with_no_args() {
        assert_eq!(parse_gm_command(".heal"), Ok(GmCommand::Heal));
        assert_eq!(parse_gm_command(".kill"), Ok(GmCommand::Kill));
    }

    #[test]
    fn parses_tele_names_case_insensitively() {
        assert_eq!(
            parse_gm_command(".tele goldshire"),
            Ok(GmCommand::Tele(TeleSpot::Goldshire))
        );
        assert_eq!(
            parse_gm_command(".tele Northshire"),
            Ok(GmCommand::Tele(TeleSpot::Northshire))
        );
        assert_eq!(
            parse_gm_command(".tele SENTINEL_HILL"),
            Ok(GmCommand::Tele(TeleSpot::SentinelHill))
        );
        assert_eq!(
            parse_gm_command(".tele westfall_coast"),
            Ok(GmCommand::Tele(TeleSpot::WestfallCoast))
        );
    }

    // ---- Clamping ----

    #[test]
    fn speed_clamps_to_0_5_through_10() {
        assert_eq!(parse_gm_command(".speed 0.01"), Ok(GmCommand::Speed(0.5)));
        assert_eq!(parse_gm_command(".speed 999"), Ok(GmCommand::Speed(10.0)));
        assert_eq!(
            parse_gm_command(".speed 0.5"),
            Ok(GmCommand::Speed(0.5)),
            "boundary is legal, not clipped below it"
        );
        assert_eq!(
            parse_gm_command(".speed 10"),
            Ok(GmCommand::Speed(10.0)),
            "boundary is legal, not clipped above it"
        );
    }

    #[test]
    fn xprate_clamps_to_1_through_20() {
        assert_eq!(parse_gm_command(".xprate 0"), Ok(GmCommand::XpRate(1)));
        assert_eq!(parse_gm_command(".xprate 500"), Ok(GmCommand::XpRate(20)));
    }

    #[test]
    fn level_clamps_to_1_through_60() {
        assert_eq!(parse_gm_command(".level 0"), Ok(GmCommand::Level(1)));
        assert_eq!(parse_gm_command(".level 255"), Ok(GmCommand::Level(60)));
    }

    // ---- Bad args ----

    #[test]
    fn bad_or_missing_numeric_args_are_rejected() {
        assert!(parse_gm_command(".speed").is_err(), "missing arg");
        assert!(parse_gm_command(".speed fast").is_err(), "non-numeric");
        assert!(parse_gm_command(".speed nan").is_err(), "non-finite");
        assert!(parse_gm_command(".xprate").is_err());
        assert!(parse_gm_command(".xprate abc").is_err());
        assert!(parse_gm_command(".level").is_err());
        assert!(parse_gm_command(".level abc").is_err());
        assert!(parse_gm_command(".money").is_err());
        assert!(parse_gm_command(".money abc").is_err());
        assert!(
            parse_gm_command(".money -5").is_err(),
            "negative copper is not a valid u32"
        );
    }

    #[test]
    fn god_rejects_a_garbage_arg() {
        assert!(parse_gm_command(".god sideways").is_err());
    }

    #[test]
    fn tele_rejects_an_unknown_name_and_a_missing_arg() {
        assert!(parse_gm_command(".tele narnia").is_err());
        assert!(parse_gm_command(".tele").is_err());
    }

    // ---- Unknown command / malformed input ----

    #[test]
    fn unknown_command_is_rejected() {
        assert!(parse_gm_command(".nuke").is_err());
        assert!(parse_gm_command(".").is_err(), "empty command name");
    }

    #[test]
    fn text_missing_the_leading_dot_is_rejected() {
        assert!(parse_gm_command("speed 3").is_err());
        assert!(parse_gm_command("hello world").is_err());
    }

    // ---- xprate bp math (pure, read side) ----

    #[test]
    fn xprate_default_bp_is_one_x() {
        assert_eq!(XPRATE_DEFAULT_BP, 10_000);
    }

    // ---- Tele coordinates: all four spots are on the same map (same-map move, no SMSG_NEW_WORLD) ----

    #[test]
    fn every_tele_spot_is_on_map_zero() {
        for spot in [
            TeleSpot::Goldshire,
            TeleSpot::Northshire,
            TeleSpot::SentinelHill,
            TeleSpot::WestfallCoast,
        ] {
            let (map_id, ..) = spot.coords();
            assert_eq!(map_id, 0, "{spot:?} must be a same-map move");
        }
    }
}
