# Sheath stock-client check

Execution status: outstanding. This is the unresolved visual leg of #101 (Sheathed weapons/shields
stow in the wrong position). #101's headless legs are green: the raw cmangos sheath byte reaches the
wire unremapped for both a hip weapon and a back-mounted shield. What #101 could not settle is
whether an unmodified stock 1.12 client renders that byte correctly, or whether a stock client's
persistent `ItemCache.wdb` is showing a stale answer from before the fix. This document is the
three-condition A/B that settles it. Only a human operator with a stock 1.12 client and Benilla can
run it.

## 1. Fixture items and expected wire values

| Entry | Name | `item_template.sheath` | Family | Stowed | Drawn (melee) |
|---:|---|---:|---|---|---|
| 25 | Worn Shortsword | 3 | one-hand weapon | left hip | in hand |
| 1166 | Dented Buckler | 4 | shield | back | off-hand |

These are the raw cmangos dump values (`item_template` column 113), not a LyraCore remap:
`importer/src/main.rs:374` reads the column verbatim, `module/src/items/tables.rs:93-100` stores it
unchanged, and `gateway/src/codec/item.rs:217` sends it unchanged as
`SMSG_ITEM_QUERY_SINGLE_RESPONSE.sheathe_type`.

**Fixture caveat.** The dev fixture's hand-authored entry 25 (`module/src/seed.rs:339-364`) never sets
`sheath`, so it defaults to 0 (`module/src/seed/fixtures.rs:500-533`) until `./lyracore import` runs.
Import wipes and rebuilds `game_item_template` entirely
(`DELETE FROM game_item_template WHERE entry > 0`, `importer/src/main.rs:4177`) from the real dump, which
is where entry 25 picks up `sheath=3` and where entry 1166 first exists at all — it is not seeded.
**Run this check on a shard that has completed `./lyracore import`, not the bare `dev up` fixture.**

Keep two concepts separate, per the issue's implementation decision:

- **Item-template sheath** (`item_template.sheath`, wire field `sheathe_type`) — where an item stows
  when the unit's weapons are stowed. Set once per item, queried once per client session (subject to
  `ItemCache.wdb`).
- **`UNIT_FIELD_BYTES_2` byte 0** (the sheath *state*, `gateway/src/codec/entity.rs:63`,
  `gateway/src/codec/values.rs:209-217`) — whether the unit itself is stowed (0), melee-drawn (1), or
  ranged-drawn (2). Written by `CMSG_SETSHEATHED`
  (`gateway/src/world/handlers/combat.rs:50-57` → `module/src/gw.rs:346-352` →
  `module/src/world.rs:1759-1777`) and relayed live so nearby observers see the draw/stow happen.

A wrong hip/back placement while drawn/stowed toggles correctly is an item-template problem. A
character that never appears to draw or stow at all is a `UNIT_FIELD_BYTES_2` problem — #101 already
fixed and tested that path (`gateway/src/codec/tests.rs:1831-1867`,
`gateway/src/stdb/subscriptions.rs::sheath_change_relays_and_a_no_op_stays_silent`).

## 2. Three-condition A/B procedure

Conditions, in order:

1. **Benilla** — comparison oracle. Benilla keeps received item templates in a session-local in-memory
   map, so it always reflects the current server answer.
2. **Stock 1.12 (build 5875), existing cache** — whatever `ItemCache.wdb` the client already holds.
3. **Stock 1.12, `ItemCache.wdb` removed** — delete only that one file, under the client's
   `WDB/<locale>/` directory (e.g. `WDB/enUS/ItemCache.wdb`). Leave every other file alone: other WDB
   caches (`CreatureCache.wdb`, `NameCache.wdb`, ...), the whole `WTF/` tree (accounts, macros,
   keybinds), and `Cache/` are untouched. Reconnect after deleting it.

Same character and equipment in all three conditions. Per condition:

1. Equip Worn Shortsword (25) and Dented Buckler (1166).
2. Stow (`Z`). **Record:** where the sword renders, where the shield renders.
3. Draw with a melee attack. **Record:** both items in hand.
4. Stow again (`Z`). **Record:** both items back at their stowed position, matching step 2.

Record the client build shown on the login screen and the server commit alongside every row.

## 3. Confirming a clean client re-queries

The gateway does not log individual `CMSG_ITEM_QUERY_SINGLE` requests or the fields inside a
response — only the outgoing opcode name, at debug level:
`log::debug!("OUT {m}")` in `gateway/src/world/mod.rs:852` prints `OUT
SMSG_ITEM_QUERY_SINGLE_RESPONSE` every time the gateway answers a query, with no entry id or byte
value attached (`ServerOpcodeMessage`'s `Display` impl is the variant name only).

Run the gateway with `RUST_LOG=info,gateway::world=debug` (the invocation `docs/danger-zones.md`
already uses for a targeted session; expect it to be noisy over a whole realm, fine for one
character) and tail its log across condition 3's reconnect. An `OUT
SMSG_ITEM_QUERY_SINGLE_RESPONSE` line appearing right after login is the client asking again — cache
was cleared successfully and the server got a fresh chance to answer. No such line means the client
is still serving both items from a source that didn't need to ask, and the placement result for
condition 3 does not distinguish "server said the right thing" from "client never asked."

The response byte itself does not need re-proving here: Part 1 of this check already pins it as
`sheathe_type=3` / `sheathe_type=4` for entries 25 / 1166 at the codec level
(`gateway/src/codec/tests.rs:1428-1463`, `:1531-1556`), and #101 wire-decoded the same bytes off a live
local gateway. The log's only job in this check is confirming the client *asked*, not decoding what
it was told.

## 4. Results

| Client build | Cache | Stowed sword | Stowed shield | Drawn sword | Drawn shield | Verdict |
|---|---|---|---|---|---|---|
| Benilla | n/a | | | | | |
| stock 1.12 | warm | | | | | |
| stock 1.12 | clean | | | | | |

## 5. Decision

- **Clean cache fixes it** (condition 3 matches Benilla, condition 2 does not): the bug was stale
  `ItemCache.wdb`, not a server defect. The follow-up is a documented cache-invalidation step for the
  LyraCore dev-client workflow — the smallest addition that clears only `ItemCache.wdb`, living in
  `docs/development-cli.md` (the CLI's command/safety contract) or `docs/quickstart.md` (the walkthrough),
  whichever already covers reconnecting a test client. Do not touch the rest of `WDB/` or `WTF/`.
- **Clean cache does not fix it** (condition 3 still wrong): the data pipe is proven byte-correct
  (Part 1), so the remaining gap is the mapping from the wire byte to the client's internal stow-posture
  table. Capture condition 3's actual item-query and update-object traffic and compare it field-for-field
  against a known-compatible vanilla server (cmangos or VMaNGOS) before changing any codec value. Do not
  remap `sheathe_type` from this check alone — the issue is explicit that known-good imported values stay
  untouched without that comparison.

Prior art: #101 (server-side sheath-state relay, proven byte-correct item-query response; left the
live client eyeball open, which this document closes).
