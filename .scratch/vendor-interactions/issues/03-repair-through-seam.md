# 03 — Repair through the seam

**What to build:** A player repairing gear at a vendor gets today's behavior through the seam.
Repairing one item resolves the client item-instance guid to the player's durable slot before
dispatch; item instance zero means repair-all (the whole-body durable operation); an unknown
instance guid is a harmless no-op; a rejected repair produces the same private system-chat
message so the player can diagnose it. CMSG_REPAIR_ITEM leaves the legacy handler; the seam
owns guid→slot resolution for repair (prefer the existing slot-lookup primitive over
re-scanning the full item list, if behavior is identical).

**Blocked by:** 01 — Vendor-action seam + tracer bullet.

**Status:** ready-for-agent

- [ ] Repair of one item dispatches the correct durable slot for the given instance guid.
- [ ] Instance guid zero dispatches the whole-body repair-all operation.
- [ ] An unknown instance guid performs no durable call and sends nothing.
- [ ] A rejected repair produces the same private system message as before; transport loss is
      session-fatal.
- [ ] Seam-level unit tests cover all four branches; existing repair socket tests stay green.
