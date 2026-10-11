# Hearthstone spell identity and cast completion

Research for [issue 651](https://github.com/LyraCoreProject/LyraCore/issues/651),
against LyraCore commit `0d084310db5dc9d37a60247b5e669ddaf0b71bb8`.

The item selects the wrong spell. The cast lifecycle already supports the required
delay and cancellation. Fix the seeded item and spell together, preserve the
imported vanilla spell identity, and map the vanilla home-bind teleport effect.

## Evidence

- The seeded item 6948 selects spell 50119. Its seeded spell, Call Stone, has one
  `E_RECALL_HOME` effect and inherits a zero cast time from `base_spell`.
  [Item](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/module/src/seed.rs#L496-L514),
  [spell](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/module/src/seed.rs#L1207-L1219),
  [default](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/module/src/seed/fixtures.rs#L449-L459).
- The item importer overrides Hearthstone to 50119. Removing that entry preserves
  the source item's `spellid_1` and `spelltrigger_1`.
  [Override and source-field handling](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/importer/src/main.rs#L3354-L3370),
  [source fields](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/importer/src/main.rs#L3460-L3470).
- The Gateway sends the template spell ID in the item query. Its codec documents
  that the client renders the use text through its own `Spell.dbc`. Sending 8690
  therefore addresses the reported missing use text, but the visible tooltip
  still requires a build 5875 client check.
  [Item query codec](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/gateway/src/codec/item.rs#L213-L230).
- ClassicDB lists spell 8690 as Hearthstone, with a ten-second cast, raw effect 5
  Teleport Units and a second raw effect 77 Script Effect. Item 6948 has the
  return-home use text. These pages corroborate the issue's expected behavior;
  they are reference data, not Blizzard's specification.
  [Spell](https://classicdb.ch/?spell=8690), [item](https://classicdb.ch/?item=6948).
- The primary CMaNGOS code assigns raw implicit target 9 to
  `TARGET_LOCATION_CASTER_HOME_BIND`. It resolves that destination through the
  Character's home-bind coordinates and map. Raw effect 5 then applies the
  resolved destination, including a different map.
  [Target table](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/SpellTargets.cpp#L42),
  [destination](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/Spell.cpp#L1840-L1848),
  [effect](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/SpellEffects.cpp#L1683-L1731).
- LyraCore's raw effect mapping leaves effect 5 as `E_SCRIPTED`, a no-op.
  Its spell header importer already reads cast time from `SpellCastTimes.dbc`.
  Preserve that imported time and classify only effect 5 with a home-bind
  destination as `E_RECALL_HOME`, with `T_SELF` routing. Mapping every teleport
  to home would change other teleport spells. Mapping both Hearthstone effects
  to recall would execute the teleport twice.
  [Raw mapping](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/importer/src/spell.rs#L440-L466),
  [cast time](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/importer/src/spell.rs#L1205-L1212),
  [target routing](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/importer/src/spell.rs#L1696-L1721).

## Existing lifecycle

`apply_item_use` keeps items whose spell has `E_RECALL_HOME`, then calls
`begin_cast`. A timed spell schedules a `PendingCast` and emits a start event with
the cast duration. `fire_pending_cast` resolves its effects at completion.
`interrupt_cast` deletes the scheduled row, so cancellation prevents recall.
Movement also calls `break_channel`, which cancels the pending cast. No separate
Hearthstone scheduler is needed.
[Item use](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/module/src/items/ops.rs#L523-L540),
[cast admission](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/module/src/spell/cast/resolve.rs#L1371-L1546),
[completion](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/module/src/spell/scheduler.rs#L19-L57),
[cancellation](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/module/src/spell/control.rs#L493-L550),
[movement](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/module/src/world.rs#L1589-L1595).

`E_RECALL_HOME` uses the durable Character's bound home and instance 0, through
the shared teleport path. Preserve this destination behavior.
[Recall effect](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/module/src/spell/cast/targeting.rs#L1267-L1273),
[recall destination](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/module/src/world.rs#L916-L944).

## Checks for the fix

1. Import actual build 5875 data. Item 6948 must retain on-use spell 8690. Its
   spell header must have `cast_time_ms = 10000`, with exactly one
   `E_RECALL_HOME` effect targeting self. An unrelated raw teleport must remain
   unchanged.
2. Decode the item-query response and check the first spell slot is 8690 with
   on-use trigger 0.
3. Use a Hearthstone through the durable item-use path. Check start duration
   10000, unchanged location before completion, bound home at completion,
   instance 0, and the same item instance and count throughout.
4. Cancel a pending Hearthstone cast and wait past its former deadline. Check
   that no completion or teleport occurs and the item remains. Check movement
   cancellation through the movement path too.
5. Check the tooltip, cast bar and cancellation in a build 5875 client. A packet
   test cannot prove the client renders its local spell description.

The issue covers spell identity, cast timing and cancellation. Hearthstone's
item cooldown needs separate item-cooldown handling if it is included in scope.
The item query currently defaults its cooldown fields, and the spell importer
only derives cooldowns from spell DBC fields. Do not claim the item cooldown is
fixed by changing the cast time.
[Item query spell fields](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/gateway/src/codec/item.rs#L218-L222),
[spell cooldown import](https://github.com/LyraCoreProject/LyraCore/blob/0d084310db5dc9d37a60247b5e669ddaf0b71bb8/importer/src/spell.rs#L1213-L1217).
