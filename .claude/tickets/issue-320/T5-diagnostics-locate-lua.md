# T5: Script Diagnostics locate the generated Lua

Parent: issue #320. Runs in parallel with T0, T1, T3 and T4. Blocks T6.
Model: sonnet. Repo: engine (`module/`).

## Problem

The issue asks that *"runtime diagnostics identify the generated Lua location"*, with source maps
explicitly deferred. Once the Lua an author reads is machine-generated, "which script failed" stops
being enough: the author needs a line to look at.

Measured against piccolo 0.3.3, what the interpreter actually gives:

| Failure | Error text piccolo produces |
|---|---|
| runtime, `local d = c.field` where `c` is nil | `runtime error: type error, expected table, found nil` |
| runtime, three frames deep | `runtime error: type error, expected table, found nil` |
| `error('authored message')` | `lua error: authored message` |
| syntax, `local a = = 1` | `parse error at line 1: found "Assign", expected "grouped expression or name"` |

So:

- **Syntax failures carry a line.** They carry no chunk name.
- **Runtime failures carry neither a line nor a chunk name**, at any depth.

The chunk name is already supplied, by `Closure::load(ctx, Some(script.name), ...)` in
`RuntimeScriptHost::compiled`, and piccolo does not put it in the error string.

This bounds what v1 can honestly promise, and the bound is worth writing down rather than
discovering later.

## Delivery

Deliver what the interpreter permits, and record the limit where the next agent will read it.

1. **`ScriptDiagnostic` already names the Runtime Script and the event**, as host-supplied labels
   independent of piccolo. Verify with a test that a syntax failure's message reaches the author
   with piccolo's `at line N` intact and not truncated away by `DIAGNOSTIC_MESSAGE_CAP`. A syntax
   error in a large generated chunk is exactly the case where the useful part could be cut.
2. **Say which Lua the line refers to.** The generated Lua *is* what is shipped: it sits in
   `game_script.source`, so a line number in a diagnostic indexes a text an operator can read back.
   State that in `runtime_script.rs`'s module doc comment, in the paragraph about Script Diagnostics.
   That is the whole of "identify the generated Lua location" for v1, and it costs nothing.
3. **Record the runtime-failure gap.** A doc comment on `ScriptDiagnostic` stating that piccolo
   0.3.3 supplies no line and no chunk name for a runtime failure, that the Host's own labels are
   the only locator in that case, and that source maps cannot be added on top of a line number the
   interpreter does not produce. This is the fact that determines whether the deferred source-map
   work is even reachable.
4. If T0 landed on branch A and the fork is already carried, adding the chunk name and line to
   piccolo's runtime errors is a small further patch and worth doing here. Check T0's outcome first.
   If T0 landed on branch B, do not open a fork for this alone; report it.

## Files owned

- `module/src/runtime_script.rs`: the module doc comment, `ScriptDiagnostic`'s doc comment, and
  diagnostic tests. Coordinate with T0, T2 and T4, which also touch this file.

## Out of scope

- Source maps, and any presentation of a TypeScript location. Explicitly out of scope on the issue.
- Changing `DIAGNOSTIC_MESSAGE_CAP` or `DIAGNOSTIC_LABEL_CAP` without measuring first. If a real
  generated-chunk syntax error is truncated past usefulness, that is a measured finding and a
  proposed number, not a silent bump.
- The failure-containment behaviour: what discards Staged Effects, what stops the next script, what
  `run_event` collects. All of that is #317's and is correct.
- Any CLI or artifact change. An operator reading a script's source back is `spacetime sql` today; a
  CLI verb for it is not this issue.

## Acceptance tests

- A syntax failure in a chunk large enough to matter yields a Script Diagnostic that still carries
  piccolo's line number after capping.
- A runtime failure yields a Script Diagnostic naming the Runtime Script and the event.
- The module doc comment states, in one place, what locates a failure and what does not.
- Existing diagnostic tests pass unchanged:
  `a_diagnostic_names_the_script_the_event_and_the_failure_kind`,
  `a_diagnostic_message_is_capped_however_much_the_script_raises`,
  `a_truncated_diagnostic_message_stays_valid_utf8`,
  `diagnostic_labels_are_bounded_and_stay_valid_utf8`.
