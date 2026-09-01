# T4: Map Loot Tag Refusals to the vanilla loot error

Parent: issue #385. **Runs after T2. Parallel with T3. Blocks T5.**
Model: mid. Estimated size: ~170k tokens.

## Problem

The Gateway reads corpse rows before it asks the Module whether a player may open the corpse. Item
and money reducer Refusals are treated as no outbound response, which can leave an old loot window
open and gives no explanation. Vanilla has a `DIDNT_KILL` loot response for this rule.

## Delivery

Extend the existing loot-window store seam without moving ownership into the Gateway.

1. Add or generate the Gateway binding for T2's corpse-open reducer and expose it through the
   narrow loot-window store adapter.
2. Before reading creature corpse money or items for `CMSG_LOOT`, call the open reducer. A gameplay
   Refusal stops all reads and returns a loot error. GameObject opening does not call the corpse
   Gate.
3. Add the smallest codec helper for `SMSG_LOOT_RESPONSE` with `LootMethodError::DidntKill`. Prefer
   the existing typed `wow_world_messages` type where it fits the surrounding code.
4. Classify T2's stable Loot Tag Refusal separately from transport failures. Map that Refusal from
   open, take-item, and take-money to `DIDNT_KILL`. Keep established behavior for unrelated
   gameplay Refusals unless a focused test proves they are the same ownership denial.
5. On ownership Refusal, clear `OpenLootState` before returning the error packet. Do not remove loot
   rows, release another player's corpse, or end the session.
6. Keep transport disconnection fatal and propagated as `Err`.

The Module's actor and corpse guids are diagnostic data. Do not expose custom server text to the
client.

## Acceptance criteria

1. A corpse-open Refusal emits exactly one `SMSG_LOOT_RESPONSE` with `DIDNT_KILL`, performs no loot
   reads, and leaves no open window.
2. A take-item ownership Refusal emits `DIDNT_KILL`, does not remove an item, and closes the window.
3. A take-money ownership Refusal emits `DIDNT_KILL`, does not change money, and closes the window.
4. An authorized corpse opens and reads the same money and item rows as before.
5. GameObject and chest opening remain unchanged.
6. A transport failure still propagates and can terminate the session through the existing path.
7. The fake loot-window store can drive every outcome without a socket or real Coordinator.
8. No tag membership or eligibility decision is reimplemented in the Gateway.

## Tests

Extend the existing loot-window fake-store tests. Cover open, item, and money ownership Refusals;
closed state; no reads after refused open; transport failure; authorized corpse; and GameObject
non-regression. Assert the decoded error enum or exact typed packet, not a substring in raw bytes.

## File ownership

- `gateway/src/codec/loot.rs`
- `gateway/src/world/handlers/loot.rs`
- narrow loot-window store/reducer adapter files under `gateway/src/stdb/`
- generated or checked-in binding files for T2's new reducer
- focused Gateway loot-window tests

Do not edit subscriptions, entity projection, or Module files. T3 owns viewer flags.

## Definition of done

Touched Rust files are individually formatted. `cargo test -p lyracore-gateway` and
`cargo clippy -p lyracore-gateway` are clean. Push to the dedicated T4 branch and report the commit
for T5 to integrate.
