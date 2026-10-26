# Unsupported vmangos behavior

This list starts with the Milestone 1 globals census. It does not claim to cover unported Import
Families. The generated census records these findings; the vmangos Compatibility Manifest and
Source Profile gate are due in S6c.

| Finding | Selected rows | Current limit | Next slice |
| --- | ---: | --- | --- |
| `globals.portal-condition` | 16 | `areatrigger_teleport.required_condition` is nonzero, but the Module's portal catalogue has no condition field. The current classic-db import also omits portal conditions. The globals census counts the rows at current parity. | S3a reports the omission; S3g translates supported condition kinds. Gameplay support needs a later decision. |

Verified against vmangos commit `464179081673cfd240f7ffe7f0daf96bca0b5a70`, selected for patch 10,
and the current `build_areatrigger_teleport_sql` output columns. No source rows are included here.
