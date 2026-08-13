# 04 — Buyback view construction owned by the seam (incl. world-entry replay)

**What to build:** The complete buyback-window reconstruction (fabricated item objects plus
the descriptor update carrying inventory-slot guids, prices, and timestamps) becomes a seam
capability that RETURNS outbound packets instead of writing to the socket. World entry replays
the buyback ring through this same capability (still skipping entirely when the ring is
empty, so a ring-less login stays byte-identical). The wire-slot base (69) becomes one shared
constant used by both the render direction (base + i) and the parse direction (wire − base).

This closes a real coverage gap: give the shared socket-test store a settable buyback ring
(today it is hardwired empty) and add the first tests that pin the rendered ring content and
the login-time replay.

**Blocked by:** 01 — Vendor-action seam + tracer bullet.

**Status:** ready-for-agent

- [ ] The seam exposes buyback-view reconstruction returning packets; sell/buyback callers can
      reuse it (they migrate in later tickets — until then the legacy shared helper may
      delegate to it or remain, but only ONE implementation of the view math exists).
- [ ] World entry replays a non-empty ring identically to today's bytes; an empty ring emits
      nothing at login.
- [ ] Fabricated guids, slot numbering, price and timestamp descriptor indices are unchanged
      (pin with a content-level test).
- [ ] One shared wire-slot-base constant covers both directions of the mapping.
- [ ] The shared socket-test store's buyback ring is settable; all existing tests stay green.
