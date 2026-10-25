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
old SpacetimeDB data and do not come across. Plan to import content and create Accounts again.

1. Pull `main`. `./lyracore` installs the pinned CLI on its next run.
2. Stop the old runtime with `./lyracore dev down`. Startup refuses an unrelated process on port 3000,
   3724 or 8085.
3. Install the Native Host toolchain with `rustup toolchain install 1.97.1`. The Module still builds
   with 1.93.0. Native startup also needs OpenSSL 3 or newer.
4. Run `./lyracore dev up`. The first run also builds the Native Host.
5. Run `./lyracore import`, then `./lyracore account create TEST`.

`./lyracore dev smoke` checks login and world entry before you start a client.

Some files change:

- `.lyracore/native-data/` holds the Realm data. Keep it across restarts.
- `.lyracore/native-owner-token` and `.lyracore/native-cli.toml` replace `.lyracore/coordinator-token`.
- `.lyracore/logs/gateway.log` is no longer written. All output goes to `.lyracore/logs/spacetime.log`.
- `dev up` publishes one Shard, `lyracore`. It does not publish `lyracore-realm`,
  `lyracore-kalimdor` or `lyracore-instances`.

For Module code changes, use `./lyracore dev watch` instead of `dev up`. It hot-reloads each saved
change into the running Host and keeps your client connected. See
[Development reload](./quickstart.md#development-reload).

## Branches that change the Gateway

The change deletes `gateway/`. A rebase gives a delete/modify conflict for each Gateway file you
changed. Git cannot resolve these conflicts, because the new code is in different files and has a
different shape. Port the change by hand. Do not restore the deleted files.

### Where the code went

| Old location | New location |
| --- | --- |
| `gateway/src/codec/*` | `crates/lyracore-protocol/src/codec/*`, mostly unchanged |
| `gateway/src/world/who.rs`, `packet_lint.rs` | `crates/lyracore-protocol/src/` |
| `gateway/src/world/handlers/<area>.rs` | `module/src/native/<area>.rs` |
| Opcode routing in `gateway/src/world/mod.rs` | `module/src/native/world.rs`, `receive` |
| `gateway/src/logon/*`, Session and login queue code | `module/src/native/logon.rs`, `admission.rs`, `mod.rs` |
| `gateway/src/stdb/store/*`, subscription caches | Gone. Read tables through `ctx.db` |
| `gateway/src/codec/*/subscriptions/*`, Relay fanout | The Module function that writes the Relay row calls `crate::native::<relay>` |
| `gateway/src/stdb/aoi.rs`, `world_view/*` | `module/src/aoi.rs` and `module/src/native/visibility.rs` |
| Shard routing, Transfer, `realm_core.rs` | Gone. One Shard holds the Realm |
| `gateway/src/world/*_tests.rs`, `test_support/world_fake` | Module unit tests, `module/tests/*` and `native-host/check.py` |

Some areas have new file names. Vendor and bank requests are in `economy.rs`. Melee is in
`combat.rs`. Casts are in `cast.rs`. Use
`grep -n CMSG_YOUR_OPCODE module/src/native/world.rs` to find the handler for an opcode.

### Patterns

**Client request.** The Gateway decoded a packet, checked it and called a `gw_*` reducer through
`call_reducer!`. Now `native/world.rs` matches the `ClientOpcodeMessage` and calls the handler for
its area. The handler runs inside the transaction. It applies the Gate, calls the core function
directly and sends replies with `super::send(ctx, connection_id, message, false)`.
`module/src/native/death.rs` shows the whole pattern for `CMSG_SPIRIT_HEALER_ACTIVATE`.

**Refusal.** Return an error from the handler. `super::rollback_silent(cause)` rolls back and
sends nothing. `super::rollback_reply(message, cause)` rolls back and sends one packet. The Native
Host delivers that packet after the rollback. Do not send a refusal packet with `send` and then
return an error, because the rollback deletes it.

**Relay.** The Gateway subscribed to a Relay table and encoded each new row. Now the Module code that
inserts the row also calls the native Relay function. `module/src/breath_relay.rs` inserts the row,
then calls `crate::native::breath_event`. That function selects the recipient and builds the packet
with the protocol crate. For one Character, look up its connection in `game_native_player` and use
`send_raw`. For nearby observers, use `broadcast_aoi` in `native/visibility.rs`.

**Gateway Verb.** The `gw_*` reducers in `module/src/gw.rs` still exist. Durable tests and
`native-host/check.py` call them as the Operator. The native runtime does not call them. If your
branch adds a `gw_*` reducer only so the Gateway can call it, put that logic in the native handler
instead. A change to a `gw_*` argument list is a schema change. `dev watch` refuses it.

**Packet codec.** Codec changes move across with small edits. The protocol crate builds for WASM,
so it cannot use threads, sockets or the system clock.

### Port a branch

1. List your Gateway changes with `git diff --stat $(git merge-base HEAD origin/main) -- gateway/`.
2. Rebase onto `main`. Resolve conflicts outside `gateway/` as usual.
3. For each Gateway change, find the new location in the table, then make the same change there.
4. Remove the deleted Gateway files from the rebase with `git rm`.
5. Move Gateway tests to the matching Module tests. If a test checks wire bytes or packet order,
   add a case to `native-host/check.py`.
6. Run the checks in [testing.md](./testing.md), including the native protocol check.

The runtime change already includes some fixes. Before you port a change, read the new handler.
For example, `native/death.rs` already asks a Spirit Healer to confirm before it resurrects a ghost.

## Package changes

Package function signatures do not change. Some behaviour does change. The Package API lint does
not report these changes. [package-api.md](./package-api.md) gives the full contract.

- `emit_bot_invite_intent` and `emit_bot_leave_intent` change the group in the same transaction.
  They create no Group Intent. A group invariant failure aborts the transaction.
- `set_sessionless_action_consent` also clears the Character's last completed Character Relocation.
  Absent consent permits sessionless actions.
- `enter_sessionless_areatrigger` moves the Character in the same transaction and returns the id of
  the completed Character Relocation. It creates no Transfer Intent or Escrow.

If your Package waits for a Group Intent or Transfer Intent to complete, remove that wait. The
action is complete when the call returns.

## Multi-Shard Realms

Do not update a Realm that has players. This change has no migration for durable state from
several Shards, pending Escrows and Holds, Gateway leases or Account Claims. Expiry of old leases and
Account Claims can remove Characters or channel memberships. A separate reviewed migration must come
first. Until then, keep the Realm on the last `main` commit before the Native Host.
