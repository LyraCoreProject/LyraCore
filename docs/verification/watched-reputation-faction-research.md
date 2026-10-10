# Watched reputation faction research

Research status: complete. Client Verification status: outstanding. This note checks source at
LyraCore commit `46bf948b27b6d21555832b7505ce06ba5fae4f48`, the commit named in
[issue #650](https://github.com/LyraCoreProject/LyraCore/issues/650). It uses CMaNGOS Classic commit
`8ec338a1704e7dcb1c0213eb7ed58f9231ade40f` and the `wow_world_messages` 0.3.0 source commit
`9a1162cb1696518c4c38c794c18e693f0bd68f51`. No live Realm or client was used.

## Confirmed protocol behavior

CMaNGOS initializes `PLAYER_FIELD_WATCHED_FACTION_INDEX` to signed `-1`. It restores the field
from the saved Character value and saves its raw 32-bit representation. Its SQL column is unsigned,
so the saved clear value is `4294967295`. The SQL default of zero is not the Character creation
default. [Creation](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Player.cpp#L804),
[restoration](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Player.cpp#L14146),
[saving](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Player.cpp#L15722-L15723),
[SQL column](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/sql/base/characters.sql#L684).

`CMSG_SET_WATCHED_FACTION` is opcode `0x0318`. The CMaNGOS handler reads one signed 32-bit
value and puts it into that field. It does not change reputation standing and has no range Gate.
[Opcode](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Server/Opcodes.h#L830),
[handler](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/CharacterHandler.cpp#L1119-L1125).

The field occupies one 32-bit descriptor at absolute index `0x4ED`, or `1261`. CMaNGOS marks it
private to the owning Character. The bundled update-mask setter accepts `i32` at the same index.
[CMaNGOS field](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/UpdateFields.cpp#L315),
[setter](https://github.com/gtker/wow_messages/blob/9a1162cb1696518c4c38c794c18e693f0bd68f51/wow_world_messages/src/helper/vanilla/update_mask/impls.rs#L1527-L1530).

The selection names a reputation-list slot. `Faction.dbc` has separate fields for the faction ID
and signed `reputationListID`. CMaNGOS keys its reputation state by `reputationListID` and sends
64 initial slots. This supports a safe selection range of `0..=63`, with `-1` for no selection.
Checking that range is a proposed Gate, not behavior copied from the upstream request handler.
[DBC fields](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Server/DBCStructure.h#L359-L385),
[state keys](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Reputation/ReputationMgr.cpp#L243-L262),
[initial slots](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Reputation/ReputationMgr.cpp#L197-L230).

The bundled generated request decoder disagrees with CMaNGOS. It expects a two-byte `Faction`
enum. Read exactly four little-endian bytes as `i32` for this opcode.
[Generated decoder](https://github.com/gtker/wow_messages/blob/9a1162cb1696518c4c38c794c18e693f0bd68f51/wow_world_messages/src/world/vanilla/cmsg_set_watched_faction.rs#L11-L29).

## Confirmed omissions and the reported symptom

At the inspected LyraCore commit, the Character CREATE builder does not set the watched field.
The opcode path checks for a four-byte body, then returns an unavailable notice. The durable
Character row has no watched selection. Login initializes reputation slots in a separate packet.
[CREATE](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/codec/entity.rs#L303-L484),
[request path](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/world/handlers/unavailable.rs#L206-L208),
[Character row](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/module/src/character.rs),
[initial factions](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/codec/entity.rs#L730-L757).

Those omissions establish missing initialization and persistence. They do not establish that the
Bloodsail Buccaneers display is the watched bar, that Bloodsail occupies an omitted field's client
default slot, or that standing changed. The issue itself leaves those facts unconfirmed.
[Reported symptom](https://github.com/LyraCoreProject/LyraCore/issues/650).

## Chosen implementation

Append `watched_faction_index: i32` to the durable Character row with a default of `-1`. This gives
fresh and preexisting Characters the same no-selection value. Normal Transfers carry the selection
with the whole Character row, which the existing Transfer code serializes and decodes.
[Serialization](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/module/src/transfer/transport.rs#L625-L649),
[decoding](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/module/src/transfer/mod.rs#L1492-L1500).

Old in-flight Escrow contains Character bytes without the appended field. Add a narrow decode
compatibility path for that prior row shape, restoring its watched selection as `-1`. Keep malformed
or otherwise unsupported rows as a Refusal. The baseline decoder reads directly into the current
Character type, so a durable column default alone does not handle those old bytes.
[Baseline decoder](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/module/src/transfer/mod.rs#L1492-L1500).

No separate preference table or deletion registration is needed. Character deletion removes the
whole durable row, including the selection.
[Character deletion](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/module/src/world.rs#L1243-L1245).

The Gateway should decode the intent and make a Durable Request. The Module should admit `-1`
and `0..=63`, refuse other values without changing the prior selection, and require current Actor
authority and a live Character entity. The live entity disappears before Transfer exports state.
Avoid adding a taxi restriction accidentally, because the general `actor` helper also refuses
actions during flight. Send the restored selection on every self CREATE, including zero and `-1`.
Use an owner-only field update if the durable selection changes during the World Session.
[Actor authority](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/module/src/account_ownership.rs#L481-L529),
[actor helper](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/module/src/gw.rs#L152-L160),
[Transfer freeze](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/module/src/transfer/mod.rs#L1146-L1160).

## Verification to perform

These are proposed checks, not recorded results.

- At the protocol Seam, accept exactly four bytes for `-1`, `0` and `63`. Refuse truncation and
  trailing bytes. At the Module boundary, refuse `-2`, `64` and extreme signed values without
  changing the selected index or reputation standings.
- Decode self CREATE and owner updates. Assert field `1261` explicitly contains the expected value,
  including `FFFFFFFF` for clear and `00000000` for slot zero. Check a fresh Character and a
  preexisting Character with no saved selection.
- Select, change and clear a faction. Check each result after login, a same-Shard map change and a
  cross-shard Transfer. Check that old Escrow without the appended Character field imports as no
  selection, and that malformed rows still cause a Refusal.
- With a real 1.12.1 build-5875 client, record whether the reported Bloodsail display is the watched
  bar or a reputation-pane row, when it appears, and whether selecting and clearing survives the
  lifecycle steps above. Record standing before and after. Keep this symptom outstanding until
  that check establishes a connection.
