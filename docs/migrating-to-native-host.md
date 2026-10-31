# Migrating to the Native Host

The Gateway is gone. The Native Host accepts WoW connections, and the Module handles every request,
Relay and Refusal. A Realm runs in one Shard. [The architecture](./architecture.md#native-host-migration)
describes the new runtime. This page tells you what to change in your checkout, branch or Package.

Agents can do most of this work. Run `/migrate-native-host` in Claude Code from your checkout.

| You have | Read |
| --- | --- |
| A local development Realm | [Local Realm](#local-realm) |
| A branch or fork that changes `gateway/` | [Branches that change the Gateway](#branches-that-change-the-gateway) |
| A Package | [Package changes](#package-changes) |
| A multi-Shard Realm with players | [Multi-Shard Realms](#multi-shard-realms) |

## Local Realm

The new runtime does not read data from the old one. Your local Accounts and Characters stay in the
old SpacetimeDB data and do not come across. Plan to import content and create Accounts and
Characters again.

1. Pull `main`. `./lyracore` installs the pinned CLI on its next run.
2. Stop the old runtime with `./lyracore dev down`. Native startup refuses while a recorded legacy
   runtime is live, and it refuses an unrelated process on port 3000, 3724 or 8085.
3. Install the toolchain with `rustup toolchain install 1.97.1`. The Module and the Native Host both
   build with it. Native startup also needs OpenSSL 3 or newer.
4. Run `./lyracore dev up`. The first run also builds the Native Host.
5. Run `./lyracore import`, then `./lyracore account create TEST`.

To check login and world entry before you start a client, run the smoke test. It needs imported
content and an Account. It creates the named Character if it does not exist. Give it the password
you gave `account create`:

```bash
printf 'PASSWORD\n' | ./lyracore dev smoke TEST Tester --password-stdin
```

Some files change:

- `.lyracore/native-data/` holds the Realm data. Keep it across restarts.
- `.lyracore/native-owner-token` and `.lyracore/native-cli.toml` replace `.lyracore/coordinator-token`.
- `.lyracore/native-jwt/` holds the local signing keys. `.lyracore/native-host/` holds the Host
  source and build.
- `.lyracore/logs/gateway.log` is no longer written. All output goes to `.lyracore/logs/spacetime.log`.
- `dev up` publishes one Shard, `lyracore`. It does not publish `lyracore-realm`,
  `lyracore-kalimdor` or `lyracore-instances`.

For Module code changes, use `./lyracore dev watch`. It hot-reloads each saved change into the
running Host and keeps your client connected. If a Host started by `dev up` is running, stop it
with `./lyracore dev down` first. See [Development reload](./quickstart.md#development-reload).

## Branches that change the Gateway

The change deletes most of `gateway/` and moves 38 files, mainly the codecs, to new paths. By
default, a rebase follows those moves. It applies some of your changes to the new files, and it
can move files your branch added into `module/src/native/` or the protocol crate, where they do
not build. [Port a branch](#port-a-branch) turns off rename detection, so each Gateway file you
changed gives a delete/modify conflict. Then port each change by hand. Do not restore the deleted
files.

### Where the code went

| Old location | New location |
| --- | --- |
| `gateway/src/codec/*` | `crates/lyracore-protocol/src/codec/*`, mostly unchanged |
| `gateway/src/world/who.rs`, `packet_lint.rs` | `crates/lyracore-protocol/src/` |
| `gateway/examples/*` | `crates/lyracore-protocol/examples/*` |
| `gateway/src/world/entry.rs` | `login` and `enter` in `module/src/native/world.rs` |
| `gateway/src/world/handlers/<area>.rs` | `module/src/native/<area>.rs`, with the renames below |
| `gateway/src/world/{party,mail,loot,social,chat_flood}.rs` | `module/src/native/` file of the same name |
| `gateway/src/world/party_mirror.rs` | `module/src/native/party.rs` and `module/src/group/mirror.rs` |
| `gateway/src/world/{whisper,presence}.rs` | `module/src/native/chat.rs` and `social.rs` |
| Opcode routing in `gateway/src/world/routing.rs`, `family.rs` | `receive` in `module/src/native/world.rs` |
| `gateway/src/logon/*`, World Session and Login Queue code | `module/src/native/logon.rs`, `admission.rs`, `mod.rs` |
| Movement handler, `coalesce.rs`, `stdb/movement_batch.rs` | `native/world.rs` calls `crate::world::apply_movement_update`; batching is in `module/src/motion.rs` |
| `gateway/src/stdb/{store,reads}/*`, `relays.rs`, `subscriptions.rs` | Gone. Read tables through `ctx.db` |
| `gateway/src/codec/*/subscriptions.rs`, Relay fanout | The Module code that writes the Relay row calls `crate::native::<relay>` |
| `gateway/src/stdb/aoi.rs`, `world_view/*` | `module/src/aoi.rs` and `module/src/native/visibility.rs` |
| `gateway/src/{main,accept,config,read_deadline,fd_limit}.rs`, `gateway/src/world/{transport,egress}.rs` | The Host extension in `native-host/src/` |
| Other `gateway/src/stdb/*` read and call wrappers | Gone. Read tables through `ctx.db` and call core functions directly |
| `load_sample.rs`, `stage_timing.rs`, `movement_batch_metrics.rs`, `provision_cli.rs` | Gone |
| Gateway Shard routing and Transfer coordination | Gone. The Module's `realm_core.rs` and `transfer/` remain for legacy tables |
| `gateway/src/world/*_tests.rs`, `test_support/world_fake` | Module unit tests, `module/tests/*` and `native-host/check.py` |
| `gateway/tests/*` | The `admission-wait` and `recovery` checks in `native-host/check.py`, and durable tests |

These handlers have new names. `char.rs` is `character.rs`, `gameobject.rs` is `object.rs`, `item.rs` is
`inventory.rs` and `weather.rs` is `environment.rs`. Vendor and bank requests are in `economy.rs`.
Melee is in `combat.rs`. Casts are in `cast.rs`.

To find the handler for an opcode, read the start of `receive` in `native/world.rs` first. It routes
movement, teleport acknowledgements, auction browsing, reputation and addon chat on the raw opcode
number, before it decodes the packet. Search there for the hex opcode or its `*_OPCODE` constant.
During a teleport, `receive` drops every opcode except `0x01dc`, `0x004a` and `0x004b`. It routes the other requests
by `ClientOpcodeMessage` variant, so `grep -n CMSG_YOUR_OPCODE module/src/native/world.rs` finds them.

### Patterns

**Client request.** The Gateway decoded a packet, checked it and called a `gw_*` reducer through
`call_reducer!`. Now `receive` in `native/world.rs` routes the request to the handler for its area.
The handler runs inside the transaction. It applies the Gate, calls the core function directly and
sends replies with `super::send(ctx, connection_id, message, false)`. In `module/src/native/death.rs`,
`CMSG_SPIRIT_HEALER_ACTIVATE` applies a Gate and calls the core function. `confirm_spirit_healer`
in the same file sends a reply. `native/gossip.rs` calls it.

**Refusal.** A request handler returns `Result<(), Rollback>`. `Rollback::Refusal` rolls back the
transaction, and the Native Host then sends its reply packets. The list of packets can be empty.
`Rollback::Failure` ends the World Session with a Transport Loss. A plain `String` error converts to
`Failure`. A core operation that refuses returns a typed Refusal: the family's enum when the
client reply depends on the reason, or `crate::Refusal` when it does not. The `?` operator
converts `crate::Refusal` to a silent `Rollback::Refusal`. The handler maps a family enum
to its reply packets with an exhaustive `match`, as `native/social.rs` does. Only
`native_world_receive` writes the `native_reply:` envelope.

Do not send a packet with `send` and then return `Err`, because the rollback deletes the packet.

**Relay.** The Gateway subscribed to a Relay table and encoded each new row. Now the Module code that
inserts the row also calls the native Relay function. `module/src/breath_relay.rs` inserts the row,
then calls `crate::native::breath_event`. Put a new Relay function in a file under
`module/src/native/` and re-export it with `pub(crate) use` in `native/mod.rs`, as `breath_event`
is. The function selects the recipient and builds the packet with the protocol crate. For one
Character, look up its connection in `game_native_player` and use `send_raw`. For nearby observers,
use `visibility::broadcast`.

**Gateway Verb.** The `gw_*` reducers in `module/src/gw.rs` still exist. Durable tests and
`native-host/check.py` call them as the Operator. The native runtime does not call them. If your
branch adds a `gw_*` reducer only so the Gateway can call it, put that logic in the native handler
instead. A change to a `gw_*` argument list is a schema change. `dev watch` refuses it.

**Packet codec.** Codec changes move across with small edits. The protocol crate builds for WASM,
so it cannot use threads, sockets or the system clock.

### Port a branch

Commit or stash your work first. The rebase removes the Gateway changes from your commits, so save
them before you start.

1. Make a backup branch: `git branch backup/my-branch HEAD`.
2. Save the Gateway diff outside the repository:
   `git diff $(git merge-base HEAD origin/main) HEAD -- gateway/ > /tmp/my-branch-gateway.diff`.
3. Run `git rebase -X no-renames origin/main`. At each stop, run
   `git rm -r -f -q --ignore-unmatch gateway/`, resolve the other conflicts as usual, then run
   `git rebase --continue`. Git drops a commit that changed
   only `gateway/`. If it stops on an empty commit, run `git rebase --skip`.
4. After the rebase, `git ls-files gateway/` must print nothing. Read `git diff --stat origin/main HEAD`
   and check that it lists only your own changes.
5. For each hunk in the saved diff, find the new location in the table. Read the new code, then make
   the same change in a new commit.
6. Move Gateway tests to the matching Module tests. If a test checks wire bytes or packet order,
   add a check to `native-host/check.py` as [testing.md](./testing.md#native-protocol-tier) describes.
7. Run the checks in [testing.md](./testing.md), including the native protocol check.

The runtime change already includes some fixes. Before you port a change, read the new handler.
For example, the native Module already asks a Spirit Healer to confirm before it resurrects a ghost.

If you already merged the new `main` into your branch, the merge dropped your Gateway changes, and
step 2 saves an empty diff. Save the changes from your own commits instead:
`git log --full-history --no-merges -p origin/main..HEAD -- gateway/ > /tmp/my-branch-gateway.diff`.
The rebase in step 3 replays those commits, so the steps still apply.

## Package changes

Package function signatures do not change. Some behaviour does change. The Package API lint does
not report these changes. [package-api.md](./package-api.md) gives the full contract.

- `emit_bot_invite_intent` and `emit_bot_leave_intent` change the group in the same transaction.
  They create no Group Intent. A group invariant failure aborts the transaction.
- Core checks Sessionless Action Consent before invite, leave and automatic acceptance. Suppressed
  consent leaves an incoming invitation unanswered. Absent consent permits sessionless actions.
- `set_sessionless_action_consent` also clears the Character's last completed Character Relocation.
- `enter_sessionless_areatrigger` moves the Character in the same transaction. Its `u64` result was
  a Transfer Intent id and is now the id of the completed Character Relocation. A Package that looks
  up a Transfer Intent with that id still compiles and finds nothing. A repeat call with the same
  AreaTrigger, destination and controller generation returns the same id while the Character stays
  in that partition. Only the latest relocation remains.
- `party_facts` reads the durable party roster. Live Units supply the current map and Instance. For
  an absent Unit, the partition can come only from legacy coordination records.

If your Package waits for a Group Intent or Transfer Intent to complete, remove that wait. The
action is complete when the call returns.

## Multi-Shard Realms

Do not update a Realm that has players. This change has no migration for durable state from
several Shards, pending Escrows and Holds, Gateway leases or Account Claims. When old leases and
Account Claims expire, the Module despawns their Characters and can remove channel memberships. A
separate reviewed migration must come first. Until then, keep the Realm on commit `8cf70a66`, the
last `main` commit before the Native Host.
