# Recipe: add an opcode

An opcode has two halves. A client opcode (`CMSG_*`) reaches a Gateway dispatcher, which makes
Durable Reads and Durable Requests through a Store. A server opcode (`SMSG_*`) that answers a
change in the Module is sent by a Relay when a row changes. Many features need both.

Two merged changes are the worked examples. Read their diffs next to this list:

- The Meeting Stone Queue (`c2f2373a`) added a whole client-opcode family:
  `CMSG_MEETINGSTONE_JOIN`, `_LEAVE`, `_INFO` and `MSG_LOOKING_FOR_GROUP`.
- The Instance Removal countdown (`fcb1cf3b`) relays `SMSG_RAID_GROUP_ONLY` from a Module row.

```bash
git diff c2f2373a^1 c2f2373a --stat
git diff fcb1cf3b^1 fcb1cf3b --stat
```

Steps 1 to 4 are the Module side. Steps 5 to 11 are the Gateway side, and step 12 covers both.

## 1. Check the message type

Find the message in `wow_world_messages::vanilla`: `ClientOpcodeMessage::CMSG_*` for a client
opcode, `ServerOpcodeMessage::SMSG_*` for a server one. Compare its fields with cmangos or vmangos.
When gtker cannot encode the real layout, send raw bytes with `Outbound::Raw { opcode, body }`, as
the cast handler does for `SMSG_CAST_RESULT`.

## 2. Put the rule in the Module

The Gateway carries the client's intent. The Module decides.

- Add a Gateway Verb, `gw_<verb>`, in the owning family's module under `module/src/`. Its first act
  is `crate::helpers::require_operator(ctx)?`, then
  `crate::account_ownership::require_actor(ctx, request_actor)?` with a `crate::SessionActor`
  argument. Then it calls the family's core function, which holds the Gates.
- A Realm-core operation is a `realm_<verb>` reducer and takes the realm-wide facts the Gateway
  read.
- A Refusal returns `Err` with a tag the Gateway can parse back into a typed Refusal. The Meeting
  Stone Queue keeps that type in `lyracore_shared::meeting_stone`. A transport failure stays an
  error, so the two never mix.
- Declare a new module file in `module/src/lib.rs`.

## 3. Store what the Relay or a Durable Read needs

Add or extend a table in the owning family. A new column goes at the END of the struct with
`#[default(...)]`, and a `String` cannot take one. [`danger-zones.md`](../danger-zones.md) section 1
has the rules. A row the Gateway relays carries its recipient, so the Relay finds the viewer by key
instead of a scan.

## 4. Regenerate the Gateway bindings

Required for every new table and every new reducer.

- Run the `spacetime generate` command in [`danger-zones.md`](../danger-zones.md) section 1.2 and
  restore the hand-patched names and the comment it lists.
- Run `scripts/check-gateway-bindings.py`. It must report no difference.
- For each table the Gateway subscribes to, add its `SELECT * FROM <table>` to
  `coordinator_queries` in `gateway/src/stdb/connection.rs`, then add a `parity_test!` and the table
  name to `MANIFEST_TABLES` in `gateway/tests/schema_parity.rs`.

## 5. Call the reducer from the Coordinator

File: `gateway/src/stdb/store/<family>.rs`, declared in `gateway/src/stdb/store/mod.rs`. Add an
`impl Coordinator` method that makes the Durable Request as the `Actor` the handler resolved:

```rust
let coord = self.0.call_pipe();
call_reducer!(
    coord.conn.reducers,
    "gw_admit_meeting_stone",
    gw_admit_meeting_stone_then(self.session_actor(actor), go_guid)
)
```

A Realm-core call goes through `self.realm_core()?` first. Turn a parsed Refusal into a typed
outcome with `reducer_refusal_reason`, and return every other error unchanged.

## 6. Add the Durable Reads

File: `gateway/src/stdb/store/<family>.rs`. A read that a Relay or another family also calls goes
in `gateway/src/stdb/reads/`. Each read is an `impl Coordinator` method that finds rows in the Coordinator cache by a unique index. The SDK
cache has no other index, and a whole-table `iter()` holds the lock the pump needs.

## 7. Write the family dispatcher

File: `gateway/src/world/handlers/<family>.rs`, declared in `gateway/src/world/handlers/mod.rs` with
a `pub(crate) use` of its public items. For an opcode in an existing family, add an arm to that
family's dispatcher and skip to step 10.

- A Store trait, `<Family>ActionStore`. Each method's doc says whether it is a Durable Request or a
  Durable Read and on which Shard.
- `impl <Family>ActionStore for Coordinator` in `gateway/src/stdb/store/<family>.rs`, each method
  one call to step 5 or 6.
- A player struct with the session facts the family needs, and an outcome enum with
  `Handled { outbound }` and `PassThrough(msg)`.
- `dispatch_<family>_action(store, player, msg)` matches the family's `ClientOpcodeMessage` variants
  and passes every other message through.
- A local Fake of the Store and the family's tests in the same file.

## 8. Join the family to `WorldStore`

- File: `gateway/src/world/store.rs`. Add the trait to the `WorldStore` supertrait list and to the
  bounds of its blanket impl.
- File: `gateway/src/world/test_support/world_fake/<family>.rs`. Implement the trait for `WorldFake`,
  the shared Fake that implements every family, and keep the family's state in its own struct there.

## 9. Link the dispatcher into the chain

File: `gateway/src/world/mod.rs`. In `dispatch`, add a `match dispatch_<family>_action(...)` link
that sends `Handled` messages and returns, and passes `PassThrough` on. Re-export the family's items
in the `pub(crate) use handlers::{...}` list.

The order of the links matters. The first dispatcher that matches an opcode consumes it, and the
links before yours must not claim your opcodes. A message no link takes reaches the debug log
`world: ignoring`.

## 10. Build the packet

File: `gateway/src/codec/<area>.rs`, with a `pub use` in `gateway/src/codec/mod.rs` and any new gtker
type import there. Name the cmangos or vmangos source for each field. Test the builder byte for
byte, opcode and size header included.

## 11. Relay the row

File: `gateway/src/stdb/world_view.rs`.

- Register the callbacks with `wire_insert` and `wire_delete` in `register_shard_callbacks` for a
  World Shard table, or in `arm_realm_private` for a Realm-core one.
- In the callback, find the recipient's viewer by key, for example
  `view.viewer_of_owner_on_shard(shard, OwnerGuid(row.character_guid))`, and queue the packet with
  `enqueue(viewer, move |viewer| vec![...])`. The job runs on that session's writer, in order.
- If a client that enters the world must see a running state, send it from the world entry sweep as
  well. The Instance Removal countdown does this.

## 12. Test each seam

- The dispatcher, through its local Fake, in the family file.
- The packet bytes, in the codec file.
- The Relay's recipient choice and packet, in the `world_view.rs` test modules.
- The Module rule, in a durable test `module/tests/<family>.rs`. CI picks up a new target with no
  workflow edit.
- A cross-Shard Gateway behaviour, in an ignored test under `gateway/src/stdb/`. CI runs those one by
  one, so add its `--ignored --exact` line to `.github/workflows/module-durable.yml`.

Then record new terms in `CONTEXT.md` (and `CORE_TERMS.md` when a first change meets them), add a
client check under [`docs/verification/`](../verification/README.md) for what only a real client
shows, and run the commands in [`testing.md`](../testing.md).
