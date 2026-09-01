# T4: Fuel tests over generated Lua

Parent: issue #320. Runs after T2. Parallel with T3 and T5. Blocks T6.
Model: sonnet. Repo: engine (`module/`).

## Problem

`FUEL_BUDGET_PER_INVOCATION` is sized against `REPRESENTATIVE_SCRIPT`, a hand-written chunk in
`module/src/runtime_script.rs`'s test module that is *shaped like* transpiler output. Its comment
says so: *"a transpiler-shaped chunk"*. It is a guess at what TSTL emits, not what TSTL emits.

Real TSTL output differs in ways that cost fuel: inlined `__TS__` lualib helpers, class prototypes
and `__TS__New`, and callbacks that take a leading `____` self parameter. The budget has never been
measured against any of it.

The issue's acceptance criterion is *"fuel tests include representative generated code"*.

## Delivery

Commit T2's compiled output for the worked example as a fixture, and measure the real thing.

- Put the generated Lua under `module/tests/fixtures/` or beside the test as an `include_str!`
  target. Pick whichever matches how `module` already carries fixtures; check before choosing.
- Keep `REPRESENTATIVE_SCRIPT` and its test. It is the recorded baseline the budget was set from,
  and deleting it would erase the comparison this ticket exists to make.
- Add the generated fixture alongside it, asserting:
  - it runs to completion inside `FUEL_BUDGET_PER_INVOCATION`,
  - by a stated margin, the way
    `the_representative_script_fits_the_fuel_budget_with_room_to_spare` already scales its list
    fifty times,
  - and that it produces the Staged Effects the handler's TypeScript says it should. A fuel test
    that never checks the answer would pass on a chunk that does nothing.
- Record the measured fuel cost of the generated fixture in the test's doc comment, next to
  `REPRESENTATIVE_SCRIPT`'s recorded 2,054 over 30 steps. That number is the thing a future TSTL
  bump gets compared against.

If T0 landed on branch B, the fixture carries the parameter-prologue rewrite and its cost is part of
the measurement. Branch A's measured overhead for reference: 1,607 fuel to 1,734 on the
representative workload, about 8%.

### Keeping the fixture honest

A committed generated artifact goes stale. The gate is T3's: `packages check` compares the
`script.identity` sidecar and names the input that moved, and preflight check 5 folds it in. This
ticket does not build a second staleness mechanism.

What it does add is the link: state in the fixture's doc comment which manifest and handler it was
generated from and with which command, so an agent that changes the example knows to regenerate it.

## Files owned

- `module/src/runtime_script.rs`, test module only. Coordinate with T0, T2 and T5, which also touch
  it. Take the file after they land, or expect to rebase.
- `module/tests/fixtures/` (the generated Lua fixture)

## Out of scope

- Changing `FUEL_BUDGET_PER_INVOCATION`, `FUEL_PER_STEP`, `MAX_STEPS_PER_INVOCATION`, or
  `MAX_STAGED_EFFECTS_PER_INVOCATION`. If the generated fixture does not fit, that is a finding to
  report, not a number to raise. Say so and stop.
- Every other part of the Host contract.
- The `runtimescripts/` project and the build that produces the fixture.
- Diagnostics (T5).

## Acceptance tests

- Real TSTL output for the worked example runs to completion under the Host, inside the Fuel Budget,
  with stated headroom.
- Its Staged Effects match what the TypeScript source says they should be.
- The measured fuel cost is recorded in the source.
- `REPRESENTATIVE_SCRIPT` and its existing test still pass unchanged.
- The fixture's provenance, which source and which command, is stated where a reader will find it.
