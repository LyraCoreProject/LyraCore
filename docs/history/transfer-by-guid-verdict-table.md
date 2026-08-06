# The by-guid verdict table (issue #30)

> **Archived 2026-08-06 (#380).** This was the module doc of `module/src/transfer.rs`, lines 47–144.
> It is the issue-by-issue archaeology behind a rule that is now one paragraph: *a reducer reaching a
> character by guid or by name is fenced through `helpers::character_by_guid` /
> `character_by_name`, unless it holds one of the four verdicts settled below.* Every path named
> here is still enforced — by a named tripwire in `module/src/transfer/tests.rs` and by
> `character_fence_tripwire` in `module/src/tripwires.rs`, whose whitelist carries each exception's
> verdict in one line. What lives here is the REASONING for those verdicts, which is worth keeping
> and was not worth re-reading at the top of the protocol's source file every time.
>
> Read this when you are deciding a NEW by-guid path's verdict, or when you want to know why an
> existing one is not simply "refuse".

Refusal is NOT the right answer everywhere, so the class is settled per path rather than
uniformly. Same-database every one of these is harmless (a write lands on the SAME row the
destination reads); cross-database each unfenced one is a lost write, because the export blob was
serialized at `begin_transfer`.

**REFUSE** — route through `character_by_guid` / `character_by_name`; an in-transit character
reads as absent:

- `auth::delete_character` — the one that DESTROYS a durable copy another shard holds a claim on.
  Keeps its distinguished `CHAR_IN_TRANSIT` error (see the guard there).
- `chat::send_whisper` — reachable because `begin_transfer` deliberately persists with
  `set_offline: false`, so `Character.online` stays true. A visibility leak; refusing lands in
  the gateway's existing `SMSG_CHAT_PLAYER_NOT_FOUND` mapping.
- `gm::set_gm_level` — an operator write to the source copy.
- `debug::debug_spawn_player_entity` — `player_login`'s re-materialisation wearing a harness hat.
- the remaining `debug_reducers` writers: `debug_set_money`, `debug_expire_quest`,
  `debug_grant_reputation`, `debug_grant_default_actions`, `skill::debug_reseed_skills`.
- `instance::reap_instances` — the odd one out in shape, same verdict in substance: it refuses to
  read an instance as EMPTY while a transfer into or out of it is in flight
  (`in_transit_instances`), so it cannot tear down the character's `game_instance_binding`
  manifest rows mid-hop.

Five more, found by the review's independent call-site audit. The first three are by-guid writers
the "the guid came from a live entity, and `begin_transfer` deleted it" argument does NOT cover;
the last two are BACKGROUND tick passes, which that argument cannot reach at all because neither
reads `game_world_entity`:

- `world::debug_delete_character` — `auth::delete_character`'s gate-free harness twin, and
  strictly worse: `cascade_delete_character` runs the `character_owned!` sweep, which includes
  `sweep_delete_game_transfer_out`, so an unfenced call destroys the character AND both escrow
  rows in one transaction. Cross-database the destination's arrival copy is then left with no
  source out-row, `recovery` answers `Hold` forever, and the character is wedged frozen. Keeps a
  raw existence probe alongside the fence so a MISSING character stays the harness's no-op.
- `world::recall_to_home` — the ONE `teleport_player` caller needing no live entity (it reads the
  home coords straight off the durable row), and therefore the only route by which
  `teleport_player`'s unconditional durable-row write reaches an escrowed character: it moves
  `map_id`/`x`/`y`/`z`/`orientation` — five `ExportBlob` fields — plus the `pending_instance_id`
  that `in_transit_instances` reads. Reachable by guid through `debug::debug_use_hearthstone`;
  fenced in `recall_to_home` itself so a future by-guid caller inherits the gate.
- `debug::debug_set_level` — drives `stats::set_character_level`, which writes `Character.level`
  (an `ExportBlob` field) and `Character.xp` on the durable row and needs no live entity to do it.
  The `debug_set_money` sibling; fenced at the reducer, not in `stats`, because `gm_command`'s
  caller is already entity-resolved and the core also serves guids with no character row.
- `rest::rested_accrue_pass` — the 30s live-rested accrual. `begin_transfer` persists with
  `set_offline: false`, and it is `persist_entity`'s `set_offline` branch that would have called
  `materialize_on_logout` to stop the rest clock — so a character escrowed while standing in an
  inn keeps `resting == true` with a running `rested_since_micros` and this pass rewrites
  `rested_xp` on the frozen row every 30s, for as long as the escrow is held.
- `quest::quest_timer_pass` — the 0.5s timed-quest expiry, which writes `game_character_quest`, a
  MANIFEST table. The same table and the same reasoning that fenced its harness twin
  `debug::debug_expire_quest`, which this pass reached straight past.

Both tick passes carry their own `is_in_transit` gate rather than routing through
`character_by_guid` (each already holds the row, or its owned row, from a scan), and in both the
refusal is a DEFERRAL in substance — which is why REFUSE is honest here rather than
value-dropping. The rest pass leaves `rested_since_micros` running, so the first pass after the
transfer settles banks the entire span in one step; the quest pass leaves `deadline_micros` set,
so the first pass after it settles fails the quest exactly as it would have.

**DEFER into the blob as a post-begin delta** — `loot::credit_purse`. A party member collects
their share of someone else's kill, so refusing would silently DROP a third party's copper; that
third party is not transferring and could not know why they were shorted.
`defer_money_delta` folds it into the escrowed blob so the value travels with the character.

**REGENERATE at the destination** — `auth::establish_session`, deliberately UNFENCED. It rewrites
`Character.owner_identity`, which is per-CONNECTION derived state, not durable character data:
`establish_session` is its only writer and rebinds it from the live connection at every logon,
which is also why `player_login` restamps the owned rows from `ctx.sender()`. Cross-database the
player arrives through the DESTINATION's gateway, which binds it correctly with no help from the
blob. Carrying it would ship a source-gateway identity that is meaningless on arrival and
overwritten immediately — a field always wrong on arrival is worse than no field, because the
next reader cannot tell it is vestigial. Pinned by
`owner_identity_is_regenerated_at_the_destination_never_carried`.

**NOT A TRANSFER CONCERN — settled by issue #22 (group slice)** — `group::group_accept` /
`group_uninvite` / `group_leave`. These were the taxonomy's last three OPEN entries, deliberately
left undecided pending the realm-core boundary. That boundary now exists, and it dissolves the
question rather than answering it: party membership is authoritative on REALM-CORE, so a third
party's accept/kick/leave for an in-transit character is a write to the DIRECTORY database, which
is not the database the escrow froze and not a database this transfer touches at all. There is no
source-copy write left to lose, so there is nothing here to fence.

What replaced the fence question is a REPLICATION one, and it is answered in the other direction:
`game_group_member` moved onto [`NOT_TRANSPORTED`] (it must not ride the blob — the blob's
snapshot would race the authority), and the gateway re-pushes realm-core's roster onto the
destination at world entry via `group::sync_group_mirror`. A character kicked mid-loading-screen
therefore arrives ungrouped, which is what actually happened, instead of arriving with the
membership it had when it stepped into the portal.

The audited exception list is now EMPTY: every by-guid path in the tree is either fenced, listed
above with its verdict, or — for these three — no longer a world shard's state.

Enforcement: each fenced call site is pinned by a source-scan tripwire in
`module/src/transfer/tests.rs` (deleting a fence turns a NAMED test red — a pure model has no
reducers in it and sees none of them), and `lib.rs`'s `character_fence_tripwire` is the ratchet that
stops a NEW unfenced by-guid path from being added.
