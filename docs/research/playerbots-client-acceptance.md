# PB012 attended client handoff

Status, 16 September: Argus is running Core `c7306184994148570a25a5aad67ae2c3bd18f3d0` and Collection `63f82468760aaa7eab6dec0077ae430163983ecb`. All four Shards, the managed Gateway, Package replay, navigation imports, and post-publish repair passed deployment verification. The attended Follow step failed because companions took about 3.5 seconds between decisions alongside 100 existing bots. The four companions are paused while the scheduler correction is validated. PB012 and old-code retirement remain pending.

The venue is the Argus test Realm at `/home/lyracore/LyraCore`, operated as service user `lyracore`. The user authorized deployment, and the Operator reports working root access. The user will attend with a real 1.12.1 client.

## Candidate and evidence

The 16 September deployment passed all six gates, with 3,743 tests passed and 30 ignored. The managed Gateway and Standalone were replaced and verified. The backup service still reports its 4 September failure, and its timer is disabled. The failed attended attempt is sealed at `/home/lyracore/deploy-logs/argus-playerbots-retest-20260916T0926Z`; its manifest SHA-256 is `be205030743c03b8231f284185897d4acca2a2ee55671c2055eb8308e4706915`.

The earlier Service Reconciliation failure was corrected by CLI PRs #60 and #61. The deployed Core pins CLI `f5faff4da0a73906dcbe102615b53929f0eab547`. The next scheduler candidate changes gameplay, so run the complete guarded deployment procedure for every configured Shard. Preserve all earlier evidence and bind the new deployed artifacts before another client attempt. The identities below describe historical automated gameplay evidence, not the next attended candidate.

The candidate retains useful Quest targets and avoids foreign Loot Tags during autonomous Quest and Grind work. Collection work can consider an eligible live target when the bounded corpse search is inconclusive. Defense retains a valid attacker and uses the learned class combat strategy. Completed Quest waits defer after two minutes without a verified effect; movement and foreign damage cannot reset that clock. Timed casts move inside nominal spell range before starting. Recovery checks whether a self-heal can start before interrupting other work and preserves a matching pending heal.

- Core PR #519 merged as `35495170d1226f0d97d9a173c4b348606fd08934`, tree `efc53fa0574d0427525f344325e8b13b711067b7`, equal to tested head `27d6f5337276dc461cdb376927ada8f4e5c9017b`.
- Package Collection PR #36 merged as `63f82468760aaa7eab6dec0077ae430163983ecb`, tree `0cae1cbfe32d5210d8e264365ba74c74736543b0`, equal to tested head `0622ca5eacfde95916e3e4eb1765fab1e1842090`.
- `playerbots` tree `3d5ddc591116a865485ecb0a4625d253ef802f81`.
- `dungeons` tree `8969ccc9475d1f8b28687b7cd818d85165f7e05f`.
- Package content identity: `7e2e5ca12108505cbce8863a9d0d2d592ba3c52b8f53c0cadb0a5767b8f9450b`.
- Recorded Module Wasm: `/home/t3agent/.cache/playerbots-rewrite/evidence/pb012-final-composed-build-27d6f533-54a7d39d-v3/lyracore_module.wasm`, SHA-256 `cfdf2aa51ebcce07e85607d4ec6dcf820c32255d76833cc6859366dce4495faa`, 9,792,774 bytes.
- Build manifest: `/home/t3agent/.cache/playerbots-rewrite/evidence/pb012-final-composed-build-27d6f533-54a7d39d-v3/build-manifest.json`, SHA-256 `43dea32545c7d06d7e790d3faba07a9cfb36e1cc3ae743901baf4439ab9f2363`.

The recorded build uses Core `27d6f5337276dc461cdb376927ada8f4e5c9017b` and Collection `54a7d39dd869e0aaf81c707e4b30728bf008d536`, tree `541d681b4a7007fe076e4bf942a8d3e6284e7b2f`. Delivery commit `0622ca5e` changes one debug fixture error message only. The imported hour never calls that helper, and production source is byte-identical. The recorded Wasm keeps its actual source identity. All 39 retained Package inputs match its manifest. Build, source and private cleanup checks pass. Build result SHA-256 `df63a65133c259c10d2a157cef7c80ab3c58181642082a40a47a76acf21f530c`. Final Core and Collection CI pass. Canonical Collection run `34725842185` and its duplicate PR run each pass all nine jobs. The retained audit confirms 170 durable tests, 47 companion and lifecycle cases, 27 Transfer cases, all 30 class journeys and the additional Warrior level-gap case. Audit SHA-256 `a91f71907eb165210e4857f80a331dd86df5a436a089c0431b7390ac60a4d853`; independent root artifact review `c364a60d83bebb952aff97a2fb9f61ff9da1aacd78f162c2b8a2191116365da9`.

The focused Recovery regression passes at Core `262d6b7b` and Collection `e0ed910e`. Insufficient mana preserves useful casting-position movement; a real cooldown preserves melee movement. An affordable pending Smite remains retained while the heal is unavailable. Once ready, Lesser Heal can preempt that cast and remains retained through completion. All three scenarios end with a resolved self-heal and 50 actual healing. Strict execution assessment SHA-256 `284ecd95ec8e4636f57a18de09b6cb0d04d3a962434f5fa62a51d6307800cdb5`; root review `1f729e54415202a379ceb691fab822fb2661db9a3bf388b888cf0bb0b564e491`. Independent Standards and Spec reviews are clear.

The accepted v19 observation ran 25 bots for 3,600 seconds and closed at 02:30:51 UTC on 13 September. All bots started at level 1, earned six starter Quest rewards, and ended at level 4 or 5. All 90,583 measured decisions have matching timing evidence. Both raw archives finalized within their bounds, and private cleanup passed. Input manifest SHA-256 `a18ed06c22bd744e5ec2707702d8d5cb624f92f795d016baae53711c08f8f997`; run `5fde8f19391b154f66e43135cbfae75aed5eea79a3f2a4c0e29e295beaf8d463`; root acceptance `e681419b347641418f4194c55f756ded100449405aa3c6a2d41a5d0b856deaa9`.

Independent reviews account for all 269 flagged intervals. There is no unexplained stall, repeated Refusal burst or idle injured healer. Three active Grind intervals remain right-censored, with target contention and short reassignments accounted for. Static Core geometry is clear across 146,375 assessed segments. Rendered-client clipping remains pending. The measured setup supports these 25 bots, with zero rolled-back transactions, maximum scheduler lag of 0.507 seconds and a 16.8% writer-time estimate. This is not a configured Realm capacity claim or an operating-system CPU measurement.

The earlier v18 hour remains failed. UTC log rotation left 18,464 measured decisions without timings and prevented final geometry capture. Recorder PR #21 now retains the live timing stream and exact measurement window; v19 uses that merged correction. Original failed run SHA-256 `a6b8b51de2840c212d884f380ba303a070366f30e19bd30f65c6490b4c55994d`. The preceding v17 hour remains failed for the Recovery readiness defect, now corrected and covered by focused regression and v19. Original v17 run `8e8814d01896161826735d155abad3967bf786460733c00110c88fbd37a5a49c`. Earlier evidence remains preserved.

The 16 September running Gateway SHA-256 is `5b36a5f846e1fb80095f09273e0037a1a8627dad65a3ce73bf95331b2650bd63`. Record and rehash the actual replacement before the next login.

The Map 36 importer merged as Core commit `375b38c3004b61500ce480a72a681d7999a5c9ca`, tree `22fbc76a174a6f60abe2bea3d8148c047afff149`, equal to the reviewed PR #515 source. The accepted importer binary SHA-256 is `0a3270f84023745ef563a5ed7772e056f6db95098bd3229ee5ab720562e51abd`. Its client archive manifest is `/tmp/playerbots-rewrite/client-baseline/source-manifest.json`, SHA-256 `9320f727dcfbfc94a9a0f626ca1f0eaffc30546cc2a39deea01627506f17d078`.

The private Gateway and Headless Client route passed at Core `ccbe3ffb731e47a85bc4dfaca98a093a0c41d05e` and Package `590c2b1ff3731cbee0ed96f7ff52c3312fc89202`. It proved server-side import, Follow movement, AreaTrigger 78 entry, AreaTrigger 119 exit and five-member Transfer settlement. Rendered geometry and visible clipping still require the real client. Result SHA-256 `a432681423bb0775872adb6ac717eb4431f3d3988a3c999448ac694ad2edec77`; independent review `83ff94b948cae2fa5d907690f58ea353a85dbd8642208367dc3006f4220981d1`.

Use `docs/research/playerbots-client-session-template.json` for the attended record. Candidate source identifies the current delivery revision. The separate Module source, recorded Wasm, build manifest and Package content fields identify the actual observed build. Deployed Gateway, client, staging, observer, time, attended evidence and signoff fields remain pending.

## Client and staged identities

Use an unmodified World of Warcraft 1.12.1.5875 client. Record the executable hash and an installation inventory before login. The previously discovered local installation contains a package patch archive and does not yet satisfy this condition. Preserve it and select a verified installation instead.

Stage one authenticated human leader and four companions:

- Warrior Tank
- Priest Healer
- Mage Damage
- Mage Damage

The paused Argus party is Pbguide `2117`, Tankbot1 `2126`, Healbot1 `2134`, Dpsbot1 `2143`, and Dpsbot2 `2152`, in group `12374`. Verify those rows and their current ownership and partitions after deployment. Record Character and hostile GUIDs as exact decimal strings in the new session manifest before sending any addon command. Lua 5.0 numbers cannot safely carry every 64-bit GUID, so keep each GUID as text.

The accepted private run used leader `1000006`, Warrior `1000001`, Priest `1000011`, and Mages `1000016` and `1000021`. Those values identify private fixture evidence only. Never paste them into an attended command unless the new Argus staging record independently produces the same exact value.

Choose declared hostile creatures before combat and record their GUIDs. The human leader must have one supported ordinary control spell for the control step. Record its spell id and name.

## 1.12.1 command codec

Send addon traffic on the `PARTY` channel with prefix `STC`. Build the two pipe characters inside Lua with `string.char(124,124)`. Pasting literal pipes through the native chat editor can add another escape layer. The attended Argus capture showed four pipes on the wire from a pasted `||` command. Runtime construction produced two pipes, a Module reply, and a visible client reply. The Gateway decodes this escape layer once. Each command is one `1/1` envelope:

```lua
/run SendAddonMessage("STC",table.concat({"v1","playerbots.order","SEQUENCE","1/1","follow","BOT_GUID"},string.char(124,124)),"PARTY")
/run SendAddonMessage("STC",table.concat({"v1","playerbots.order","SEQUENCE","1/1","stay","BOT_GUID"},string.char(124,124)),"PARTY")
/run SendAddonMessage("STC",table.concat({"v1","playerbots.order","SEQUENCE","1/1","assist","BOT_GUID","MEMBER_GUID"},string.char(124,124)),"PARTY")
/run SendAddonMessage("STC",table.concat({"v1","playerbots.order","SEQUENCE","1/1","target","BOT_GUID","HOSTILE_GUID"},string.char(124,124)),"PARTY")
```

Replace every placeholder from the new session manifest. Increment `SEQUENCE` for each envelope. Record the exact outbound text and the returned payload:

```text
STC\tv1|playerbots.order.result|0|1/1|INTENT_ID|OUTCOME
```

Display addon replies with a handler that replaces pipe characters before calling the chat frame renderer:

```lua
/run PB=CreateFrame("Frame");PB:RegisterEvent("CHAT_MSG_ADDON");PB:SetScript("OnEvent",function()if arg1=="STC" then DEFAULT_CHAT_FRAME:AddMessage((string.gsub(arg2,string.char(124),":")))end end)
```

The envelope sequence orders transport. The Module creates the durable intent id and issuance sequence. Record both. An unexpected Refusal, absent reply, or reply for another intent stops that step.

## Before the user connects

The user has authorized deployment on Argus. Use the repository's guarded Realm procedure for the remaining service, import and staging work.

1. Confirm the Core and Package source identities above are merged or approved for the attended candidate. Require clean checkouts.
2. Bind the final Wasm, build manifest, Gateway executable, importer, archive inventory, and route revision in the session manifest. Rehash each deployed artifact on Argus.
3. Reconcile the tracked Standalone Supervisor and Gateway, then verify their exact running artifact identities. Save the reconciliation and service status outputs.
4. Apply the approved World Import Profiles to their owned World Shards and Instance Pool. Require one complete active Map 36 vmap generation and matching receipts on the Instance Pool. Keep Map 36 terrain, navigation, and Navigation Coverage absent.
5. Stage the human, four companions, party, roles, Companion Orders, declared hostiles, and the control spell. Save the resulting decimal GUIDs in the session manifest. Inventory the existing bot population and controllers, then measure scheduler lag with that population present. The 16 September Argus run had 100 Legacy bots plus four companions sharing 16 decision slots per 500 ms pass. Decisions arrived about 3.5 seconds apart while movement legs covered only one second. Slow Follow failed attended acceptance. A new session requires measured decision intervals and movement under the retained population.
6. Confirm the client is 1.12.1.5875 and unmodified. Save its executable hash and installation inventory.
7. Create one evidence directory named with the UTC start time. Copy the filled pre-run manifest there before login.

Resume at the failed Service Reconciliation gate. The Operator must prove the replacement Gateway is running and healthy against all four published Shards before proceeding to imports, staging and login.

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

This appendix prepares the diagnostic reads. The guarded Operator must supply the installed `spacetime` path, server binding, database names, and token context from the authorized Argus session. Do not substitute a direct database connection. Record those values in `operator_read_binding` in the session manifest and save the exact command, exit status, stdout, and stderr for every invocation.

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
