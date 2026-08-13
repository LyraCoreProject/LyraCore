# 07 — Gossip vendor path through the seam

**What to build:** Talking to a vendor NPC and choosing "Browse goods" opens the same vendor
window as a direct right-click, through the same seam implementation — the two entry paths can
no longer drift. The seam contributes the is-vendor decision (stock presence) that the gossip
flow uses to synthesize the Browse-goods option (imported gossip still cannot hide valid
stock), and the gossip select-option VENDOR arm calls the seam's inventory-open capability
instead of duplicating the stock-read + raw-response triple.

**Blocked by:** 01 — Vendor-action seam + tracer bullet.

**Status:** ready-for-agent

- [ ] A selected vendor gossip option opens byte-identical inventory to a direct vendor request,
      via the same code path.
- [ ] Vendor stock presence still contributes the synthesized Browse-goods option; an imported
      menu missing its vendor row still reaches the stock.
- [ ] The gossip flow learns no vendor storage or reducer details beyond the seam interface.
- [ ] Existing gossip-vendor socket tests stay green.
