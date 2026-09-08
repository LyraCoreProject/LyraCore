# Item GUID allocation

Characters and items consume the same durable high-water mark in a Shard's GUID Range.
Realm-core assigns disjoint billion-value ranges. `install_guid_range` refuses reassignment;
deleting a Character, deleting an item, Transfer and reinstalling the same range never rewind
the mark. A batch must fit below the range's exclusive end before the allocator advances.
The range now budgets Character creation and item creation together. Its base is the allocator
floor, leaving 999,999,999 issued identities per range.

New items retain `HIGHGUID_ITEM = 0x4000` in bits 48 through 63. Bit 47 marks this allocation
format. Bits 0 through 46 hold the issued value, so 140,737,488,355,327 is the largest usable
item allocation. Requests above that limit return `ITEM_GUID_EXHAUSTED` before advancing the
mark. Missing ranges return `NO_GUID_RANGE`; a batch crossing a range end returns
`GUID_RANGE_EXHAUSTED`. Items do not truncate or pack their Character owner's GUID.

Grants reserve complete batches before updating partial stacks or deleting Trading items.
Issued identities are never reused, including items consumed by a quest or craft earlier in
the same request. If a craft cannot grant all products, it restores the original carried
inventory with the original item GUIDs. Profession skill gains and automatic learning wait
until all direct product grants succeed. This restores carried inventory on a refused craft
without minting refund items from an exhausted range. Other effects retain their authored order.
This is not whole-cast rollback: a mixed recipe with triggered casts or other side effects needs
separate acceptance coverage. The committed conjuring fixture has one direct CreateItem effect;
it does not establish that every imported recipe has that shape.

An item keeps its GUID while moving between inventory slots and during Character Transfer.
Trading creates new recipient items. Mail, Auctions and Buyback store item snapshots and
create new items when the recipient takes them. Preserved identities now rely on disjoint
GUID Ranges and a durable issuer mark, rather than a packed Character namespace.

Transfer does not move or replace a Shard's allocator. An arriving Character raises the local
mark only when its GUID belongs to the destination's own range. Incoming items need no floor
update because the issuing Shard retains their high-water mark after they leave.

## Compatibility and rollout

No table, column, reducer argument or Transfer row format changes. Existing item rows and saved
Transfer Escrow keep their exact GUIDs. There is no automatic renumbering or data sweep.
Destination import checks arriving item ownership, duplicate GUIDs and local collisions before
inserting item rows. A conflict returns `ITEM_GUID_CONFLICT`; the reducer transaction preserves
the destination and source Escrow. It never deletes another Character's item to make room.

The old allocator normally used only the low 32 bits, but unchecked increments could escape
that range. No bit below the item prefix is provably unreachable by the old code. Before
deployment, an authorized Operator must inventory item GUIDs and saved Transfer Escrow across
every Shard. Record duplicate GUIDs with different owners, malformed item prefixes, legacy IDs
with bit 47 set, missing or overlapping GUID Ranges, and allocator marks at or beyond their
range end. Resolve those cases under separate human approval. Local collision checks do not
replace this realm-wide audit. An already escaped Character GUID also needs explicit review.

Without an installed range, Character cleanup no longer raises the allocator mark. Resolve a
missing range before this rollout; do not delete legacy Characters first and then seed a mark
from surviving rows. The update never invents a range assignment or resets a mark.

Pause World Sessions and all item-granting or ownership-changing activity during the update.
Record outstanding Transfer Escrow. Verify each Shard's permanent range assignment, publish the
reviewed Module to every Shard, and confirm that all Shards run the same build before resuming
activity. An old Module would mint from a new item's maximum GUID without respecting the shared
allocator. Rolling back requires the same pause and a reviewed plan for new-format rows.
Never reset `game_guid_allocator`, reinstall a different range or delete the range registry.

Before deployment, obtain human review of this allocation and rollout plan. On an authorized
isolated realm, verify existing and new items through grant, split, crafting, Trading, Mail,
Buyback and round-trip Transfer, including a conflicting legacy arrival. Check actual 5875
client item rendering and slot moves. Unit tests and private reducer tests do not establish
real-client acceptance or prove a live realm's existing GUIDs are conflict-free.
