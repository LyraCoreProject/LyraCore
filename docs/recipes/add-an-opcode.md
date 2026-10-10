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
  Stone Queue keeps that type in `lyracore_shared::meeting_stone`. The Gateway's `classify` treats
  every other error as a Transport Loss, so the two never mix.
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

## 5. Call the reducer from the Store trait method

File: `gateway/src/stdb/store/<family>.rs`, declared in `gateway/src/stdb/store/mod.rs`. It holds
`impl <Family>ActionStore for Coordinator`. Each trait method makes its Durable Request itself, as
the `Actor` the handler resolved:

```rust
fn admit_meeting_stone(&self, actor: Actor, go_guid: u64) -> Result<MeetingStoneOutcome> {
    meeting_stone_outcome(call_reducer!(
        self.0.call_pipe().conn.reducers,
        "gw_admit_meeting_stone",
        gw_admit_meeting_stone_then(self.session_actor(actor), go_guid)
    ))
}
```

Do not add an inherent `impl Coordinator` method for the trait method to forward to. Keep one only
when a Relay, logon or another family calls it too. A Realm-core call goes through
`self.realm_core()?` first. A Durable Request the Gateway makes for the World Session, with no
client acting, uses `self.owner_actor()`. A family with a typed Refusal parses it with
`reducer_refusal_reason` and returns every other error unchanged.

## 6. Add the Durable Reads

File: `gateway/src/stdb/store/<family>.rs`. Write the read in the trait method. A read that a Relay
or another family also calls is an `impl Coordinator` method in `gateway/src/stdb/reads/`. Find rows
in the Coordinator cache by a unique index. The SDK cache has no other index, and a whole-table
`iter()` holds the lock the pump needs.

## 7. Write the family dispatcher

File: `gateway/src/world/handlers/<family>.rs`, declared in `gateway/src/world/handlers/mod.rs` with
a `pub(crate) use` of its public items. For an opcode in an existing family, add an arm to that
family's dispatcher and skip to step 10.

- A Store trait, `<Family>ActionStore`. Each method's doc says whether it is a Durable Request or a
  Durable Read and on which Shard. A method that acts for a Character takes `actor: Actor`. Steps 5
  and 6 implement it.
- The dispatcher resolves the session's Character to `Option<Actor>` once, with `Actor::new`. When
  it is `None`, make no Durable Request. The client usually gets no answer.
- When a Store call fails, match on `classify(&error)`. A `DurableFailure::Refusal { reason }` gets
  the family's answer to the client and the session continues. A `DurableFailure::TransportLoss`
  returns the `Err`, which ends the World Session.
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

## 9. Route the opcode to its family

File: `gateway/src/world/routing.rs`. Add the opcode to its family's arm in `owner()`. Each opcode
has one owner: a second owner is an unreachable pattern, and the build fails. A new family adds a
`Family` variant, an arm in `owner()`, and a representative opcode in the routing tests.

File: `gateway/src/world/mod.rs`. A new family adds one arm to the `match family` in `dispatch`. The
arm calls `dispatch_<family>_action(...)`, sends the `Handled` messages, and ignores a
`PassThrough`. Re-export the family's items in the `pub(crate) use handlers::{...}` list. A message
no family owns reaches the debug log `world: ignoring`.

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

- The dispatcher, through its local Fake, in the family file. One Fake answers
  `ReducerCallError::refused` and the client gets the Refusal answer. One answers
  `ReducerCallError::transport_lost` and the dispatcher returns `Err`.
- The packet bytes, in the codec file.
- The Relay's recipient choice and packet, in the `world_view.rs` test modules.
- The Module rule, in a durable test `module/tests/<family>.rs`. CI picks up a new target with no
  workflow edit.
- A cross-Shard Gateway behaviour, in an ignored test under `gateway/src/stdb/`. CI runs those one by
  one, so add its `--ignored --exact` line to `.github/workflows/module-durable.yml`.

Then record new terms in `CONTEXT.md` (and `CORE_TERMS.md` when a first change meets them), add a
client check under [`docs/verification/`](../verification/README.md) for what only a real client
shows, and run the commands in [`testing.md`](../testing.md).
