# Profession trainer icons

Investigated issue [649](https://github.com/LyraCoreProject/LyraCore/issues/649) at
LyraCore commit `46bf948b27b6d21555832b7505ce06ba5fae4f48`. No real-client reproduction
or live realm access formed part of this investigation.

The user identified Fizzlegear in Ironforge and the training window. This points to
Springspindle Fizzlegear, creature entry 5174, whom the
[pinned ClassicDB source](https://github.com/cmangos/classic-db/blob/cd0c426a3b2ff56dd518bf009025299468e60fdb/Full_DB/ClassicDB_1_12_1_z2815.sql.gz)
names "Artisan Engineer". The user has not identified an individual offering.

## Confirmed packet defect

The [trainer codec](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/codec/trainer.rs#L31-L67)
always sends trainer list type 0. In build 5875, type 2 enables the created-item
icon lookup for recipe offerings. Type 0 skips that lookup and uses the offering
spell's own icon. This gives a direct cause for incorrect crafted recipe icons.

The archived [1.12.1 FrameXML](https://github.com/MOUZU/Blizzard-WoW-Interface/blob/d162a4c0d198a4381b5b6573d975635ed7316702/1.12.1/FrameXML/ClassTrainerFrame.lua#L251)
sets the selected service icon through `GetTrainerServiceIcon`.
The [Benilla transcription](https://github.com/Mathih13/benilla/blob/f7666010b182bb0478ee06fbfc6ac3e75e60a2dc/crates/benilla-app/src/ui_trainer/law.rs#L32)
records the native function's rule. I checked its decisive instructions against
the local 1.12.1 executable, without launching it.

Executable: an unmodified 1.12.1 build 5875 `WoW.exe`.
SHA-256: `b4756d38ef207c02ed651f4952bd89a70b4857b73a33413339e1b285b28d2dc7`.

```sh
objdump -d --start-address=0x4d8f50 --stop-address=0x4d9150 "$WOW_5875_EXE"
```

At `0x4d8fe2`, the function compares the current trainer list type at `0xb73a08`
with 2. Other types branch to `0x4d9008`, which reads the original offering's
`SpellIconID`. Type 2 scans for effect 36 or 57 at `0x4d8ff5` and `0x4d8ffa`,
reads its trigger spell at `0x4d9050`, then reads that spell's product item at
`0x4d906a`. A product item uses its item display icon. A missing product falls
back to the original offering's icon. The same global equals 2 in
`IsTradeskillTrainer` at `0x4d8ea0`.

## Engineering example from pinned data

I downloaded the pinned ClassicDB archive into `/tmp/lyra649-classic.sql.gz`
and checked its decompressed SHA-256 against the import lock. It matched
`d2083bcd2670451279cbf93af138eadae04c6d183a4cd0ff0357047e4a565de6`.
This was an offline file read, not a live shard read.

The source has 52 `npc_trainer` rows for Springspindle Fizzlegear. One offers
spell 3984, "Handful of Copper Bolts", with required skill line 202 and skill
rank 30. Its first spell effect is 36 and triggers recipe 3922. That recipe's
first effect is 24 and creates item 4359, also "Handful of Copper Bolts", with
item display 10700. The offering's own `SpellIconID` is 1. Type 0 chooses that
spell icon; type 2 chooses the product item's display icon. These facts come
from `npc_trainer`, `spell_template`, and `item_template` in the
[same pinned archive](https://github.com/cmangos/classic-db/blob/cd0c426a3b2ff56dd518bf009025299468e60fdb/Full_DB/ClassicDB_1_12_1_z2815.sql.gz).
This is an example for verification, not a claim that the user selected it.

## Correct type and offering identity

CMaNGOS [sends each original offering ID and the trainer list type](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/NPCHandler.cpp#L99-L172).
Its [loader sets list type 2](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Globals/ObjectMgr.cpp#L8967-L8968)
when an offering's first trigger teaches a profession.
The [profession test](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/SpellMgr.cpp#L1117-L1131)
checks effect slot 1 for a skill effect and a profession skill line. Other lists
default to 0. This differs from creature template trainer types.
The [skill predicate](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/SpellMgr.h#L1924-L1933)
includes primary professions, Fishing, Cooking, and First Aid. Riding is a
separate case, and combat skill lines are not professions.

Copying the creature template type would also change weapon master and riding
lists. LyraCore [records weapon masters as template type 2 and riding as 1](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/crates/lyracore-shared/src/trainer.rs#L9-L25).
Use offering facts to derive the packet type. Recognized profession learn rows
provide those facts in the [current importer](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/importer/src/main.rs#L5090-L5164).
The fix uses those learn rows to identify all twelve imported professions. A custom
list with recipes but no profession learn row retains type 0. Preserve the
original offering ID so the icon lookup and purchase address the same offering.

## Other observations

The same reference packet writes talent cost 0 for all offerings and sets
`first_rank` only for the first rank of a primary profession. LyraCore sets both
fields to 1 for every skill learn row. That is a separate purchase-button defect;
it does not control the icon lookup above.

The [gossip definition](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/GossipDef.h#L64-L75)
assigns the trainer book icon value 3. LyraCore copies imported gossip icons in
[the importer](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/importer/src/main.rs#L2469-L2488),
and [the codec](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/codec/npc.rs#L89-L120)
only adds vendor and innkeeper fallback actions. No trainer gossip defect was
confirmed. Creature NPC flags pass through to the client in
[entity encoding](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/codec/entity.rs#L507).
No cursor defect was confirmed.

The report identifies an Ironforge engineering trainer and the training window.
The packet defect explains crafted recipe service icons there. The exact
offering the user selected remains unknown. Verify Handful of Copper Bolts at
Springspindle Fizzlegear, then a gathering profession, a secondary profession,
and a class trainer with a real client after the packet fix.
