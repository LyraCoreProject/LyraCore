# Verification write-ups

Each file records one behaviour checked against a live stack or a real 1.12.1 build 5875 client:
the fixture, the exact procedure and the result. A file whose status says outstanding needs a person
at a real client.

| Document | What it checks |
| --- | --- |
| [`aura-capacity-verification.md`](./aura-capacity-verification.md) | The 32-buff/16-debuff cap end to end: refusal, untouched survivors, the overflow log line, and the wire-level `SMSG_SPELL_FAILURE` relay. |
| [`aura-stacking-probes.md`](./aura-stacking-probes.md) | The stacking-family decision on real `game_aura` rows, as an operator sees it. |
| [`cc-diminishing-returns-probe.md`](./cc-diminishing-returns-probe.md) | Crowd-control diminishing returns, whose persisted state and removal-time window only a live database shows. |
| [`vmap-rollout.md`](./vmap-rollout.md) | Exact collision on both populated World Shards, and that the Instance Pool receives no open-world vmap generation. |
| [`mount-verification.md`](./mount-verification.md) | The land-mount fixture ids, the attended procedure, and the Headless Client scenario. |
| [`taxi-flight-verification.md`](./taxi-flight-verification.md) | The direct-route flight baseline, and the cancel path when catalogue geometry mutates mid-flight. |
| [`movement-batch-acceptance.md`](./movement-batch-acceptance.md) | The steady-heartbeat batching path under a load driver, on a `disposable:` realm only. Script in `scripts/`. |
| [`auction-house-client-check.md`](./auction-house-client-check.md) | The auction house against a real 5875 client. Status: outstanding, needs a human. |
| [`guild-client-check.md`](./guild-client-check.md) | Guilds between two real 5875 clients across a Shard Boundary: founding, invites, chat, ranks, Transfer, emblem, Charter and deletion. Status: outstanding, needs a human. |
| [`mail-client-check.md`](./mail-client-check.md) | Mail against a real 5875 client. Status: outstanding, needs a human. |
| [`meeting-stone-client-check.md`](./meeting-stone-client-check.md) | Meeting stones between two real 5875 clients across a Shard Boundary: the JOIN packet, tooltip, minimap button, every status line, Transfer, logout and a stone-formed Party. Status: outstanding, needs a human. |
| [`self-resurrection-client-check.md`](./self-resurrection-client-check.md) | The Soulstone Self-Resurrection Option against a real 5875 client: the death dialog button, the revived vitals, the buff duration and the Release Spirit edge. Status: outstanding, needs a human. |
| [`distract-client-check.md`](./distract-client-check.md) | Distract against a real 5875 client: the ground reticle, the Creature turning and holding, patrols, and visible versus stealthed players. Status: outstanding. |
| [`duel-client-check.md`](./duel-client-check.md) | Duel visuals against a real 5875 client, which the automated tests cannot see. Status: outstanding. |
| [`chat-client-check.md`](./chat-client-check.md) | Realm-wide chat between two real 5875 clients across a Shard Boundary: party, channels and moderation, AFK and DND, whisper, friends, `/who`, ignore, say range, proximity emotes, the language Gate and the flood mute. Status: outstanding, needs a human. |
| [`raid-client-check.md`](./raid-client-check.md) | Raids, Group Broadcasts, raid chat, member stats across Shards and the Instance Removal countdown against real 5875 clients. Status: outstanding, needs a human. |
| [`hunter-pet-live-check.md`](./hunter-pet-live-check.md) | Taming, pet bars and pet lifecycle against a live development realm and a real client. |
