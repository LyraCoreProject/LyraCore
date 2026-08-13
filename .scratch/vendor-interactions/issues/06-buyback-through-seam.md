# 06 — Buyback through the seam

**What to build:** A player buying back a sold item gets today's behavior through the seam.
The wire buyback slot maps to the same zero-based durable ring slot (using the shared
constant from ticket 04); a successful buyback rebuilds the full buyback view so shifted and
cleared ring entries appear immediately; a rejected buyback stays non-fatal with its current
client-visible behavior (silent). CMSG_BUYBACK_ITEM leaves the legacy handler — which this
ticket may now delete if it has no remaining arms after 02/03/05 land, or leave for the
contraction ticket.

**Blocked by:** 01 — Vendor-action seam + tracer bullet; 04 — Buyback view owned by the seam.

**Status:** ready-for-agent

- [ ] Wire slots map to ring slots exactly as before (Slot1 → 0 … Slot13 → 12).
- [ ] A successful buyback returns the full rebuilt buyback view.
- [ ] A rejected buyback is non-fatal and preserves today's client-visible behavior;
      transport loss is session-fatal.
- [ ] Seam-level unit tests cover the mapping, success, refusal, and fatal branches; the
      existing wire-slot socket test stays green.
