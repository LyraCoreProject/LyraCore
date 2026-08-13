# 02 — Buying through the seam

**What to build:** A player buying an item from a vendor gets exactly today's behavior —
success delivers the item and purse change via existing subscriptions (no duplicated
delivery), rejection shows the same SMSG_BUY_FAILED reason — but the whole sequence
(mapping vendor, item entry, amount, account, and player to one durable buy operation, plus
client error mapping) is owned by the vendor-action seam. CMSG_BUY_ITEM leaves the legacy
handler; the seam trait gains the buy operation; refusal vs fatal classification goes through
the seam's classifier.

**Blocked by:** 01 — Vendor-action seam + tracer bullet.

**Status:** ready-for-agent

- [ ] A rejected purchase produces the same SMSG_BUY_FAILED client message as before.
- [ ] A successful purchase calls the durable buy operation with the same facts as before and
      emits no extra packets (subscriptions deliver the row changes).
- [ ] Transport loss during buy is session-fatal; gameplay refusal is not.
- [ ] Seam-level unit tests cover success, refusal mapping, and fatal classification.
- [ ] Existing buy socket test stays green.
