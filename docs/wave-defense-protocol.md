# Wave defense addon protocol

The ordinary addon channel carries prefix `STC` and envelope
`v1|package.wave_defense.ACTION|SEQUENCE|1/1|PAYLOAD`. The Gateway authenticates the Character;
the payload never selects an Actor. Replies use the same command with `.result` appended.
Commands fit 64 bytes; payloads and replies fit 1024 bytes. Multipart requests are unsupported.

All fields below are pipe-separated. Text fields replace pipes and control characters with spaces.
GUIDs and request numbers are decimal strings, not Lua floating-point identifiers.

Read requests `status`, `offer`, and `roster` have empty payloads. The addon polls status every two
seconds and requests offer or roster only when needed. Mutation payloads begin with a monotonically
increasing request number saved by the addon. Status returns the last accepted number so a new
installation can resume above it. A repeated accepted number returns current state without applying
the action again. Older numbers are refused.

| Action | Payload after request number |
|---|---|
| `select` | class ID, identifying the one launch specialization for that class |
| `queue` | none; each human must consent, including premade members |
| `ready` | match ID, wave number |
| `choose` | match ID, offer revision, choice ID |
| `leave` | match ID, or zero while queued |

Successful mutations return status. Expected refusals return `Refused|reason` without changing state.
The addon displays the reason and refreshes state; it never assumes a grant from a button click.

Status fields are, in order:

`v1|phase|match_id|wave|level|xp|next_xp|base_hp|base_max|enemies|seconds|gold|pending|ready_count|is_ready|class|queue_seconds|last_request|offer_revision`

Phases are `lobby`, `queued`, `preparation`, `combat`, `won`, and `lost`. Match ID zero means no match.
Seconds is the preparation or respawn countdown where applicable. Class is the normal WoW class ID.

Offer fields begin `v1|match_id|revision|milestone|pending`. Each of up to three choices appends four
fields: `choice_id|kind|name|description`. Kind is `spell` or `talent`. An empty offer has only the
five prefix fields. Selection requires the exact match, revision and choice ID.

Roster fields begin `v1`. Each member appends five fields: `guid|name|role|bot|ready`. Role is `tank`,
`healer`, or `damage`; booleans are `0` or `1`. Bots are labeled in both roster and Character name.

The selected layout is A: a top objective dock, centered preparation rewards, and a separate lobby
queue window. A compact HUD remains during combat. Shops remain world NPCs. The addon must not
present remote purchases as shop interactions.
