# Unsupported vmangos behavior

This list covers the ported Import Families. The globals import and generated census report the
same Compatibility Manifest finding. The complete vmangos Source Profile gate is due in S6c.

| Finding | Selected rows | Current limit | Next slice |
| --- | ---: | --- | --- |
| `globals.portal-condition` | 16 | `areatrigger_teleport.required_condition` is nonzero, but the Module's portal catalogue has no condition field. The current classic-db import also omits portal conditions. The globals import retains these rows and reports their trigger ids as unsupported. | S3g translates supported condition kinds. Gameplay support needs a later decision. |

Verified against vmangos commit `464179081673cfd240f7ffe7f0daf96bca0b5a70`, selected for patch 10,
and the current `build_areatrigger_teleport_sql` output columns. No source rows are included here.

Items findings count emitted offers, keyed by creature and item. Full-world has 424 conditioned
offers, 208 offers with item flags, and 951 offers with a restock interval. Alliance-single has
21, 36 and 276 respectively. The Module stores `max_count`, but no restock interval, source item
flags or offer condition. The items import retains the offers and reports these fields as
`items.vendor-condition`, `items.vendor-item-flags` and `items.vendor-restock`. The generated
items censuses include these counts. Loot conditions are audited with the remaining loot family
in S3c. The 16 portal conditions above remain unsupported.
