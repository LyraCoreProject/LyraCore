# Item GUID allocation

An item keeps its GUID while it moves between inventory slots and during Character Transfer.
Trading creates a new item GUID for the recipient. Mail, Auctions and Buyback retain an item
snapshot and create a new GUID when the recipient takes the item. They do not restore the
original item GUID.

New item GUIDs retain `HIGHGUID_ITEM = 0x4000` in bits 48 through 63. Bit 47 marks the new
allocation format. Bits 11 through 46 hold the complete Character GUID; bits 0 through 10
select one of 2,048 item identities. Character GUIDs above 68,719,476,735 receive
`ITEM_GUID_OWNER_OUT_OF_RANGE`, with no truncation. The current billion-sized Character ranges
fit through range 67; range 68 fits only partially. Expanding beyond that requires a reviewed
allocation change before assigning those ranges.

Allocation selects free identities in this Character's block and checks each candidate against
all local item rows. A batch is selected before any existing stacks change or Trading deletes
outgoing items. A full block returns `ITEM_GUID_EXHAUSTED`, leaving the request unchanged.
Deleted identities can be reused. The limit bounds simultaneously allocated items, not lifetime
grants. Normal inventory has fewer than 2,048 slots.

Reuse relies on existing ownership rules. Transfer preserves the Character and its items together
under Escrow. Only one Shard may accept inventory changes for that Character. Every operation
that changes an item's Character owner creates a new recipient GUID. A future ownership-changing
path that preserves item GUIDs must also replace this allocation policy.

## Compatibility and rollout

No table, column, reducer argument or Transfer row format changes. Existing item rows and saved
Transfer Escrow retain their exact item GUIDs. There is no automatic renumbering or data sweep.
Legacy items may already overlap another Character's namespace. Destination Transfer import
checks arriving item ownership, duplicate GUIDs and local collisions before inserting item rows.
A conflict returns a Refusal; the reducer transaction preserves the destination and the source
Escrow. It never deletes another Character's item or chooses a replacement identity.

The old allocator normally used only the low 32 bits, but its unchecked increment could escape
that range. Bit 47 alone cannot prove that an existing item was created by the new allocator.
Before deployment, an authorized Operator must inventory item GUIDs and saved Transfer Escrow
across every Shard. Record duplicate GUIDs with different owners, malformed item prefixes,
Character GUIDs above the supported limit and legacy GUIDs with bit 47 set. Resolve those cases
under separate human approval. The allocator's local collision check is not a realm-wide audit.

Pause World Sessions and all item-granting or ownership-changing activity during the update.
Drain or retain recorded Transfer Escrow, publish the reviewed Module to every Shard, and verify
that all Shards run the same build before resuming activity. An old Module would otherwise mint
from a new item's maximum GUID without respecting the new block boundary. Rolling back has the
same risk and requires the same pause and a reviewed plan for new-format rows.

Before shipping, obtain human review of this layout and rollout plan. On an authorized isolated
realm, verify existing and new items through grant, split, Trading, Mail, Buyback and round-trip
Transfer, including a conflicting legacy arrival. Check actual 5875 client item rendering and
slot moves. Unit tests and private reducer tests do not establish real-client acceptance or
prove a live realm's existing GUIDs are conflict-free.
