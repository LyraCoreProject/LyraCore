# PB012 attended client handoff

Status: prepared handoff. The Quest fallback correction passes the 25-bot dispersion test and the unchanged productive-target test. Its fresh imported hour is running. Core CI passes and final Package CI continues. Argus access, deployment approval, the deployed Gateway and human attendance remain pending. No attended client observation or configured Realm operation has occurred.

The venue is the Argus development Realm at `/home/lyracore/LyraCore`, operated as service user `lyracore`. Root login is still unavailable. The user will attend with a real 1.12.1 client. Venue selection does not authorize deployment.

## Candidate and evidence

The candidate retains useful Quest targets and avoids foreign Loot Tags during autonomous Quest and Grind work. Teleport now replaces a stale movement leg with a stop at the destination, so a released ghost stays at its graveyard. Recovery records one capacity Refusal while preserving reevaluation, healing and ordinary expiry. Defense and explicit companion assistance retain their existing rules. Quest fallback now varies its choice among three nearby eligible targets using the existing Character salt.

- Core PR #516 head `24ab2c718f9451be993922ac948ddeff02a29b5e`, tree `2dda9d6d293cd5e19e94f0838af1ac3b8ab31277`.
- Package Collection PR #36 head `6fb2689111a14624b394a65431b40d2aab632ff2`, tree `791a735c99fc58942ffb89f953a24234235f7dba`.
- `playerbots` tree `d22e89801ff2e333994cefba09cb3284b00785dd`.
- `dungeons` tree `8969ccc9475d1f8b28687b7cd818d85165f7e05f`.
- Package content identity: `1e849fdc1d99e92495dcc9ed0c6ced3038344cc9a266535a230841052c80bf51`.
- Recorded Module Wasm: `/home/t3agent/.cache/playerbots-rewrite/evidence/pb012-quest-dispersion-24ab2c71-2fdc43d0-green-v1/lyracore_module.wasm`, SHA-256 `07b74b12851aa15f05d75905d6b66f7cbbafe0bc5b977630b628154c10488ab7`, 9,665,319 bytes.
- Build manifest: `/home/t3agent/.cache/playerbots-rewrite/evidence/pb012-quest-dispersion-24ab2c71-2fdc43d0-green-v1/build-manifest.json`, SHA-256 `6b0d303eebba4d7f109bbfdeb718b7ca02ba05461b52e06e4736488d38a4677f`.

The recorded build uses Core `24ab2c718f9451be993922ac948ddeff02a29b5e` and Collection `2fdc43d0f85cad25296a877578917af74aeac54e`, tree `34394159414ff5ee1a06bed0fcc06f0e027c73bb`. The final Collection changes only four Core CI references. Its playerbots and dungeons trees match the build exactly. The session template records the current PR source and actual Wasm source separately.

The recorded build passes both focused Quest cases. At one timestamp, 25 ungrouped bots choose the three eligible alternatives in a distribution of 8, 10 and 7, excluding the foreign-tagged primary. The unchanged productive-target case retains useful work through primary respawn, switches after foreign retagging and earns ordinary Quest 7 credit with sole corpse entitlement. Independent Standards and Spec review is clear. Result SHA-256 `b446d6fc2687181cedfe3f5ca098becc1bbeb1e98ac046ffda1458d96b40c746`; review `578140feaa3e6f5acefe812ae52585cfd8a2fed447c38ab7cdaaa63a1c433a15`.

The preceding imported hour used Core `607ca975` and Collection `be599eb0`. It completed all 3,600 seconds with 25 bots, clear static geometry and zero Refusal-spam candidates. Imported behavior failed because bots repeatedly chose the same Quest fallback and lost its Loot Tag race. Seven advancement gaps remained open, and one Grind interval lacked target evidence. Independent assessment SHA-256 `47a8a266458a86944ee09c0e1e8e30899d5116e98ba4c4ae064aa40dff8b6107`. The new focused correction passes. Its full hour is running with the complete eight-entry Grind capture scope.

The original driver exited 1 during private-directory cleanup after capture. Its original run and traceback remain preserved. Separate reviewed receipts record the stopped owned process and completed private cleanup. This repair does not change the failed behavior result.

Earlier productive-target, relocation, recovery and Transfer evidence remains at its actual source identities. Core `24ab2c71` passes its current CI. Final CI continues at Collection `6fb26891`. All seven gameplay and firewall jobs passed at preceding Collection `c4960b5b` in [canonical run 34700753123](https://github.com/LyraCoreProject/packages/actions/runs/34700753123). General module and datascripts checks still depend on the Core API merge. Both source PRs remain unmerged.

The deployed Gateway executable remains pending the Argus candidate build. Record and rehash that exact file before login.

The Map 36 importer was merged by Core commit `375b38c3004b61500ce480a72a681d7999a5c9ca`. Git records its exact merge tree as `22fbc76a174a6f60abe2bea3d8148c047afff149`, equal to the reviewed PR #515 source tree. The accepted importer binary SHA-256 is `0a3270f84023745ef563a5ed7772e056f6db95098bd3229ee5ab720562e51abd`. Its client archive manifest is `/tmp/playerbots-rewrite/client-baseline/source-manifest.json`, SHA-256 `9320f727dcfbfc94a9a0f626ca1f0eaffc30546cc2a39deea01627506f17d078`.

The private Gateway and Headless Client route passed at Core `ccbe3ffb731e47a85bc4dfaca98a093a0c41d05e` and Package `590c2b1ff3731cbee0ed96f7ff52c3312fc89202`. That run proved server-side import, Follow movement, AreaTrigger 78 entry, AreaTrigger 119 exit, and five-member Transfer settlement. It did not prove rendered geometry or visible clipping in a real client. The result is `/tmp/playerbots-rewrite/pb012-map36-private-route-ccbe3ffb-590c2b1f-v9-result.md`, SHA-256 `a432681423bb0775872adb6ac717eb4431f3d3988a3c999448ac694ad2edec77`. Independent review is `/tmp/playerbots-rewrite/pb012-map36-private-route-v9-007-evidence-review.md`, SHA-256 `83ff94b948cae2fa5d907690f58ea353a85dbd8642208367dc3006f4220981d1`.

Use `docs/research/playerbots-client-session-template.json` for the attended record. Its corrected candidate source, recorded Module Wasm, build manifest and Package content fields are filled from the actual build. Deployed Gateway, client, staging, observer, time, attended evidence and signoff fields remain pending. Later caller-only changes do not change the actual Wasm source identity.

## Client and staged identities

Use an unmodified World of Warcraft 1.12.1.5875 client. Record the executable hash and an installation inventory before login. The previously discovered local installation contains a package patch archive and does not yet satisfy this condition. Preserve it and select a verified installation instead.

Stage one authenticated human leader and four companions:

- Warrior Tank
- Priest Healer
- Mage Damage
- Mage Damage

The future Argus staging run generates the Character and hostile GUIDs used for this session. Record them as exact decimal strings in the session manifest before sending any addon command. Lua 5.0 numbers cannot safely carry every 64-bit GUID, so keep each GUID as text.

The accepted private run used leader `1000006`, Warrior `1000001`, Priest `1000011`, and Mages `1000016` and `1000021`. Those values identify private fixture evidence only. Never paste them into an attended command unless the new Argus staging record independently produces the same exact value.

Choose declared hostile creatures before combat and record their GUIDs. The human leader must have one supported ordinary control spell for the control step. Record its spell id and name.

## 1.12.1 command codec

Send addon traffic on the `PARTY` channel with prefix `STC`. Do not use addon `WHISPER`, which this client version does not support. Each command is one `1/1` envelope:

```lua
/run SendAddonMessage("STC","v1|playerbots.order|SEQUENCE|1/1|follow|BOT_GUID","PARTY")
/run SendAddonMessage("STC","v1|playerbots.order|SEQUENCE|1/1|stay|BOT_GUID","PARTY")
/run SendAddonMessage("STC","v1|playerbots.order|SEQUENCE|1/1|assist|BOT_GUID|MEMBER_GUID","PARTY")
/run SendAddonMessage("STC","v1|playerbots.order|SEQUENCE|1/1|target|BOT_GUID|HOSTILE_GUID","PARTY")
```

Replace every placeholder from the new session manifest. Increment `SEQUENCE` for each envelope. Record the exact outbound text and the returned payload:

```text
STC\tv1|playerbots.order.result|0|1/1|INTENT_ID|OUTCOME
```

The envelope sequence orders transport. The Module creates the durable intent id and issuance sequence. Record both. An unexpected Refusal, absent reply, or reply for another intent stops that step.

## Before the user connects

The Operator should use the repository's guarded Realm procedure after the user approves deployment. Do not improvise direct database or service commands.

1. Confirm the Core and Package source identities above are merged or approved for the attended candidate. Require clean checkouts.
2. Bind the final Wasm, build manifest, Gateway executable, importer, archive inventory, and route revision in the session manifest. Rehash each deployed artifact on Argus.
3. Reconcile the tracked Standalone Supervisor and Gateway, then verify their exact running artifact identities. Save the reconciliation and service status outputs.
4. Apply the approved World Import Profiles to their owned World Shards and Instance Pool. Require one complete active Map 36 vmap generation and matching receipts on the Instance Pool. Keep Map 36 terrain, navigation, and Navigation Coverage absent.
5. Stage the human, four companions, party, roles, Companion Orders, declared hostiles, and the control spell. Save the resulting decimal GUIDs in the session manifest.
6. Confirm the client is 1.12.1.5875 and unmodified. Save its executable hash and installation inventory.
7. Create one evidence directory named with the UTC start time. Copy the filled pre-run manifest there before login.

Deployment approval and user attendance should be the final two decisions. Root access to Argus is the remaining access dependency.

## Attended steps

For every step, record UTC start and finish times, pass, fail, or not observed, visible behavior, the exact command and reply, and a media timestamp when motion or rendering matters. Save a Durable Read before and after the action. A visible failure remains a failure even when internal state reports success.

1. Issue Follow to all four companions. Walk, stop, turn, and move around one real obstacle. Confirm legal ground movement, stable spacing, and no unrequested pull.
2. Issue Stay to one Mage. Move the party away and pause. Confirm the Mage stays near the commanded point. Issue Follow and confirm the same Mage rejoins with its Damage role retained.
3. Issue Target to the Warrior for one declared eligible hostile. Confirm the selected creature becomes the fight target. Observe Warrior threat, Priest healing, and both Mage damage roles.
4. Issue Assist to one Mage for the human leader or another recorded party member. Engage a second declared hostile with that member. Confirm the Mage changes to that fight and releases the old target.
5. During ordinary combat, retain at least one completed Priest heal and one completed Mage damage cast. Wound the party without debug state writes, then leave combat. Confirm recovery restores health and missing buffs without repeated starts or an idle injured Healer.
6. Cast the recorded control spell on one hostile. Confirm stale attacks and casts against that creature stop. Confirm another useful action may continue and record any explicit survival exception.
7. Let one companion die in ordinary combat. Confirm its death state, release, resurrection, retained role and Companion Order, and return to the group. Confirm no companion starts an unrequested pull during regrouping.
8. Walk to the Deadmines entrance and send AreaTrigger 78 through normal client movement. Confirm the human and all four companions render on the Map 36 floor in the same nonzero instance. Walk from the landing `(-14.5732, -385.475, 62.4561)` toward the supported exit approach `(-14.4154, -391.4037, 63.7006)`. Confirm visible floor support, no clipping, and ordinary Follow movement. Send AreaTrigger 119 only after that observation. Confirm the whole party returns to Map 0 near `(-11208.7, 1675.9, 24.5733)`, with roles and Follow Orders retained and no old-location action.
9. Leave the party. Confirm companion control releases and eligible solo work resumes. Regroup, restore the recorded roles and commands, then confirm behavior matches the pre-release party.

Stop the run on a client disconnect, unexplained stall, visible clipping, unrequested pull, repeated cast start, idle injured Healer, lost role or order, split party, or unresolved Transfer.

## Capture and read list

Keep client media and server evidence on the same UTC timeline. At each boundary, record the exact Character positions and the rows relevant to that step.

- Candidate: Core and Package commits and trees, source cleanliness, Module Wasm, Gateway, importer, client executable, client installation, archive inventory, and active Package trees.
- Party: group, membership, partition, Realm locator, Character, world entity, bot, role, Companion Order, and command intent and receipt rows.
- Runner: Bot Objective, Foreground Action, chosen Candidate, history, movement and cast progress, recovery, failures, and deferred destinations.
- Effects: current Action, pending cast, cast receipt, impact receipt, health, aura, threat, and melee rows.
- Death: Character and world entity death fields, corpse, ghost, resurrection, Runner history, role, and Companion Order.
- Transfer: Transfer Intent, transfer out, transfer in, Character, world entity, instance binding, partition, and Realm locator on both sides, plus Gateway operation logs.
- Map 36: active vmap generation, all receipts, config, navigation revision, and explicit absence of terrain, navigation, coverage, and coverage manifest rows. Capture them before entry, before exit, and after return.
- Transport: every addon envelope and reply, relevant client movement and AreaTrigger packets, Gateway log, Module log, and the attended observer's media timestamps.

Copy raw command output without editing it. Record each capture command, exit status, stdout and stderr hash. Finish with a manifest of every regular file and symlink in the evidence directory, then rehash the candidate and client identities. Record service status and verify no test-only private process remains.

## Read-only diagnostic command appendix

This appendix prepares the diagnostic reads. It does not authorize or perform an Argus read. The guarded Operator must supply the installed `spacetime` path, server binding, database names, and token context after Argus access is restored. Do not substitute a direct database connection. Record those values in `operator_read_binding` in the session manifest and save the exact command, exit status, stdout, and stderr for every invocation.

Use this read-only command shape. `SPACETIME_BIN`, `SPACETIME_SERVER`, and each database value are unavailable until the Operator binds the deployed Realm:

```bash
"$SPACETIME_BIN" sql --server "$SPACETIME_SERVER" "$DATABASE" "$SQL"
```

Before any read, replace these tokens from the future staging record. None comes from the accepted private fixture:

| Token | Session manifest field |
| --- | --- |
| `LEADER_GUID` | `staging.leader.guid` |
| `WARRIOR_GUID` | `staging.companions.warrior_tank.guid` |
| `PRIEST_GUID` | `staging.companions.priest_healer.guid` |
| `MAGE_ONE_GUID` | `staging.companions.mage_damage_one.guid` |
| `MAGE_TWO_GUID` | `staging.companions.mage_damage_two.guid` |
| `HOSTILE_GUID` | one entry in `staging.declared_hostiles` |
| `GROUP_ID` | `partitions.group_id` |
| `MAP36_INSTANCE_ID` | `partitions.map36_instance_id` |
| `MAP36_GENERATION_ID` | `partitions.map36_generation_id` |
| Realm database | `operator_read_binding.realm_database` |
| open-world database | `operator_read_binding.open_world_database` |
| Instance Pool database | `operator_read_binding.instance_pool_database` |

Run the Realm reads before login, after staging, before Map 36 entry, after Map 36 entry, after return, after party release, and after regrouping:

```sql
SELECT * FROM game_group WHERE group_id = GROUP_ID
SELECT * FROM game_group_roster_revision WHERE group_id = GROUP_ID
SELECT * FROM game_group_member WHERE group_id = GROUP_ID
SELECT * FROM game_group_member_partition WHERE group_id = GROUP_ID
SELECT * FROM game_character_shard WHERE character_guid = LEADER_GUID
SELECT * FROM game_character_shard WHERE character_guid = WARRIOR_GUID
SELECT * FROM game_character_shard WHERE character_guid = PRIEST_GUID
SELECT * FROM game_character_shard WHERE character_guid = MAGE_ONE_GUID
SELECT * FROM game_character_shard WHERE character_guid = MAGE_TWO_GUID
```

Run the following reads once against the open-world database and once against the Instance Pool database at every action boundary. An absent row on one side is evidence and must be retained. Repeat the Character-specific statements for `LEADER_GUID`, `WARRIOR_GUID`, `PRIEST_GUID`, `MAGE_ONE_GUID`, and `MAGE_TWO_GUID`:

```sql
SELECT * FROM game_character WHERE guid = LEADER_GUID
SELECT guid, map_id, instance_id, x, y, z, health, max_health, dead, target_guid, player_flags FROM game_world_entity WHERE guid = LEADER_GUID
SELECT * FROM game_instance_binding WHERE character_guid = LEADER_GUID
SELECT * FROM game_bot_transfer_intent WHERE bot_guid = LEADER_GUID
SELECT * FROM game_transfer_out WHERE character_guid = LEADER_GUID
SELECT * FROM game_transfer_in WHERE character_guid = LEADER_GUID
SELECT * FROM pkg_playerbots_bot WHERE character_guid = LEADER_GUID
SELECT * FROM pkg_playerbots_companion_order WHERE character_guid = LEADER_GUID
SELECT * FROM pkg_playerbots_runner WHERE character_guid = LEADER_GUID
SELECT * FROM pkg_playerbots_action WHERE character_guid = LEADER_GUID
SELECT * FROM game_creature_spline WHERE guid = LEADER_GUID
SELECT * FROM game_pending_cast WHERE caster_guid = LEADER_GUID
SELECT * FROM game_pending_spell_impact WHERE caster_guid = LEADER_GUID
SELECT * FROM game_melee_attack WHERE attacker_guid = LEADER_GUID
SELECT * FROM game_aura WHERE target_guid = LEADER_GUID
SELECT * FROM game_corpse WHERE owner_guid = LEADER_GUID
SELECT * FROM game_resurrect_request WHERE target_guid = LEADER_GUID
SELECT * FROM pkg_playerbots_companion_combat_receipt WHERE attacker_guid = LEADER_GUID
SELECT * FROM pkg_playerbots_companion_cast_receipt WHERE caster_guid = LEADER_GUID
SELECT * FROM pkg_playerbots_companion_impact_receipt WHERE caster_guid = LEADER_GUID
```

The human leader normally has no `pkg_playerbots_*` row. Preserve that absence. For each declared hostile, replace `HOSTILE_GUID` and capture the fight boundary on the Character's current database:

```sql
SELECT guid, map_id, instance_id, x, y, z, health, max_health, dead, target_guid FROM game_world_entity WHERE guid = HOSTILE_GUID
SELECT * FROM game_threat WHERE creature_guid = HOSTILE_GUID
SELECT * FROM game_melee_attack WHERE target_guid = HOSTILE_GUID
SELECT * FROM game_pending_cast WHERE target_guid = HOSTILE_GUID
SELECT * FROM game_pending_spell_impact WHERE target_guid = HOSTILE_GUID
SELECT * FROM game_aura WHERE target_guid = HOSTILE_GUID
```

Capture the durable command boundary on both World databases after each addon reply. The intent lives on the issuer's current database; the receipt lives where the companion command was applied:

```sql
SELECT * FROM game_party_command_intent WHERE issuer_guid = LEADER_GUID
SELECT * FROM game_party_command_receipt WHERE bot_guid = WARRIOR_GUID
SELECT * FROM game_party_command_receipt WHERE bot_guid = PRIEST_GUID
SELECT * FROM game_party_command_receipt WHERE bot_guid = MAGE_ONE_GUID
SELECT * FROM game_party_command_receipt WHERE bot_guid = MAGE_TWO_GUID
```

Run this Map 36 set on the Instance Pool before entry, before exit, and after return. `MAP36_GENERATION_ID` comes from the first generation row and is then fixed in the session manifest. The generation must be active, its generation receipt and live chunk metadata must each contain the expected 11 rows, and the terrain, navigation, Navigation Coverage, and coverage manifest queries must remain empty:

```sql
SELECT id, map_id, state, expected_chunks, accepted_chunks, expected_bytes, manifest_digest, source_identity, selection_identity FROM game_vmap_generation WHERE map_id = 36
SELECT id, generation_id, shard_ordinal, key FROM game_vmap_generation_receipt WHERE generation_id = MAP36_GENERATION_ID
SELECT id, key, map_id, cell_x, cell_y FROM game_vmap_chunk WHERE map_id = 36
SELECT id, xp_rate, nav_enabled, hosts_instances, bots_idle, vmap_enabled, nav_coverage_enabled FROM game_config WHERE id = 0
SELECT * FROM game_navigation_revision
SELECT key, map_id, cell_x, cell_y FROM game_terrain_chunk WHERE map_id = 36
SELECT key, map_id, cell_x, cell_y, base_z FROM game_nav_chunk WHERE map_id = 36
SELECT generation_id, cell_key, map_id, cell_x, cell_y, base_z FROM game_vmap_nav_coverage WHERE map_id = 36
SELECT generation_id, map_id, cell_count, digest, complete FROM game_vmap_nav_coverage_manifest WHERE map_id = 36
```

The Operator must save each set under the matching `diagnostic_reads.boundaries` entry. This command preparation remains distinct from attended acceptance: all entries stay `not_run` until the guarded Argus reads and the human-observed steps occur.

## Acceptance boundary

This document remains `pending_human` until the user observes and signs the run. A pass requires every step above, complete source and artifact identity, preserved raw evidence, no unresolved finding, and explicit signoff. The separate imported-world one-hour observation and required automated suites remain independent acceptance requirements. The private Map 36 pass supplies server-side route evidence; it does not replace the attended visual check.
