# Danger zones

This page is the authority for the traps below and for schema migration rules. Each entry cost
real debugging time at least once. A change that crosses a line in §1 needs a human review before
it ships, whoever wrote it.

To deploy this revision, follow [the native operations guide](native-operations.md). It covers
backup, publication, migration calls and verification for a Realm with one Native Host and one
Shard. An existing multi-Shard Realm needs a reviewed data migration first; see
[Multi-Shard Realms](migrating-to-native-host.md#multi-shard-realms).

## 1. Do-not-touch traps

1. **`spacetime publish -c` is destructive.** It wipes the database and breaks login until
   `claim_operator` runs again. Never run it. Plain `spacetime publish` auto-migrates, or aborts
   without touching the data. `lyracore publish` refuses to forward `-c` or any other flag-shaped
   argument, so deploy through it.
2. **Migrations:**
   - A new column goes at the END of the struct with a `#[default(...)]`. That is an additive
     auto-migrate. A column anywhere else makes the publish abort.
   - SpacetimeDB SQL INSERT needs every column, and it matches values to columns by position, not
     by the names it lists. Durable tests write fixture rows with `Standalone::insert_row`, which
     reads the published columns and their declared defaults, so a new column needs nothing there.
     When an existing importer or native check INSERT can omit the new column, add its default to
     `crates/lyracore-shared/src/import_sql_defaults.txt`. The importer, the native check and the
     `conformance/support` worlds fill it from there.
   - Change native reducer arguments and their Host calls together. The Host and Module also share
     the BSATN layouts of `game_native_cipher` and `game_native_packet`. Update
     `native-host/src/feed.rs` with `module/src/native/mod.rs`, then run the native protocol check.
     Both sides' unit tests pin these layouts, and `game_native_motion_permit`, to the rows in
     `native-host/src/delivery_rows.rs`. Publish a Module with `game_native_motion_permit` before
     you start a Host that subscribes to it; that Host's output subscription fails without it.
   - `./lyracore dev watch` permits only an unchanged Module definition. Its development Host
     compares all reducer arguments and private table layouts before it replaces the Module.
     SpacetimeDB's normal migration check does not compare reducer arguments. Use a planned
     update for schema changes; see [development reload](quickstart.md#development-reload).

   **A defaulted column's default can be a valid value, not a sentinel (#456).** `cell` was
   END-appended to the AOI-scoped tables as `#[default(0i64)]`, and 0 is the packed id of grid cell
   (0, 0). Every existing row claims that cell the moment the publish lands, and AOI looks `cell` up
   by equality. Moving entities fix themselves on their next heartbeat. Static `game_gameobject`
   rows never restamp and stay invisible until backfilled. After a publish of this shape, call
   `debug_backfill_cell_ids` as the Operator before you open the native listeners.
   When you END-append a defaulted column that something indexes or filters on, ask what the
   default means as a value. The migration will apply cleanly and still be wrong.
3. **The 5875 partial-VALUES crash.** Any partial `UNIT_FIELD_*` update must go through the
   `dirty_reset` path so it never carries `OBJECT_FIELD_TYPE`. Sending TYPE again crashes the
   client (null+0x110). Copy `build_health_values` or `build_resistance_values` in
   `crates/lyracore-protocol/src/codec/values.rs`; each has a wire-pinning test.
4. **`SMSG_SET_FACTION_STANDING` sends the reputation index** (`Faction.dbc` `ReputationListID`),
   not the `faction_id`. The id indexes past the client's 64-slot array and crashes the client
   with ERROR #132.
5. **Don't gate features on client-display readback.** The server is authoritative, so verify
   effects server-side. Flag client visuals for a human to look at.
6. **`#[default(...)]` cannot default a `String` column.** The SpacetimeDB macro type-checks every
   default inside a `const` block, and dropping a `String` in const-evaluated Rust is
   `error[E0493]` on stable. This blocks `#[default(String::new())]` and `#[default("")]` alike.
   Numeric and bool defaults are fine. Put new `String` data in a new table instead: it has no
   existing rows, so no column needs a default. `game_creature_waypoint` and `game_creature_spell`
   use the same one-row-plus-child-rows shape. See `NpcTextSlot` in `module/src/gossip.rs`.
7. **Never `.iter()` or `.count()` a spatial table** (`game_world_entity`, `game_corpse`,
   `game_creature_spawn`, `game_gameobject`, `game_dynamic_object`). That includes the usual
   `let entities = ctx.db.game_world_entity();` handle, which is the same whole-world read one line
   later. Use `helpers::entities_near(ctx, map_id, instance_id, x, y, radius)` for a radius search
   and `helpers::in_same_partition(&e, map_id, instance_id)` for a membership test. A whole-table
   scan reads every map and every Instance, so it costs O(world) where the helpers cost
   O(neighbourhood), and it ignores the map and Instance partition that gameplay depends on.
   `partition_discipline_tripwire` in `module/src/tripwires.rs` enforces this with a per-file
   `WHITELIST` that only shrinks; a new scan fails `cargo test -p lyracore-module --lib`. Raise a
   budget only for a realm-wide or partition-managing read, with a one-line reason. A budget
   larger than the real count fails its own test.

### Changing the World Source

A classic-db to vmangos re-import replaces world content and changes spawn guids. Rebuild Package
Spatial Claims against the new Import Revision and recreate Instances. Classic-only quest, item
and credit ids can disappear while Character records still refer to them. Follow
[the source migration procedure](native-operations.md#move-an-existing-realm-from-classic-db-to-vmangos),
including a restored-copy rehearsal and the order of world import before Package apply.

### After every publish

Call `debug_repair_after_publish` as the Operator on the Shard. `init` does not run again on an
auto-migrating publish, so this reducer arms the interval schedules `init` arms
(`arm_interval_schedules` in `module/src/seed.rs`). It deletes and re-creates the instance reaper,
the Gateway lease reaper, the weather schedule, the Meeting Stone reminder and the catch-all creature
tick, which discards a cadence set with `debug_rearm_creature_tick`. It creates the event reaper,
melee, pet care, aura, breath, duel, ground area and motion publish schedules only when missing, and
a Transport crossing for each Transport without one. It also re-freezes Proc profiles, restores
missing Auction expiries, re-tags legacy auction mail as Auction Mail before the Mail Timer pass
(so a legacy "Auction won" is deleted, not returned), arms a Mail Timer for each Mail that has
none, moves each legacy Character GM level onto its Account and re-seeds the curated content and
fixtures `init` seeds. `packages apply` calls it for each Shard it publishes.
A skipped call looks like a mid-session hang, not an error.

Imported content wins over that re-seed. A seed that writes real ids skips once the importer has
stamped its family in `game_import_meta`: `spell` (Weakened Soul, Frost Armor, Chilled, Drain
Soul, Mana Burn, Stealth, Demon Skin, the stacking probe spells, the synthetic Warrior talents),
`items` (Soul Shard), `globals` (the starting spell kits, the Hunter Tame Beast kit row, the
Warrior crit rates) and `dbc` (the Boar creature family, the Riding skill line). Fixtures on
Fixture-Reserved Identifiers and the `SPELL_GROUPS` rules are written on every Shard; the
importer does not import spell groups. `seed_curated_content` in `module/src/seed.rs` holds the list.

The first run on a Shard with Mail from before the Mail Timer applies the vanilla 30-day rule
(3 days with a cash on delivery price) to that Mail. Every Mail already past its life returns to
its sender or is deleted with its copper, at 100 Mail a second. That is a maintainer decision, not
a fault.

### Vendor stock migration

`game_npc_vendor` end-appends `restock_seconds` and `stock_flags` with typed zero defaults.
The private `game_vendor_stock` and `game_vendor_stock_runtime` tables start empty. Publish the
Module before starting the updated Native Host: its first connection announces a process identity
through `native_vendor_stock_runtime`. A new process identity clears stock; reconnects and Module
publication within one Host process retain it. An older Module lacks that reducer and the updated
Host refuses the connection. No native delivery row or existing reducer argument changes. Upgrade both Module and Host for
restart resets; an older Host does not announce a new process.

Reimport the `items` Import Family once, with the updated importer, to gain stock and restock on
an existing Realm. This is one cold import of the source item catalogue, creature loot and vendor
offers; it clears current stock counters. Item templates at or above the Operator Template Range
floor survive, including Operator-created items. Loot and offers that refer to those item entries
or creature entries survive too. Imported rows must not collide with their retained row IDs.
Before its first write, the importer refuses a plan that would leave any owned Item or retained
loot/offer without an item template. A source entry removed from the new catalogue can therefore
block the import; resolve it through a reviewed content migration before retrying. These checks
assume a cold import with no concurrent content edits or Item grants.
Package claims replay only when `--packages` is supplied; replay does not restore Operator edits.
With that flag, the same preflight also refuses removal of an owned or referenced Package Item
template that the enabled Package claims would not restore.
The import does not replace Characters or their owned Items. Supply the same client Data directory
(`--dbc`) required by the items import's Random Property validation; no separate DBC catalogue
reimport is needed for vendor stock.
Without that reimport, the zero restock interval preserves today's purchases: the list shows the
configured maximum and purchases do not consume stock. Seeded and authored offers with a zero
interval keep that behavior. Direct SQL INSERTs must supply both appended columns; shared import
SQL defaults fill older inserts. Rust Packages that construct `NpcVendor` must supply both fields.

Stock belongs to a vendor spawn and item, independently of other spawns with the same entry or
shared offers. Ordinary death, corpse removal and respawn retain stock and elapsed time. Removing
the spawn or its Instance discards stock. These lifetimes follow the in-memory creature object in
vmangos; LyraCore does not implement vmangos grid unloading. Server restart restores full stock.
The counter is lazy: each elapsed interval restores `buy_count` items, capped at the maximum.
A partial refill observed by a list read or buy check resets its timestamp to that request,
discarding the remainder. A refused buy can still observe restock, including this timestamp reset.
It never pays for, stores or consumes the refused purchase. A failure during storage commit rolls
back the payment, stock debit and queued acknowledgement together.
A successful purchase resets the timestamp and selects the next interval. Offer flag 1 selects
80 through 120 percent of the interval; flag 2 scales it above 2500 active, unqueued World Sessions.
Integer truncation occurs after each scaling operation. A computed zero interval is floored to one
second to avoid the Reference Server's division by zero for such source inputs, as agreed by the
maintainer.

These rules come from `Creature::GetVendorItemCurrentCount`, `UpdateVendorItemCurrentCount`,
`VendorItemCount` and the creature destructor at
[vmangos d9173eca](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Creature.cpp).
List zero counts remain visible, as in
[`SendListInventory`](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Handlers/ItemHandler.cpp).

### Buyback migration

`game_buyback_item` and `game_buyback_state` are new private tables. Existing table layouts and the
Native Host delivery layouts do not change. Use the normal planned publish and
`debug_repair_after_publish`; no Shard clear or content reimport is needed for buyback.

The legacy `game_character_buyback` table keeps its schema for additive migration. Its snapshots
cannot restore Item identity and are discarded by the repair reducer or the Character's next login.
The repair reducer ends connected World Sessions that have no `game_buyback_state` row. Their
clients discard legacy buyback links, prices and synthetic objects; the next login starts a fresh
ring and timestamp origin. World Sessions that already have buyback state stay connected. Run the
repair reducer before play resumes after the upgrade, even when the legacy table is empty.
A Handoff or World Port within one World Session preserves buyback, including its login origin
and paused Temporary Enchantments. Logout and fresh login discard it, including login after a
crash or a sessionless Transfer. Carried and banked Items retain their existing rows and GUIDs.

The Character Transfer table list now carries buyback without changing table layouts or the blob
format. The existing compatibility path fills both tables empty for older manifests that omit
them, so sessionless import and fresh login still work. A native World Port requires the original
buyback session row and refuses import atomically if it is absent; an older destination refuses
the new manifest tables. Update both Shards before moving active World Sessions. Retain the source
copy after a refusal and do not attest an import that failed. The older Item row codecs remain
explicit in `module/src/transfer/legacy_item_rows.rs`.

### Corpse AOI migration

The native runtime indexes `game_corpse` by map, Instance and `cell`. The end-appended `cell`
column defaults to zero, which is also a valid cell. After you publish this schema to an existing
Shard, call `debug_backfill_cell_ids` as the Operator before you open the native listeners. It
restamps corpse positions as well as the other spatial tables. New corpses stamp their cell on
insertion; corpse decay and Instance teardown use their time and Instance indexes.

### DynamicObject AOI migration

The native runtime adds a map, Instance and `cell` index to `game_dynamic_object`. Its new `cell`
column defaults to zero, which is a valid cell. After you publish this schema to an existing Shard,
call `debug_backfill_cell_ids` as the Operator before you open the native listeners. It restamps
DynamicObjects and reconciles their AOI Grants. New ground effects stamp their cell on insertion;
expiry removes the DynamicObject and its Grants in the same transaction.

### Combo finisher migration

`game_spell.max_duration_ms` and `game_spell_effect.points_per_combo` default to zero, and older
spell rows mark only Slice and Dice with `CAST_FLAG_COMBO_FINISHER`. Until spells are reimported,
the other finishers skip the combo-point Gate and keep their points, no finisher scales with combo
points, and Slice and Dice lasts its 6-second base. After you publish this schema to an existing
Shard, reimport spells with `--dbc` before play resumes.

### Self-cast refusal migration

Older spell rows lack `CAST_FLAG_REFUSES_SELF_CAST`. Until spells are reimported, a Character can
start a negative spell on itself, such as Fireball with itself selected, where vmangos refuses it with
`BAD_TARGETS`. After you publish this revision to an existing Shard, reimport the `spell` family before
play resumes.

### Chance on Hit migration

`game_item_template.spellppmrate_1` through `spellppmrate_5` default to zero, and older spell rows
lack `CAST_FLAG_WEAPON_PROCS`. Until both are reimported, every Chance on Hit rolls its spell's
proc chance, which is one proc per minute for most weapons, and only white swings roll it. After you
publish this schema to an existing Shard, reimport the `items` and `spell` families before play
resumes.

Enchantment Chance on Hit needs the combat-spell Stat Kind on `game_item_enchantment` rows and the
`game_enchant_proc_rate` table. Crusader, Fiery Weapon and the other combat-spell enchantments
never roll until you reimport the `items` family with `--dbc`. Fiery Weapon, Lifestealing and Icy
Chill roll at the wrong rate until you also reimport the `spellmeta` family.

Extra attacks need effect 19 imported as `E_ADD_EXTRA_ATTACKS`. Older spell rows keep it as
`E_SCRIPTED`, so Hand of Justice, Ironfoe and Sword Specialization grant nothing until you reimport
the `spell` family. The new `game_extra_attack` table starts empty and needs no repair.

### Passive creature combat migration

A plain Module publish adds the private `game_creature_combat_grid` table. It starts empty; the
next melee tick arms a grid check for existing threat with no creature victim. No SQL repair is
needed. Publish through a planned update; development reload requires an unchanged schema.

Passive creatures retain threat and combat without a victim. Their ordinary melee leash does not
expire this threat. The grid check uses the pinned Reference's default geometry, visibility distances
(100 yards outdoors, 170 in dungeons, 533 in battlegrounds) and 30-second activity interval. A grid
with no nearby Character or active creature enters evade and clears that combat. LyraCore keeps the
creature object loaded; this does not implement grid unloading or change vendor stock lifetime.

### Equip spell migration

The seven END-appended source fields on `game_spell` and five category overrides on
`game_item_template` default to zero. Reimport `spell` and `items` before play resumes so equip
effects retain their forms, visibility, duration and category cooldown rules.

Reimport `spell` after the passive visibility update to retain party and pet area-aura origins in
`game_spell.cast_flags`. The translated target alone cannot recover that distinction. After the
import, `debug_repair_after_publish` applies the current visibility rule to stored cast auras;
ordinary passive rows move to hidden slot 255 without losing their effects.

`game_spell_effect` END-appends `equip_supported` with a false default. Until the `spell`
reimport, equipment effects remain inert. That reimport also separates melee and ranged AP,
healing and spell crit, and marks source percentages for conversion to basis points after their
amount is rolled. Healing retains its school mask in `A_MOD_HEALING` for the existing flat
healing calculation. Flat school damage and damage shields no longer import as damage percentages
or absorbs. The complete admitted/inert type list and Item counts are in
[the equip aura support audit](schema.md#equip-aura-support).

The skill update needs a `spell` reimport to translate source auras 30 and 98 to `A_MOD_SKILL`.
After import, publish repair rebuilds stale equipment Auras and repairs existing cast and passive
Auras from their current definitions. It also repairs level-dependent skill caps. No skill table
columns change, and no earned current value is reset. A stored value of 5 cannot distinguish old
initialization from earned progress, so existing Characters keep it. A newly learned skill
starts at 1. Login and repair also add the rows of combat skills an existing Character knows
through a spell but never stored, such as starting Daggers, Bows or Wands, or learns from a
starting spell at login. Those rows start at the level cap, the value a missing row used to give.
Login applies the same rules and reconstructs both client bonus halves.

Before play resumes:

1. Publish the Module through a planned additive update.
2. Reimport `spell` and `items` with the updated importer.
3. Call `debug_repair_after_publish` after the imports.

A repair before reimport is safe; call it again after the imports. Each derived equip aura retains
its source definition locally. Repair keeps unchanged effects and rebuilds stale definitions,
including changed units, visibility and eligibility. It does not restart equip cooldowns or retain
duplicate effects. Changed finite equip effects restart their source duration during this explicit
repair. Unchanged aura identity and deadlines are preserved. Connected Characters and ghosts
receive the rebuilt state.

Repair also compares every ordinary cast Aura's frozen effect kind, parameters, units and cadence
with its imported definition, including offline and permanent rows. It removes missing definitions,
refreshes obsolete metadata, converts percentage units once and clears obsolete absorb capacity.
Aura writes notify connected clients and refresh their sheets. Existing cast magnitudes, stacks,
application times and remaining durations stay frozen, apart from the unit conversion. A legacy
Seal of Righteousness judgement marker restores a missing damage effect at its saved cast level
and deadline. School-modifier group repair keeps the source's stronger elixir or the newest
exclusive spell and removes conflicting saved holders without refreshing their deadlines. Buff and
debuff slots follow the current definition. Missing learned passives are restored without stacking
existing effects. No manual Aura cleanup or recast is required. Repeating repair is idempotent.
Unsupported cast modifiers retain ordinary Aura holders with inert numeric effects; this includes
the source types listed as inert in the equip audit. Flat school damage,
incoming school damage, incoming healing and school crit are active after the spell-power update.
Reimport `spell` to fill the six END-appended source equipment, attribute and damage-class fields
and derive missing coefficients. Equipment class defaults to -1 (unrestricted); the other new
columns default to zero. Reimport `items` to populate the zero-default END column
`game_item_template.damage_school` from `dmg_type1`. The zero-default END column
`game_ranged_impact_schedule.school` freezes the damage school at launch. Run publish repair
after reimport to refresh both cast and equip auras. The zero-default END column
`game_ground_area.effect_id` identifies each new ground effect's incoming coefficient. Older Package Deltas default to inert equip
effects; an author must supply a supported claim.

`game_world_entity` END-appends `sheet_ap_modifiers` and `sheet_ranged_ap_modifiers` with
zero defaults. These derived projections pack the positive signed-16 modifier in the low half
and the negative signed-16 modifier in the high half. Ranged base AP no longer includes these
modifiers. The final publish repair recomputes both projections for connected Characters; world
entry recomputes them for returning Characters. Neither column is Character-owned Transfer state.
Two further zero-default END columns, `sheet_ranged_dmg_min_fraction` and
`sheet_ranged_dmg_max_fraction`, retain fractional ranged damage for the wire. They leave the
existing integer columns intact. Publish repair recomputes them for connected Characters; world
entry recomputes them on login. Both fields are derived and do not cross Character Transfer.
The zero-default END field `sheet_ranged_crit_bp` keeps the ranged weapon's skill contribution
separate from melee crit. Publish repair and world entry refresh it; weapon skill writes refresh
the affected melee or ranged projection. It is also derived and does not cross Character Transfer.
The zero-default END fields `sheet_ap_multiplier` and `sheet_ranged_ap_multiplier` cache
UNIT_FIELD_ATTACK_POWER_MULTIPLIER and its ranged twin, the percent Auras' product minus 1. Publish
repair and world entry refresh them; they are derived and do not cross Character Transfer.
The spell importer now maps auras 166 and 167 (attack power percent, melee and ranged) to
`A_MOD_COMBAT` on the new `COMBAT_ATTACK_POWER_PCT` and `COMBAT_RANGED_ATTACK_POWER_PCT` fields.
Run `spell --dbc` again after publish so imported Spells carry them.
`sheet_ap_base` and `sheet_ranged_ap` keep their `u32` type and hold the bits of the signed base,
so SQL shows a negative base as a large number and the wire reads it as `i32`. Rows saved before
this change hold non-negative bases and need no migration.

`game_item_aura` and its finite-expiry schedule `game_item_aura_expiry` start empty.
`debug_repair_after_publish` reconciles equipment and refreshes the sheet of every online Character
without starting an equip cooldown, including passive effects on ghosts. Fresh login, reconnect,
resurrection and form changes also reconcile equipment. World entry restores passive effects
for a ghost. World exit removes derived equip Auras;
Transfer excludes them and world entry rebuilds them once. Ordinary Auras keep their published
row layout and Transfer codec.

Transfer now carries `game_spell_cd`, including an active equip cooldown. Old blobs that lack
this table import with no spell cooldown rows. An older destination refuses the new manifest.
Update both Shards before an active World Session crosses them. Keep a refused source Transfer
until both ends can import it. The Native Host delivery layouts are unchanged.

The category and category deadline on existing spell cooldown rows default to zero; their spell
deadlines stay unchanged. A pending cast's new category override defaults to `None`, which uses
the spell's category. New Item casts retain their Item category through completion. World entry
sends remaining spell and category cooldowns in `SMSG_INITIAL_SPELLS`.

### Damage shield migration

Reimport `spell`, then call `debug_repair_after_publish` before play resumes. Both cast and equip
Auras use the corrected shield kind. Repair clears obsolete absorb capacity and retains cast
amounts and application times. Repeated repair preserves unchanged equip effects.

### Regeneration migration

Reimport `spell`, then call `debug_repair_after_publish` before play resumes. Both cast and equip
Auras use the corrected MP5 and HP5 kinds. Repair stops obsolete periodic energize and heal ticks,
repairs Evocation's former 15% magnitude to the source 1500% spirit multiplier, and moves Anger
Management to 3/5 of its rage every three seconds. The remaining cast amounts and application times
stay frozen. Repeated repair preserves unchanged equip effects and does not add a second
regeneration contribution.

`game_character_regeneration` is a new empty private table. It needs no backfill. Transfer carries
its fractional health remainder and next tick deadline; older Transfer blobs start with neither.
Existing Characters use the two-second consumer without a recast or manual Aura cleanup. Rage decay
and energy keep their existing consumers.

### Temporary enchantment migration

Poisons, sharpening stones, weapon oils and shaman weapon spells need effect 54 imported as
`E_ENCHANT_ITEM_TEMPORARY`. Older spell rows keep it as `E_SCRIPTED`, so these items and spells
put nothing on the weapon until you reimport the `spell` family. Poisons have no charges until you
also reimport the `spellmeta` family, and a poison proc needs the combat-spell enchantment rows from
an `items` reimport with `--dbc`. The new `game_temporary_enchantment` table starts empty and needs
no repair.

### White melee catalogue migration

`game_crit_per_agility` and `game_creature_defense` start empty after a publish. Reimport the
`globals` and `creatures` families before play resumes; `debug_repair_after_publish` seeds warrior
levels 1 and 60 only on a Shard that never imported `globals`. Other class/level combinations need imported crit rates;
creatures need their no-parry, no-block and no-defense flags. Missing crit rates use the previous
crit calculation in combat and on the sheet, with one warning per Shard. This fallback does not
replace the globals reimport. Existing Character sheet values are refreshed on world entry,
equipment changes and weapon skill changes.

### Starting stack migration

`game_start_item.count` defaults to one. Until you reimport the `globals` family, a new Character
starts with one of each item: one throwing knife, one arrow or bullet, one piece of food. After you
publish this schema to an existing Shard, reimport `globals` before new Characters are created.

### Warrior stance migration

Warrior stances are now Forms: `A_SHAPESHIFT` Auras built from the new public `game_form` and
`game_form_spell` tables. The new public `game_spell_form` table holds the Forms each spell requires
or excludes. All three start empty. No `game_aura` column, Transfer codec or Native Host layout
changes. Stance and passive Auras travel with the other Auras; an older destination Module keeps an
`A_SHAPESHIFT` Aura as an inert Aura.

While `game_form` is empty, warriors behave as before Forms. A warrior enters the world with the
Battle Stance form byte and switches stances through the old `E_SET_STANCE` spell rows. No stance
passive applies, so Defensive Stance has no damage or threat change, and logout returns a warrior
to Battle Stance. Until `debug_repair_after_publish` runs, `game_spell_form` is empty too, so no
spell needs a stance.

Before play resumes:

1. Publish the Module through a planned additive update.
2. Call `debug_repair_after_publish`. It fills `game_spell_form` from each spell's `form_mask` and
   `form_exclude` where the spell has no rows.
3. Reimport `spell` with `--dbc`. It writes Forms 17 to 19 from SpellShapeshiftForm.dbc, the
   spells' Form Sets, the three stances as `A_SHAPESHIFT`, Tactical Mastery as
   `A_FORM_POWER_RETAINED`, `CAST_FLAG_NO_AURA_CANCEL` on spells the client shows no aura icon
   for, and `CAST_FLAG_LOCKS_FORM` on Nefarian's Berserk.
4. Call `debug_repair_after_publish` again. Each online warrior with no stance Aura gets the stance
   its client form byte shows, or Battle Stance, with that stance's passives. A druid form byte is
   left alone.

Offline warriors enter the world in Battle Stance unless they hold a stance Aura.

### Druid form migration

Bear, Dire Bear and Cat Form are now Forms 5, 8 and 1. `game_form` gains four END-appended columns
with defaults: `natural_attack_ms`, `attack_power_from_agility`, `clears_snares` and
`entry_chance_spell`. A Package Delta written before this revision omits them, so its Forms keep the
weapon combat profile, clear no snares and cast nothing on entry. The new Effect Kind
`A_FORM_ENTRY_CHANCE` (`0xD7`) carries Furor. `WorldEntity.stance` stays a column, but nothing reads
it, and the Module ignores the reserved `E_SET_STANCE`.

Until you reimport `spell` with `--dbc`, the druid form spells keep their old `E_SET_STANCE` rows,
so casting them enters nothing: no druid can enter a feral form, and a druid shown in one fights
with its weapon. The cast still spends its mana and starts the global cooldown. Before play resumes:

1. Publish the Module through a planned additive update.
2. Reimport `spell` with `--dbc`. It writes Forms 1, 5 and 8 with their combat profiles, snare
   clearing, Furor spells and passives (Cat 3025, Bear 1178 and 21178, Dire Bear 9635 and 21178),
   the three form spells as `A_SHAPESHIFT`, and Furor as `A_FORM_ENTRY_CHANCE` with the Form Set
   Cat, Bear and Dire Bear. Forms 17 to 19 get the new columns at their defaults.
3. Call `debug_repair_after_publish`. Each online druid whose client form byte shows 1, 5 or 8 and
   who holds no shapeshift Aura gets that Form's Aura and passives. Its mana goes into the Reserve
   Pool and the Form's pool starts empty. A dead or ghost druid gets no Form. The same repair's
   `repair_cast_auras` rewrites every held Furor Aura, online or not, from its old kind to
   `A_FORM_ENTRY_CHANCE`.

Druid talent passives whose Form Set names the feral Forms, such as Sharpened Claws and Feral
Instinct, now come and go with those Forms. Before, they held in every form. Players see the stat
change on the character sheet when they leave a feral Form.

A druid whose Auras change between the reimport and the repair is projected first and drops to
caster form, as does a druid in a form after a crash. Offline druids enter the world in caster form,
as they did before: an `E_SET_STANCE` form never survived logout.

### Remaining form migration

Travel Form, Aquatic Form, Ghost Wolf, Shadowform, Stealth and Moonkin Form are now Forms 3, 4, 16,
28, 30 and 31. `game_form` gains the END-appended column `blocks_mounting` with default false, and
`power_type` 255 (`lyracore_shared::form::OWN_POOL`) shows the unit's own pool. A Package Delta
written before this revision omits the column, so its Forms block mounting and taxis only when they
act as shapeshifted, as before.

From the publish on, breaking stealth ends every Aura of the stealth spell, not only its
`A_STEALTH` Aura. Until you reimport `spell` with `--dbc`, the six form spells keep their old effect
rows: no unit enters these Forms, Travel Form gives no speed, and Ambush, Garrote, Cheap Shot, Sap,
Pick Pocket and Premeditation stay refused, as they have been since Forms 17 to 19, because their
Form Set requires Form 30. Before play resumes:

1. Publish the Module through a planned additive update.
2. Reimport `spell` with `--dbc`. It writes Forms 3, 4, 16, 28, 30 and 31 with their passives
   (Travel 5419, Aquatic 5421, Moonkin 24905), the shapeshift effect of every spell naming those
   Forms as `A_SHAPESHIFT` (every Stealth rank, and creature spells such as Arantir's Deception and
   Spirit Shade Visual), and `blocks_mounting` on Forms 1, 3, 4, 5, 8, 16 and 31.
3. Call `debug_repair_after_publish`. First it ends Stealth remnants, online or offline: the Auras
   of a stealth spell whose unit no longer holds that spell's `A_STEALTH` Aura. An older break, on
   logout, a cast or a swing, shed only the `A_STEALTH` Aura, so such a rogue still carries
   Stealth's slow. Without this step the reimported shapeshift row would put that rogue in Form 30
   without stealth. Then `repair_cast_auras` rewrites the shapeshift row of every held Aura of
   these spells, online or not, to `A_SHAPESHIFT`. Each online Character is then projected: a
   stealthed rogue shows form byte 30 and keeps its energy, and a druid in Travel Form gets 5419. A
   druid who held one of these Auras beside a feral Form keeps the newer one. Creatures are not
   projected: a creature already holding Stealth 1785 (Defias Blackguard, Shadowy Assassin, Shadow
   Panther) keeps form byte 0 until it stealths again or respawns. Only its look is affected.

Offline units holding one of these Auras enter the world in its Form. Players see these changes:
Holy spells are refused in Shadowform and healing spells in Moonkin Form, as their Form Sets say;
Travel, Aquatic and Moonkin Form and Ghost Wolf refuse mounts and taxis; the Stealth openers work.

### Global cooldown migration

`game_spell.gcd_ms` now comes from the source `startRecoveryTime`. An older import set 1500 for
every spell without the passive (0x40) or on-next-swing (0x4) attribute bit. Until you reimport
`spell` with `--dbc`, the old values stay:
- Battle, Defensive and Berserker Stance, and every other spell vmangos gives no global cooldown,
  start one for 1.5 seconds.
- Spells vmangos gives 1 second, such as the rogue Combo Point builders, keep 1.5 seconds.
- The few spells vmangos gives one but the old rule did not, such as Levitate, Water Breathing and
  Detect Traps, start none.

The change is in the importer only, so no Module publish, schema change or repair is needed. Reimport
`spell` from this revision before play resumes.

### Global cooldown category migration

A caster now has one global cooldown per `game_spell.gcd_category`, the source
`startRecoveryCategory`. The column is END-appended with default 0, and the public
`game_global_cooldown` table replaces `game_spell_cooldown`. The old table moves to `retired.rs`;
its rows end within seconds and nothing reads them. The new table starts empty, and no repair is
needed. Publish through a planned update; development reload requires an unchanged schema.

Until you reimport `spell` with `--dbc`, every existing spell is in category 0, so they all share
one global cooldown as before. Stances, Heroic Strike and the other spells without one are still
refused while a global cooldown runs. Reimport `spell` from this revision before play resumes.

A Package Delta written before this revision has no `gcd_category`, so its new spells are in
category 0: their global cooldown neither blocks nor waits for a category 133 spell. Emit the
Package again from a Base Snapshot of this revision, or set `gcd_category`, before you apply it.

### Reserve Pool migration

`game_world_entity` gains three END-appended columns: `reserve_power_type` (default 255),
`reserve_power` and `reserve_max_power` (default 0). The default 255 is the "no Reserve Pool"
value, not a power type, so every existing row holds none, which is true: before this change no
Form showed a pool other than the unit's own. No repair or reimport is needed, and the Transfer
codec and Native Host layouts are unchanged. A Package Form that shows a pool other than a unit's
own, such as rage for a druid, reserves that unit's own pool on its next entry or world entry.
A Package Form whose `power_type` is not a power type (above 4) is now refused at apply; an
already applied one keeps the unit's pool and logs a warning on entry.

### Aura charge migration

Every Aura now freezes its spell's proc profile and `proc_charges`, not only `A_PROC_TRIGGER` and
`A_PROC_DAMAGE` rows, and adds the caster's `SPELLMOD_CHARGES` modifiers. No schema change and no
reimport are needed: the `spell` family already imports `proc_flags`, `proc_chance` and
`proc_charges` for every spell, and `spellmeta` already imports every `spell_proc_event` row; the
Module now gives a first rank's row to its higher ranks.

An Aura applied before this publish carries no profile, so it shows no charge count and spends
none. `debug_repair_after_publish` re-freezes each Aura that has neither proc flags nor charges
with its spell's full charges. Without the repair, such an Aura stays uncharged until it is
reapplied. The repair refills a live mixed holder once: Holy Shield's block row gets 4 charges,
and the holder spends from the larger count of its rows, so a Holy Shield with charges already
spent goes back to full.

### Creature mechanic immunity migration

`game_creature_template` gains the END-appended column `mechanic_immune_mask` with default 0, which
means no template immunity, so every existing template is unchanged until a reimport. Reimport the
`creatures` family to fill it from the World Source; then bosses and elites refuse the mechanics
their vmangos template names. A live creature reads its template's mask on each check and keeps no
copy, so it needs no repair, respawn or `debug_repair_after_publish` step.
Package Deltas may claim the column; one that omits it keeps the default. The Operator Content API
does not carry it, so an Operator update keeps the template's mask.

### Follow-up Cast migration

`game_spell_follow_up` is a new table and starts empty, so no spell makes a Follow-up Cast after the
publish. Reimport the `spell` family to fill it; then Divine Shield, Divine Protection and Blessing
of Protection apply Forbearance and a second bubble on that unit is refused. Forbearance (25771) is
already imported. No existing row changes, so the publish needs no repair, and a unit that is
already shielded gets no Forbearance until its next bubble.

### Follow-up claim migration

No table changes; `game_spell_follow_up` exists since the Follow-up Cast migration. Reimport the
`spell` family straight after the publish. Until then:

- Power Word: Shield applies no Weakened Soul. The Module no longer reads the old `A_ABSORB` `p1`
  link, and the follow-up rows that replace it come with the import. So rank 1, which the link
  covered, can be recast on any unit that has no live Weakened Soul.
- Bandages still apply Recently Bandaged through the old synthetic trigger effect, and Recently
  Bandaged still refuses the next bandage by its immunity. The item check that refused a bandage
  by aura id is gone.

The reimport drops the synthetic bandage trigger and the `p1` link, and writes follow-up rows for
the 19 First Aid spells and all 11 Power Word: Shield ranks; before, only rank 1 linked Weakened
Soul. Live Recently Bandaged and Weakened Soul Auras need no repair: the imported spells were
already aura 77 immunities with bit 0x100. A Shard without a spell import keeps the bandage rows
its `init` seeded, whose Recently Bandaged no longer refuses a bandage; recreate it to get the new
seed. `debug_repair_after_publish` updates the shield fixture and Weakened Soul seeds.

### Cast item migration

`game_pending_cast` gains the END-appended `cast_item_guid` (default 0), and an Item cast now uses
up its Item when the cast goes off, not when it starts. Zero means no cast item. A timed Item cast
in flight at the publish already used up its Item at start, so zero is right for it: it completes
without using up another, and its cooldown category still marks it as an Item cast. No repair or reimport is needed, and the Native Host layouts are unchanged.

### Taxi team mount migration

`game_taxi_node` gains `own_mount_horde` and `own_mount_alliance`, end-appended with default true.
Reimport the `dbc` family straight after the publish. Until then every node passes the node-status
query's team filter for both teams, including a node with no mount at all, which the old resolved
display check skipped; so a nearer mountless node can be reported as not known. The reimport writes
both flags from each team's own mount entry. No repair is needed.

### Taxi network migration

`game_taxi_node` gains `in_taxi_network`, end-appended with default true. Reimport the `dbc` family
straight after the publish. Until then every node counts as in the taxi network, so a flight
master's lookup can land on a node that only spell paths leave. The taxi menu, the learn step and
the node-status query now share one lookup with no 10-yard limit. A Character who opened a menu
before the change may have learned a node that lookup now skips; no repair is needed, and the
node stays known. The reimport writes the flag from `spell_template` effect 123 and `TaxiPath`.

### Quest availability migration

The private `game_quest_availability` and `game_quest_prerequisite` tables are new. Reimport the
`quests` family straight after the publish. Until then no quest has an exclusive group, a maximum
level or a predecessor beyond its `prev_quest_id`, so each of those quests is offered and accepted
as before. The reimport also clears a `prev_quest_id` that names a breadcrumb or a missing quest,
as vmangos does. A Character already on two quests of one exclusive group keeps both; no repair is
needed.

The next-quest-in-chain check needs no reimport: it reads `next_quest_id`, which existing quests
already hold. From the publish on, a Character who has taken or finished a quest's next quest in
its chain can no longer take that quest, and its giver no longer marks it.

### Taxi multi-hop migration

`game_active_taxi_flight` gains `current_leg_index`, end-appended with default 0. The private
`game_taxi_flight_leg` table is new. A flight already in the air has no leg rows, so it is a
one-leg route on leg 0 and flies as before. No repair is needed.

### Native Relay events

Combat, spell cast, spell impact, Speech and emote notifications use SpacetimeDB event tables. Each
adds `_live` to its original table name, such as `game_combat_event_live`. The original
`game_combat_event`, `game_spell_cast_event`, `game_spell_impact_event`, `game_chat_event` and
`game_emote_event` stay ordinary tables with their published schemas, and share each row type with
the event table. The Module writes new notifications only to the event table, and nothing reads
or reaps the original tables.

SpacetimeDB refuses a change to an existing table's event flag. Keep the old declarations and
their indexes for additive schema compatibility. Do not clear a Shard to make this migration apply.

Event tables reach connected subscribers without initial replay, cached rows, update callbacks or
delete callbacks. The Module emits client packets in the transaction that changes gameplay state. A
reconnect restores durable state and receives later notifications. Events emitted during a Transport
Loss are not replayed. Event inserts still enter the commit log, so they do not replace history
retention. See the [event table contract](https://spacetimedb.com/docs/tables/event-tables/).

`game_group_event` and `game_system_message_event` keep their published storage. Group
notifications go directly from the Module; login also reads pending system messages.

Durable tests observe notifications through subscriptions opened before the action. A SQL read
after the action sees an empty event table.

## 2. Tooling gotchas

- **`spacetime sql` (2.5):** no `ORDER BY`, no `IN` or subqueries, no `Timestamp` or `Identity`
  literal. A range filter on a single column can wrongly return 0 rows; use a histogram or
  `COUNT`. To load a `Timestamp` column, go through a reducer that stamps `ctx.timestamp`.
- **`spacetime sql` prints whole floats without a decimal point.** A regex that reads a coordinate
  back needs `-?[0-9]+(\.[0-9]+)?`, or a Character parked at exactly `x=-8890.0` fails every
  "did it arrive" poll.
- **`spacetime call` u64 arguments:** shell-quote the decimal argument to keep it intact. Inside
  structured arguments, keep u64 fields as JSON numbers, for example
  `'{"guid":9007199254740993,"ownership":null}'`. JSON strings in those fields are refused. CLI
  2.7.1 forwards numeric argument text unchanged; avoid float-based intermediates when you build it.
  See the pinned [argument
  assembly](https://github.com/clockworklabs/SpacetimeDB/blob/v2.7.1/crates/cli/src/subcommands/call.rs#L111-L130)
  and [HTTP
  body](https://github.com/clockworklabs/SpacetimeDB/blob/v2.7.1/crates/cli/src/api.rs#L69-L76).
- **A bare `spacetime publish` drops the debug tables.** `lyracore publish` builds with
  `--features=debug_reducers`, passes `--yes`, runs preflight first and refuses `-c`. A bare
  `spacetime publish` omits the feature, so it tries to drop the feature-gated tables and aborts
  with "Removed table: game_debug_readout".
- **`#[default(0)]` on a u64 breaks only at deploy.** It encodes four bytes where eight are
  expected ("data too short for u64: Expected 8, given 4"), and no unit test sees it. Write
  `#[default(0u64)]`. `./lyracore preflight` checks every `#[default(...)]` encoding offline, with
  the pinned SpacetimeDB version gate, schema extraction and RLS-filter validation. Run it before
  every publish.
- **`game_package_import` is private.** Read it with `spacetime sql` only, with an equality filter
  on `family`. `lyracore packages apply` is the supported way to change it; never write it by hand.
  `apply` can publish the Module; see [Applying packages](../packages/README.md#applying-packages)
  for when. Its confirmation covers the Realm changes; `--check` prepares locally without Realm
  writes. An empty Package Delta payload is not "unknown". It means "no Package claims this family"
  and clears the Package spell range.
- **`auto_inc` sequences sit behind explicitly numbered imported rows.** A content import writes
  quest, vendor, trainer and creature-loot rows with explicit ids and does not advance the
  sequence. A later `id: 0` insert from a reducer, or SQL `VALUES (0, …)`, then allocates a
  colliding id. Reducers panic with errno-12 and roll back the whole transaction (a seed that
  "does nothing" is usually this), and SQL inserts fail silently. Fixture and scenario rows use
  fixed reserved ids in the decimal 5,090,000–5,099,999 range (`509_0000`–`509_9999` in Rust)
  with a delete first. `SpellEffect.id` is the same class: it is the deterministic key
  `(spell_id << 2) | index`, so never plain-insert one the importer may also write. Use the
  fixtures' `upsert_effect`.
- **In an imported world, low ids are not yours.** Deleting and reinserting faction-template rows
  14 and 1 breaks the imported Monster and Player templates until the next import. Item entries 50
  and 52 are real imported items. Reputation on a real faction imported without a reputation bar
  does nothing. All fixture data lives at reserved ids (509xxxx items and factions, 50900 quests,
  5090x faction templates), and staging helpers do nothing when the real rows exist. Teardown
  assertions compare against a count taken before the test, never against zero.
- **Probe line of sight before you park a test fixture.** Melee swings gate on `has_los`, which
  uses the exact vmap ray (`vmap::los_ray`, WMO only) when `game_config.vmap_enabled` is on, and
  the rasterized nav obstacle grid when it is off. A spot that looks open can sit inside geometry,
  and then the fight stalls with an armed row, `last_swing_ms=0` and no error. Probe the ray that
  will gate it: `spacetime call lyracore -- debug_vmap_ray <map> <x0> <y0> <z0> <x1> <y1> <z1>` on
  a vmap-enabled map (require `los=None`), or `spacetime call lyracore -- debug_nav_leg <map> <x> <y> <x2> <y2>`
  on a grid-only one (require `has_los=true`). A stationary automated client only
  reaches the 5-yard standstill melee range, so put fight fixtures at 4 yards or less unless the
  client walks in. A level-1 Character in an imported world is a valid aggro target for nearby
  spawns, so disposable test Characters default to level 5.
- **gtker vanilla has known wire gaps:** the update-mask wall, the loot-item codec and the
  faction-standing width. If a packet decodes correctly but the real client misbehaves, suspect a
  gap between gtker and the real client before your own logic.
- **A headless protocol client shares gtker's reader.** It confirms that the server sent the right
  packet and nothing more. It cannot reproduce a client crash or lock-up. That class of bug needs
  a real 5875 client and a person watching it.

## 3. Operating SpacetimeDB

Deployment, backup, restart and verification steps are in
[the native operations guide](native-operations.md). This section covers the node underneath.

### Supervise the node

`deploy/systemd/spacetimedb-standalone.service` is the committed unit for a supervised node. It
waits thirty seconds between restart attempts and allows five starts in five minutes. It gives
every start `524288` file descriptors and appends stderr to
`/var/log/lyracore/spacetimedb-standalone.log`. The unit starts the binary at
`/opt/lyracore/spacetimedb/spacetimedb-standalone` and sets no `LYRACORE_NATIVE_*` variables, so it
opens no WoW listeners. A Native Host service needs the Native Host executable and those
variables; use this unit's restart, descriptor and log settings as its base. After you repair a
start-limit failure, run `systemctl reset-failed` on the unit before you start it again.

The service account `lyracore` owns the data and log directories, so the data survives an
executable upgrade and a crash:

```bash
sudo useradd --system --home-dir /var/lib/lyracore --shell /usr/sbin/nologin lyracore  # once
sudo install -d -o lyracore -g lyracore /opt/lyracore/spacetimedb /var/lib/lyracore/spacetimedb /var/log/lyracore
sudo install -o lyracore -g lyracore -m 0755 /path/to/spacetimedb-standalone /opt/lyracore/spacetimedb/spacetimedb-standalone
sudo install -o root -g root -m 0644 deploy/systemd/spacetimedb-standalone.service /etc/systemd/system/spacetimedb-standalone.service
sudo systemctl daemon-reload
sudo systemctl enable --now spacetimedb-standalone
```

`lyracore-ops service reconcile`, from the `lyracore-cli` repository, runs the last three commands,
restarts the unit and reads back `ActiveState`, `LimitNOFILE` and `StandardError` against the
tracked unit. It never creates the service account, the directories or the binary. Its
[Host operations](https://github.com/LyraCoreProject/lyracore-cli/blob/main/docs/operations.md)
page has the full plan.

Inspect the service and its stderr log with:

```bash
sudo systemctl status spacetimedb-standalone
sudo journalctl -u spacetimedb-standalone --no-pager -n 100
sudo tail -n 100 /var/log/lyracore/spacetimedb-standalone.log
```

After you replace the binary, restart the unit and repeat those checks. Never delete
`/var/lib/lyracore/spacetimedb` during an upgrade: it is the node's database state. The unit binds
to loopback. Do not change its listen address to expose it.

### SpacetimeDB never deletes its own history

SpacetimeDB 2.7.1 keeps every commit-log segment, snapshot and daily module log until someone
deletes them. On a busy Shard this fills the disk: on 2026-09-28 the `lyracore` database held
73 GB of history, and the next deploy had no room to build. The Native Host's own cleanup runs only
during Development reload.

Never delete files under `replicas/` by hand. A restart needs the newest valid snapshot and every
segment after it, and a gap between segments stops the database from starting. Deleting a segment
in the middle, or the newest snapshot, makes the database unrecoverable. Install the pruner, the
disk monitor and the journald cap from the [disk policy](./operations/disk-policy.md) before
another sustained bot run.
