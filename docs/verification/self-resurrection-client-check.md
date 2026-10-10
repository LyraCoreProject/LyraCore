# Self-resurrection client check

Execution status: outstanding. This checklist needs a human with an unmodified 1.12.1 build-5875
client. The automated tests prove the Self-Resurrection Option at the reducer seam, the
`PLAYER_SELF_RES_SPELL` bytes, and the `CMSG_SELF_RES` dispatch. They cannot prove what the client
draws or when it sends the opcode. The field index and the 30 minute duration come from emulator
source and the ClassicDB dump, not from a client.

Use only an isolated non-production LyraCore stack. Never point these steps at a production database,
and never use `spacetime publish -c`. Point the client's realmlist at the isolated stack first. Import
the operator-owned 1.12.1 client data with spells and items, so the five Soulstone Resurrection
ranks, their self-resurrect spells and the Soulstone item templates exist. Run the Gateway at debug
log level.

Prepare one Warlock of level 18 or higher, and one Priest who knows Resurrection for the ally step.

Data, before any client:

- [ ] On the isolated stack, read `game_spell` and `game_spell_effect` for spells 20707 and 3026 with
      `spacetime sql`. For 20707 confirm `duration_ms = 1800000`, `cast_time_ms = 3000`, and one
      effect with kind `0xB5` (`A_SELF_RESURRECT`), `p0 = 3026`, `p0_kind = 15`. For 3026 confirm
      one effect with kind `0x26` (`E_SELF_RESURRECT`), `base_points = -400`, `p0 = 700`. Record any
      difference.

Soulstone:

- [ ] Grant the Warlock a Minor Soulstone (`debug_grant_item <guid> 5232 1`) and use it on itself.
      Record whether the 3 second cast bar shows, and whether the buff shows a 30 minute timer.
- [ ] Kill the Warlock (`debug_set_health <guid> 0`). Confirm the death dialog shows a second button
      beside Release Spirit. Record its exact text.
- [ ] Click it. Confirm the death screen closes, health shows 400, mana shows 700 (or the Warlock's
      max mana if lower), no Resurrection Sickness, and no corpse. Record the numbers.
- [ ] Record whether the client plays any cast animation or sound on use. The server sends no cast
      packet for it.
- [ ] Repeat, but click Release Spirit first. Record whether the ghost sees any self-resurrection
      prompt or button. The server keeps the option for the ghost; if the client offers a way to use
      it, confirm it works.
- [ ] Repeat, but die inside the Deadmines and release into Westfall. Record whether the option is
      still offered. The server does not carry it across a Shard hop.
- [ ] Die with a Soulstone, let the Priest resurrect the Warlock, and accept. Then die without a
      Soulstone. Confirm the dialog shows only Release Spirit.
- [ ] Die with a Soulstone, log out on the death screen, and log back in. Record the state.
- [ ] Use a Soulstone, then move during the 3 second cast. Record whether the item is lost. Every
      timed consumable is consumed when its cast starts.

Record the server commit, the client build on the login screen, Character names and levels, each
dialog text, each number read from the client, and any disconnect or error dialog.
