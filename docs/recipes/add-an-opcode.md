# Recipe: add an opcode

Client requests enter `module/src/native/mod.rs` and dispatch from `module/src/native/world.rs`.
The Module reads durable state, applies gameplay rules and emits ordered packets in one
transaction. The Native Host handles sockets and cryptographic scratch state.

## 1. Establish the build-5875 layout

Find the message in `wow_world_messages::vanilla` and compare it with the client or an authoritative
protocol implementation. Some generated layouts differ from 1.12.1. Put bounded custom readers and
packet builders in `crates/lyracore-protocol/src/codec/`.

Test the bytes, lengths and opcodes. Gate counts, booleans, sized strings and compressed data before
calling a generated reader that can panic or allocate from an unchecked field. A reader reachable
at Character selection or in the Login Queue needs the same protection as an in-world reader.

## 2. Put the gameplay rule in its owning Module code

Resolve the Actor from the authenticated World Session. Call the existing gameplay operation or
add it in the owning Module file. Keep presence, ownership, map, Instance and recipient Gates there.
Use indexes and partition-scoped spatial helpers. A request must not supply its own authority.

Preserve atomicity. An expected Refusal must leave durable state unchanged unless the behavior
explicitly defines a partial result. Use the native rollback/reply helpers when a reply must be
sent after a transaction rolls back. Inspect neighboring handlers for the required reply shape.
A transport or decoding failure remains distinct from a gameplay Refusal.

## 3. Add state only when the behavior needs it

Add or extend a table in the owning Module file. Follow the additive migration rules in
[danger zones](../danger-zones.md). Native request handling reads the Shard directly; it needs no
Coordinator cache, generated Gateway bindings or cross-Shard call.

If login or map entry must restore the state, update the entry path too. Persistent state and
transient notifications have different replay requirements.

## 4. Dispatch and deliver

Add the request arm in `module/src/native/world.rs` and its Protocol Family handler under
`module/src/native/`. Preserve the opcode's behavior in every state where the client sends it,
including packets still in flight after logout and acknowledgements during teleport.

Build replies with the protocol crate and emit them through the native send functions. State
changes use the owning native Relay. Select recipients in the Module using World Sessions and AOI
Grants. Keep owner-private fields out of Peer CREATE and AOI VALUES packets.

A map change clears the old audience. Connection notifications can still arrive while loading;
AOI entry must put self CREATE before the destination's dependent updates. Test the ordering when
the new opcode participates in that path.

## 5. Verify each affected boundary

- Protocol tests cover exact layouts and malformed client bytes.
- Module tests cover gameplay Gates, atomicity and committed state.
- `native-host/check.py` covers actual native request dispatch, replies and peer delivery.
- Benilla covers the client acknowledgement, UI and follow-up behavior that a Headless Client
  cannot establish.

Run `./lyracore preflight` after a schema change. A change to native reducer arguments or the Host's
BSATN delivery types must update both Host and Module and pass the native protocol check. See
[testing](../testing.md) for the remaining required checks.

Record new terms in `CONTEXT.md`, and in `CORE_TERMS.md` when new contributors need them. Record
real-client evidence under [verification](../verification/README.md), including the exact revision
and any fixture setup.
