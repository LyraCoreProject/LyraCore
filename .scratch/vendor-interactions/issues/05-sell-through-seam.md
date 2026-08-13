# 05 — Selling through the seam

**What to build:** A player selling an item gets today's behavior through the seam. The client
item-instance guid resolves to the player's durable slot before the slot-based durable sell
operation; an unknown instance guid is harmless (no durable call, nothing sent); a successful
sale rebuilds the full buyback view (via the seam's view capability from ticket 04) so the
player sees the new ring immediately; a rejected sale stays a silent, non-fatal action failure.
CMSG_SELL_ITEM leaves the legacy handler.

**Blocked by:** 01 — Vendor-action seam + tracer bullet; 04 — Buyback view owned by the seam.

**Status:** ready-for-agent

- [ ] A sell dispatches the correct durable slot for the given instance guid.
- [ ] An unknown instance guid performs no durable call and sends nothing.
- [ ] A successful sale returns the full rebuilt buyback view.
- [ ] A rejected sale is non-fatal and client-silent (debug log only), exactly as today;
      transport loss is session-fatal.
- [ ] Seam-level unit tests cover all branches; existing sell socket tests stay green.
