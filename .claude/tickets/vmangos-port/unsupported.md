# Unsupported vmangos behavior

This list covers the ported Import Families. The globals import and generated census report the
same Compatibility Manifest finding. The complete vmangos Source Profile gate is due in S6c.

| Finding | Selected rows | Current limit | Next slice |
| --- | ---: | --- | --- |
| `globals.portal-condition` | 16 | `areatrigger_teleport.required_condition` is nonzero, but the Module's portal catalogue has no condition field. The current classic-db import also omits portal conditions. The globals import retains these rows and reports their trigger ids as unsupported. | S3g translates supported condition kinds. Gameplay support needs a later decision. |

Verified against vmangos commit `464179081673cfd240f7ffe7f0daf96bca0b5a70`, selected for patch 10,
and the current `build_areatrigger_teleport_sql` output columns. No source rows are included here.

Items findings count emitted offers, keyed by creature and item. Full-world has 403 runtime-conditioned
offers, 208 offers with item flags, and 951 offers with a restock interval. Alliance-single has
21, 36 and 276 respectively. The Module stores `max_count`, but no restock interval, source item
flags or offer condition. The items import retains the offers and reports these fields as
`items.vendor-condition`, `items.vendor-item-flags` and `items.vendor-restock`. The generated
items censuses include these counts. Loot conditions are audited with the remaining loot family
in S3c. The 16 portal conditions above remain unsupported.

The loot audit covers all nine selected source tables, before profile-specific owner filtering.
These counts are source rows after patch and static-condition selection.
The items manifest also reports findings from creature and reference loot. Quest mail validation
remains part of S3f.

| Finding | Occurrences | Current limit |
| --- | ---: | --- |
| `loot.condition` | 4085 | The loot tables have no runtime condition field. Static WOW_PATCH conditions are applied before expansion. |
| `loot.count-range` | 5960 | The current import stores the maximum count. |
| `loot.quest-only` | 35 | Fishing and skinning tables have no quest-only flag. |
| `loot.unconsumed-table` | 2741 | Item-container and disenchant source tables have no world import path. |

The generated loot censuses break these findings down by source table. Item-container loot has
2,660 selected rows and disenchant loot has 81. The 16 portal conditions remain unsupported.

The pre-existing small-chance clamp remains. A positive independent chance below one basis point
can emit one basis point. Fishing and skinning quest-only flags remain unsupported.
