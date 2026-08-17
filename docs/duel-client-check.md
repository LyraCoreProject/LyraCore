# Duel client check

Execution status: outstanding. The automated module and gateway tests verify lifecycle decisions,
typed packets, sparse player fields, and terminal cleanup. They do not verify build-5875 visuals.

Use only an isolated local LyraCore stack. Never point these steps at a production database. Import
the operator-owned 1.12.1 client data so spell 7266 and its duel-flag gameobject template are
available. Start two unmodified 1.12.1 build-5875 clients with living same-faction characters in the
same map and instance. Record the server commit, client build, character names, and test position.

- [ ] From the first character, use the standard Duel action on the second. Confirm one flag appears
      at their midpoint and the challenged character receives the normal request UI.
- [ ] Before acceptance, try a same-faction melee attack and hostile spell. Confirm neither damages
      the other character.
- [ ] Accept on the challenged character. Confirm both clients show the three-second countdown and
      attacks remain blocked until it expires.
- [ ] After the countdown, confirm both characters can damage only each other with melee, ranged,
      direct spell, and periodic damage. Confirm another same-faction character remains protected.
- [ ] Move one participant just beyond 50 yards from the flag. Confirm that client receives the
      out-of-bounds warning once. Return within ten seconds and confirm the warning clears once.
- [ ] Leave the boundary again for ten seconds. Confirm the other participant is announced as the
      winner, both characters leave combat, the flag disappears, and another same-faction attack is
      refused.
- [ ] Start another Duel and use `/forfeit` after the countdown. Confirm it produces the fled winner
      text, clears both clients' Duel state, removes the flag, and ends combat.
- [ ] Start a final Duel and land a would-be lethal hit. Confirm the loser remains at exactly one
      health, never sees Release Spirit, creates no corpse, the winner text appears, the flag and
      Duel fields clear, and both characters leave combat.
- [ ] Repeat once with a logout or local transfer during a pending or active Duel. Confirm the
      remaining client receives one interruption, with no duplicate terminal message or stale flag.

Record any missing UI, duplicate message, disconnect, incorrect health value, or lingering combat
state. Do not mark this check complete from headless results alone.
