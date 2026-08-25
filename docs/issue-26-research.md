# Issue #26 research: world clock and per-zone weather

Sources: [issue #26](https://github.com/LyraCoreProject/LyraCore/issues/26), the repository source,
the [gtker 1.12 packet grammar](https://raw.githubusercontent.com/gtker/wow_messages/main/wow_message_parser/wowm/world/world/smsg_weather.wowm),
the [clock grammar](https://raw.githubusercontent.com/gtker/wow_messages/main/wow_message_parser/wowm/world/login_logout/smsg_login_settimespeed.wowm),
and the primary [CMaNGOS Weather.cpp](https://raw.githubusercontent.com/cmangos/mangos-classic/master/src/game/Weather/Weather.cpp)
and [Weather.h](https://raw.githubusercontent.com/cmangos/mangos-classic/master/src/game/Weather/Weather.h).

## Issue and current code

Issue #26 has no comments or linked branches as of 2026-08-19. It asks for:

- Real server time in `SMSG_LOGIN_SETTIMESPEED`, with the vanilla client free-running at 1/60.
- Per-zone weather state, using CMaNGOS `game_weather` seasonal weights, a slow Module
  scheduler, and `SMSG_WEATHER` on change, login, and zone entry.
- Hand-seeded Elwynn/Westfall data until the SQL importer handles `game_weather`.
- A live-client acceptance check: an Elwynn debug rain reaches Elwynn but a Westfall client stays dry.

The Gateway currently emits a hard-coded clock at `gateway/src/codec/entity.rs:644-650` and has no
weather packet, weather table, weather reducer, weather schedule, or weather relay. The Module owns
durable state; the existing relay convention is a public event row with auto-incremented `id` and
`created_at: Timestamp`, consumed by the Gateway coordinator and reaped by
`game_event_reaper_schedule` every second (`module/src/gc.rs`, `module/src/seed.rs`,
`gateway/src/stdb/world_view.rs`).

The repository already imports `game_area` from `AreaTable.dbc`; `terrain::zone_id_at` maps a
terrain area to its enclosing zone with one parent hop (`module/src/config.rs`,
`module/src/terrain.rs:121-139`). Existing fixtures use zone 12 for Elwynn and zone 40 for
Westfall. The live `game_world_entity` has no zone column. `game_character.zone_id` can lag during
ordinary movement because movement updates the live entity each heartbeat but re-derive the durable
zone only on persistence/teleport paths. A zone-enter weather transition must therefore define its
zone source; using only the persisted Character row is not sufficient.

## Clock contract and a current bug

The gtker grammar defines `SMSG_LOGIN_SETTIMESPEED = 0x0042` as packed `DateTime` plus `f32
timescale`. Its documentation says this packet changes the client clock; `SMSG_QUERY_TIME_RESPONSE`
does not. `0.01666667` means one in-game minute per 60 real seconds.

The [DateTime implementation](https://raw.githubusercontent.com/gtker/wow_messages/main/wow_world_base/src/manual/shared/datetime_vanilla_tbc_wrath.rs)
packs:

```
year-after-2000 << 24 | month0 << 20 | day0 << 14 | weekday << 11 | hour << 6 | minute
```

The constructor takes zero-based month and day; display adds one to the stored day. The chrono
conversion uses `month0()` and `day0()`. Current LyraCore code uses
`DateTime::new(26, Month::June, 15, Weekday::Tuesday, ...)` while claiming “2026-06-15 -> Tuesday”.
That value encodes June 16, 2026, Tuesday. June 15, 2026 is Monday. A real implementation needs a
timezone-aware date/time source and must define “server time” as host local time, configured realm
timezone, or UTC. The vanilla API is a realm/game clock, not a client-local clock.

## Weather wire contract

The 1.12 grammar defines:

```
SMSG_WEATHER = 0x02F4 {
    u32 weather_type; // FINE=0, RAIN=1, SNOW=2, STORM=3
    f32 grade;        // intensity, conventionally 0..1
    u32 sound_id;
    u8  change;       // SMOOTH=0, INSTANT=1
}
```

The packet contains no zone id. The server must select the audience; a per-zone state row and
zone-gated relay are required to prevent Elwynn weather reaching Westfall.

## CMaNGOS behaviour

CMaNGOS's `WeatherSystem` lazily creates one weather object per active zone, stores current type,
grade and timer in that object, and removes it when no players remain. Its timer interval is the
configured `CONFIG_UINT32_INTERVAL_CHANGEWEATHER` (logged in minutes). When it expires, it rolls
only once and broadcasts only if type or grade changed. A zone without chance data stays Fine/0.

The roll semantics are:

- 30% no change.
- Other rolls improve/clear, worsen, or make a radical change based on current type and grade.
- When selecting new weather, cumulative seasonal rain/snow/storm weights are sampled; the remainder
  is Fine. Four seasons are selected from `localtime` day-of-year.
- Grade is normalized to 0..1. CMaNGOS classifies <0.40 as light, <0.70 as medium, and >=0.70 as
  heavy. Fine is grade 0.

`LoadWeatherZoneChances` selects `zone` plus three chance columns for each of spring, summer,
fall, and winter from `game_weather`. Individual values above 100 are replaced with 25 and logged;
the three weights are not normalized to total 100. This supports a direct hand-seeded table shape:
`zone`, then 12 seasonal rain/snow/storm percentages.

The packet body is 13 bytes: CMaNGOS writes weather type, grade, sound id, and `change=0` (smooth)
both to one player and to all players in a zone. Sound ids are rain 8533/8534/8535, snow
8536/8537/8538, storm 8556/8557/8558, and Fine 0. A debug force should state whether it uses smooth
or instant change.

## Decisions the spec must make explicit

1. Keep clock and weather separate: clock is a login-time computation; weather is Module-owned
   durable world state.
2. Define weather row fields at least as `zone_id`, `weather_type`, `grade`, and update time,
   plus a clear no-data default (CMaNGOS Fine/0).
3. Define first-observer/login and zone-entry delivery. Existing subscription initial applies do not
   reliably fire per-row callbacks, so a newly subscribed client needs an explicit current-state
   recovery path.
4. Define how ordinary movement resolves zone transitions. The existing highest seam is Module
   movement/teleport, using `terrain::zone_id_at`, with one transition only when the enclosing zone
   changes.
5. Define a slow scheduler cadence, deterministic debug force semantics, and per-zone audience
   assertions. The acceptance test should assert both weather payload and absence of the packet for a
   Westfall viewer.

## Verification seams

- Pure clock conversion and round-trip tests, including zero-based month/day, weekday, leap day, and
  timezone/date-boundary cases.
- Pure weather tests for cumulative weights, Fine remainder, intensity bands, sound selection,
  no-change roll, and forced state.
- Module tests for per-zone persistence, scheduler updates, event GC, and one zone-enter transition
  per actual enclosing-zone change.
- Gateway/headless-client wire test for the clock and zone-gated `SMSG_WEATHER`; repeat the issue's
  live-client test because rendering day/night and rain is client behaviour.

