# Quest log at world entry

Research for [issue #647](https://github.com/LyraCoreProject/LyraCore/issues/647), checked on
2026-10-10 against LyraCore commit `46bf948b27b6d21555832b7505ce06ba5fae4f48`.
No real-client reproduction or production inspection was performed.

## Finding

The strongest code-level explanation is the gap between Character CREATE and the following quest-log
VALUES update. Populate the self CREATE with the existing quest slots so world entry presents the
current log at once. Whether that change removes the reported acceptance feedback still needs a real
1.12.1 build 5875 client check.

The report does not establish a durable progress reset. It supplies neither the affected map pair
nor the deployed commit. [Issue report](https://github.com/LyraCoreProject/LyraCore/issues/647).

## Confirmed in LyraCore

- `MSG_MOVE_WORLDPORT_ACK` routes the Character to its Home Shard and calls `enter_world` with
  `WorldEntry::WorldPort`. Both same-Shard map changes and cross-shard Transfer reuse the world-entry
  path. [World-port handler](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/world/handlers/char.rs#L468-L524).
- World entry builds a self CREATE without a quest-slot input, then sends the quest slots in a separate
  raw VALUES update. That update restores all 20 slots, including zero values for unused slots.
  [Entry CREATE](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/world/handlers/char.rs#L165-L186),
  [post-CREATE sync](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/world/handlers/char.rs#L282-L294),
  [slot mask](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/codec/update_mask.rs#L327-L347).
- The descriptor layout starts at index 198 and has 20 slots of three u32 fields. Each slot contains
  a quest ID, packed counters and state, then a timer. The existing codec packs four six-bit counters
  in the low 24 bits and state in the high byte.
  [Quest field codec](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/codec/update_mask.rs#L298-L345).
- `CharacterQuest` carries counts, reward state, failure state and a deadline. Its Transfer registry
  includes the quest rows and re-mints only the surrogate `id`.
  [Quest row and Transfer registration](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/module/src/quest.rs#L393-L466).
- The quest-log descriptor Gate defaults to enabled. An old world-entry comment saying it awaits
  verification is stale.
  [Configuration](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/config.rs#L566-L575).

## CMaNGOS comparison

All CMaNGOS links use the issue's pinned commit `8ec338a1704e7dcb1c0213eb7ed58f9231ade40f`.

Its explicit quest-accept handler checks the quest giver and quest eligibility before `AddQuest`.
That shows how acceptance occurs, but does not explain the reported client feedback by itself.
[Quest acceptance](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Quests/QuestHandler.cpp#L105-L175).

`Player::_LoadQuestStatus` restores quest IDs, timers, completion or failure state, and counters into
the Character descriptor. It also clears the unused tail.
[Quest initialization](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Player.cpp#L13960-L14048).

`Object::BuildCreateUpdateBlockForPlayer` writes the descriptor values inside the CREATE block.
`_SetCreateBits` selects populated fields that the recipient can see. This supports carrying the
existing quest state in the initial CREATE.
[CREATE encoding](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Object.cpp#L144-L187),
[recipient fields and CREATE mask](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/Object.cpp#L680-L770).
The field table agrees with index 198 and the three-field slot stride.
[Quest descriptor fields](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/UpdateFields.cpp#L140-L179).

## Client evidence and limits

An archived copy of Blizzard's 1.12.1 FrameXML listens for `QUEST_LOG_UPDATE`,
`QUEST_WATCH_UPDATE` and `UNIT_QUEST_LOG_CHANGED`. Its quest frame does not handle `QUEST_ACCEPTED`.
The Lua code updates the quest log and watch list through client APIs; it does not show the native
packet-to-feedback rule.
[Archived quest UI](https://github.com/MOUZU/Blizzard-WoW-Interface/blob/d162a4c0d198a4381b5b6573d975635ed7316702/1.12.1/FrameXML/QuestLogFrame.lua#L42-L87).
The same archive contains the `ERR_QUEST_ACCEPTED_S` text used for acceptance feedback.
[Acceptance text](https://github.com/MOUZU/Blizzard-WoW-Interface/blob/d162a4c0d198a4381b5b6573d975635ed7316702/1.12.1/FrameXML/GlobalStrings.lua#L1788).

The proposed cause is therefore an inference: the client may treat an absent quest ID followed by
a populated ID in VALUES as a newly accepted quest. The inspected source does not prove that native
client behavior. A Headless Client can check packet contents and ordering, but cannot prove that
acceptance text or sound stops.

## Fix and verification

Use the existing quest-slot read and mask in self CREATE. Remove the separate initial quest-log
sync. Keep the live quest Relay for acceptance, progress and removal. The existing regression that
expects a post-CREATE update must change to check quest fields inside CREATE.
[Current entry regression](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/world/quest_tests.rs#L90-L122),
[live Relay](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/stdb/subscriptions.rs#L3426-L3441).

Check fresh login, same-Shard map change and cross-shard Transfer through the World Session Seam.
Inspect CREATE quest IDs, counters and state, including empty and multiple-quest logs. Check that no
initial VALUES update introduces the quests after CREATE. Keep the disabled descriptor Gate covered.
For Transfer, compare durable quest rows before and after arrival, excluding re-minted row IDs.

At a real client, record the map pair, quest IDs and progress. Cross each boundary, return, and
reconnect. Verify that acceptance feedback does not repeat, progress stays intact and a subsequent
objective still updates. Record the exact build and Gateway commit.

An existing limitation remains outside this packet-order change. `build_quest_log_slots` derives
state only from objective completion and always emits timer zero, so it does not project `failed`
or `deadline_micros`. This research does not claim to fix timed or failed quest display.
[Current slot read](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/stdb/reads/quest.rs#L38-L66).

## Implementation checks

The change now puts the quest snapshot in self CREATE. After viewer registration, a writer job
compares the latest quest slots with that snapshot. It sends a full quest VALUES update only when
the slots changed, including when the final quest disappeared. Both this job and live quest Relays
read on the writer, so an earlier snapshot cannot overwrite newer queued progress.

Local checks on 2026-10-10:

- The login regression failed before the fix with quest ID 0 in CREATE instead of 777.
- `cargo test -p lyracore-gateway` passed 2,124 tests, with 26 ignored. A final targeted run with
  `--bin lyracore-gateway quest` passed 138 tests after adding reconciliation-failure coverage
  and populated inventory and skills to the descriptor preservation test.
- `cargo test -p lyracore-module --test quest_transfer -- --ignored --nocapture` passed. Two private
  Shards preserved four quests through Escrow, import, finish, release and world-port entry.
  Counters, rewarded and failed flags, and deadlines matched; the source quest rows were removed.
- Workspace formatting and Clippy with all targets and features passed.
- Two adversarial reviewers found the registration gap in the first review. Both accepted the
  reconciliation fix in the second review.

These checks do not establish the real client's acceptance-feedback behavior. That remains the
client check described above.
