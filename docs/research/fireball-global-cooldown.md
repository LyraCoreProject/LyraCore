# Fireball global cooldown timing

The reference supports starting Fireball's global cooldown when the cast starts.
Launching the projectile must not start another global cooldown. A cast longer
than its global cooldown can therefore be followed by another cast at launch.
This follows from the separate start and completion paths in
[CMaNGOS `Spell.cpp`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/Spell.cpp#L3066-L3084).

## Reference behavior

All links pin CMaNGOS Classic commit `8ec338a1704e7dcb1c0213eb7ed58f9231ade40f`.
This is primary evidence of that implementation. It does not prove Blizzard's
original 1.12 internals.

- [`Spell::Prepare`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/Spell.cpp#L3066-L3084)
  calls `AddGCD` before `SendSpellStart`, for ordinary casts with or without a cast
  time.
- [`WorldObject::AddGCD` and `HasGCD`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Object.cpp#L2599-L2617)
  store the global cooldown category's expiry as the current time plus
  `StartRecoveryTime`. The Gate lasts until that expiry.
- [`Spell::cast`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/Spell.cpp#L3271-L3282)
  calls `SendSpellCooldown` immediately before the launch message, `SendSpellGo`.
  [`SendSpellCooldown`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/Spell.cpp#L3510-L3517)
  calls `AddCooldown`, not `AddGCD`.
- [`Player::AddCooldown`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Player.cpp#L20196-L20201)
  reads `RecoveryTime` and `CategoryRecoveryTime`. Those fields describe the
  spell's own cooldown and shared spell category cooldown. They are separate
  from the global cooldown fields.

## Timing to preserve

Given a 1.5-second global cooldown, a 3-second cast starts the global cooldown at
0 seconds, outlasts it at 1.5 seconds, and launches at 3 seconds. A 1-second cast
launches with 0.5 seconds of the original global cooldown left. These examples
follow the expiry calculation in
[`WorldObject::AddGCD`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Object.cpp#L2599-L2617).
Instant casts also start the global cooldown through `Prepare` before executing.

The implementation should preserve the global cooldown recorded at cast start
and start any spell cooldown at successful completion. Moving every cooldown to
cast start would change spells with their own cooldowns.

CMaNGOS also applies global cooldown modifiers and compensates for latency in
[`Player::AddGCD`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Player.cpp#L20149-L20182).
Those adjustments do not change which cast phase starts the global cooldown.

## Cancellation and interruption

Canceling a timed cast before launch clears its remaining global cooldown in
this reference. [`Spell::cancel`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/Spell.cpp#L3114-L3126)
calls `ResetGCD` in the created, targeting and casting states.
[`ResetGCD`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Object.cpp#L2800-L2811)
erases that global cooldown category. The traveling and channeling cases do not
call it.

An ordinary client cancel follows
[`HandleCancelCastOpcode`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/SpellHandler.cpp#L394-L409),
[`InterruptNonMeleeSpells`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Unit.cpp#L4384-L4396),
and [`InterruptSpell`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Unit.cpp#L4302-L4328)
to `cancel`.

Damage pushback keeps the original global cooldown. The
[`damage path`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Unit.cpp#L1270-L1287)
chooses interruption for spells whose damage flags require cancellation,
otherwise it calls
[`Delayed`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/Spell.cpp#L6803-L6840).
That method changes the cast timer without changing the global cooldown.

A successful
[`interrupt effect`](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/SpellEffects.cpp#L3588-L3608)
also reaches `cancel`, but first applies a school lockout through `LockOutSpells`.
Clearing the global cooldown therefore does not remove that school lockout.

## LyraCore diagnosis

The local regression used Fireball with a 2-second cast, a 1.5-second global
cooldown and no separate spell cooldown. Before the fix, the stored global
cooldown expired exactly 1.5 seconds after the completion event. The cast resolver
started it at completion.

The fix starts the global cooldown when a timed cast is accepted. Completion
rechecks the other Gates and preserves that deadline. Cancellation before launch
releases the global cooldown. The durable tests use an isolated local instance:

```sh
cargo test -p lyracore-module --test cast_global_cooldown -- --ignored
```
