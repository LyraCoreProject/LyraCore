# Deeprun Tram placed objects

Research for [issue 648](https://github.com/LyraCoreProject/LyraCore/issues/648), checked against
LyraCore commit `46bf948b27b6d21555832b7505ce06ba5fae4f48` on 2026-10-10. This note records source
inspection. It does not establish the deployed import configuration or a real client's result.

## Finding and fix scope

The canonical World Import Profiles omit map 369. Add whole-map coverage to `alliance-eastern`,
`starting-eastern`, and `alliance-single`. These profiles own the Eastern Kingdoms content that
connects to Deeprun Tram. Keep the import script's planned map lists consistent. Existing code can
import the placed objects and deliver their static CREATE packets. No new AOI mechanism is justified
by the evidence. Moving tram cars and client scenery need separate checks.

The profile catalogue currently covers bounded parts of maps 0 and 1. Only `alliance-single` and
`instances` include a whole map, map 36. The spawn predicate accepts every coordinate on a whole map,
so map 369 should use that path rather than a continent rectangle. The same predicate controls
creature and gameobject coverage.
[Profile catalogue](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/importer/src/world_import_scope.rs#L160-L198),
[scope predicate](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/importer/src/world_import_scope.rs#L288-L302),
[gameobject filter](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/importer/src/main.rs#L4918-L4956).

## What the external source proves

The issue's pinned CMaNGOS ClassicDB update adds a Deeprun Chest and replaces 88 Subway Bench spawns
on map 369. It also corrects other placed objects. This proves that some station objects require
gameobject rows. An update file is neither a complete census nor evidence of what an Operator's
assembled dump contains.
[ClassicDB update](https://github.com/cmangos/classic-db/blob/ec4f596146be6467ea93c57397858e329e2db852/Updates/4790_WDB-ironforge_deepruntram_fixes.sql#L35-L141).

Transport behavior needs more than static placement. CMaNGOS creates a separate elevator transport
for its transport type and sets transport flags, an update flag, pause time, and initial state.
LyraCore's existing CREATE builder always emits a stationary position, zero gameobject flags, and
`has_transport: 0`. Those differences justify a separate tram rendering and movement check. They do
not establish which missing object the play report meant.
[CMaNGOS creation](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/GameObject.cpp#L101-L106),
[transport fields](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Entities/GameObject.cpp#L194-L247),
[LyraCore CREATE](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/codec/gameobject.rs#L58-L93).

Client terrain and model geometry have a different input path, the Operator's client archives.
The object update does not prove missing archive scenery. Whole-map spawn coverage also adds no
terrain, navigation, or vmap extraction by itself. Those modes iterate explicit bounded slices or
Instance Vmap Slices. Keep geometry changes outside this fix unless archive or client evidence
identifies another defect.
[Ingestion sources](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/docs/data-ingestion.md#L34-L54),
[terrain iteration](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/importer/src/terrain.rs#L181-L203),
[navigation iteration](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/importer/src/nav.rs#L710-L734),
[vmap iteration](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/importer/src/vmap.rs#L458-L478).

## Shard assignment and CREATE delivery

The documented Shard Map sends map 1 to the Kalimdor World Shard and map 36 to the Instance Pool.
Unmatched maps resolve to the default World Shard, so map 369 belongs with the eastern import under
that topology. The shared dungeon catalogue contains only map 36. This is a conclusion about the
documented topology, not the live Realm's settings. A custom rule for map 369 must have matching
content on its owning Shard.
[Documented rules](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/docs/architecture.md#L330-L340),
[Shard Map resolution](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/config.rs#L203-L211),
[dungeon catalogue](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/crates/lyracore-shared/src/instance.rs#L1-L15).

The script independently lists planned maps and uses them for spatial ownership checks. Adding 369
only to the Rust catalogue leaves that contract inconsistent.
[Profile map lists](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/importer/scripts/import-manifest.sh#L38-L45),
[expected maps](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/importer/scripts/import-world.sh#L259-L267).

The importer retains the spawn's map, position, and quaternion and emits templates for used entries.
The Module loads static gameobjects into instance 0 and computes their AOI cells. The Gateway
subscribes to gameobjects and their templates. Insert, recenter, and resident sweep paths reach the
same Relay. That Relay requires a matching instance and a template on the holding Shard, then emits
CREATE followed by rotation VALUES. A spawn without its template produces no CREATE.
[Import rows and templates](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/importer/src/main.rs#L4930-L5035),
[Module loading](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/module/src/gameobject.rs#L1131-L1175),
[subscriptions](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/stdb/connection.rs#L1298-L1299),
[insert dispatch](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/stdb/world_view.rs#L1703-L1731),
[recenter and sweep](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/stdb/world_view.rs#L6141-L6208),
[CREATE Relay](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/stdb/subscriptions.rs#L1130-L1177).

## Verification and remaining limits

Add a planner test with authored map-369 objects at both station distances. Assert that eastern and
single profiles retain both spawns and templates, preserve map and rotation, and exclude them from
Kalimdor and Instance Pool profiles. Update profile catalogue and shell manifest checks. Existing
tests already cover whole-map spawn preservation, quaternion packing, CREATE serialization, and AOI
partition behavior. No external SQL rows or emulator code need to enter the test fixtures.
[Whole-map and quaternion tests](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/importer/src/main.rs#L8040-L8133),
[CREATE tests](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/codec/gameobject.rs#L199-L340),
[AOI comparison](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/gateway/src/stdb/world_index.rs#L635-L687).

No local assembled dump appeared in `.import/`. This research ran no import, tests, or live reads.
Offline tests can prove import planning and packet construction. They cannot prove deployed content,
visual rendering, or tram movement. After an authorized reimport, record the source stamp, owning
Shard, map-369 spawn count, and template completeness. A Headless Client can check CREATE delivery.
A real build-5875 client must inspect identifiable placed objects at both entrances and check tram
cars and archive scenery separately. The repository's Verification ladder requires these distinct
checks.
[Verification ladder](https://github.com/LyraCoreProject/LyraCore/blob/46bf948b27b6d21555832b7505ce06ba5fae4f48/docs/architecture.md#L706-L724).
