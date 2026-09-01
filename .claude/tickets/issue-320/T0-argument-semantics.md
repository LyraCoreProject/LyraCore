# T0: Restore Lua argument semantics in the Runtime Script Host

Parent: issue #320. Blocks T2. Runs in parallel with T1 and T5.
Model: opus. Repo: engine (`module/`, and possibly a patched piccolo).

## Maintainer decision required before this starts

**Recommendation: fix it once in the Host, by patching piccolo.** The alternative is to make every
producer of Lua work around it forever, which taxes hand-written Lua too and cannot be enforced.

## Problem

The defect recorded on issue #320 is real but describes a symptom, not the fault. Recorded:

> piccolo 0.3.3 miscompiles `f({...})` — it passes the table constructor's element count as an
> extra argument.

Measured against piccolo 0.3.3 directly, the fault is broader:

**A missing argument does not read `nil`. It reads whatever scratch value the caller last left in
that register.**

There need be no table constructor at the call site at all:

```lua
local function g(a, b) b = b or 'DEF' return tostring(b) end
local t = {7, 8, 9}          -- any earlier scratch work, anywhere in the chunk
OUT = g(1)                   -- => "8", not "DEF"
```

Measured behaviour, all on piccolo 0.3.3, `Lua::core()`:

| Source | Result | Correct Lua |
|---|---|---|
| `local function f(a,b,c) ... end` `f(1)` with no earlier scratch work | `nil/nil` | `nil/nil` |
| the same after `local t = {7,8,9}` | `8/9` | `nil/nil` |
| the same after `local s = 'a'..'b'..'c'` | `c` | `nil` |
| `f({1,2,3})` into `f(a,b)` | `b = 3` | `b = nil` |
| `f({1,2,3}, nil, nil)` into `f(a,b,c)` | `nil/nil` | `nil/nil` |
| `local function h(a, ...)` `h({7,8,9})` reading `select('#', ...)` | `0` | `0` |

The last row is the diagnosis: the **argument count is correct**, and varargs are correct. Only the
callee's declared parameter registers beyond the passed count are left holding stale values instead
of being cleared.

Two consequences the recorded note does not cover:

- `x = x or default`, the commonest idiom in Lua and the shape TSTL emits for every optional
  parameter, is broken with no table constructor anywhere near it.
- This is **already live**. #318 shipped the hand-written Lua path, so a Package shipping Lua today
  is exposed. This is not only a #320 blocker.

The existing pinning test
`piccolo_leaks_a_table_constructors_element_count_as_an_extra_argument` in
`module/src/runtime_script.rs` observed a narrow slice of this and named it after the slice.

## Delivery, branch A (recommended): fix the interpreter

piccolo 0.3.3 is the newest published version (crates.io index holds 0.1.0 through 0.3.3, none
yanked), so there is no upgrade to take. Clear the callee's unfilled parameter registers in
piccolo's call setup, carry it as a `[patch.crates-io]` fork pinned by revision, and offer it
upstream.

Then:

- Rewrite the pinning test as a regression test asserting correct Lua semantics, covering every row
  of the table above, and rename it for what it tests rather than for the defect.
- Drop the `type(separator) ~= "string"` guard from `PICCOLO_SHIM` and its comment. The guard exists
  only for this defect.
- Keep the `table.concat` shim itself. That is a genuine gap in piccolo's stdlib subset, unrelated.

The Host gains correct argument semantics for every producer at once: generated Lua, hand-written
Lua, and the shim. No per-call cost. Nothing downstream needs to know.

## Delivery, branch B (fallback): make every producer pad

Verified to work, in every shape tested. Rewrite each function declaration to take varargs and
destructure:

```lua
local function f(...) local a, b, c = ...  ... end
```

| Shape | Unrewritten | Rewritten |
|---|---|---|
| `f(1)` after `local t = {7,8,9}` | `8/9` | `nil/nil` |
| `g(1)` with `b = b or 'DEF'` | `8` | `DEF` |
| `i:hook({1,2,4})` into `(self, values, extra)` | `extra = 3` | `extra = nil` |
| a callback invoked with fewer args than declared | residue | `nil` |
| `f(1,2,3)` with every argument supplied | `2/3` | `2/3` |

Cost, measured on the representative workload: **1607 fuel to 1734, about 8%**. Affordable against a
200,000 Fuel Budget.

If the maintainer picks branch B, this ticket delivers only the pinning tests and the rule as
documentation. Applying the rewrite becomes T2's job, as a TSTL Lua-AST plugin, and hand-written Lua
must carry the same discipline with a build-time refusal for a named parameter list. Say so on T2
before starting it.

## Files owned

- `module/src/runtime_script.rs` (`PICCOLO_SHIM` and the argument-semantics tests only)
- `module/Cargo.toml`, workspace `Cargo.toml` (`[patch.crates-io]`), `Cargo.lock`, branch A only

## Out of scope

- Every other part of the Runtime Script Host contract: the Fuel Budget numbers, `ALLOWED_GLOBALS`,
  `ALLOWED_LIBRARY_MEMBERS`, the Host Operations, Entity Handles, the compiler cache.
- The `table.concat` shim's existence.
- Any TypeScript, any artifact, any CLI change.

## Acceptance tests

- A missing argument reads `nil` in every shape in the table above, with scratch work of each kind
  (table constructor, string concatenation, arithmetic, call) ahead of the call.
- `x = x or default` yields the default when the argument is omitted, after earlier scratch work.
- Passing every argument still passes every argument.
- `select('#', ...)` still reports the true arity.
- The whole existing `module` test suite passes unchanged, including
  `the_representative_script_fits_the_fuel_budget_with_room_to_spare`.
- Branch A only: the module still builds for `wasm32-unknown-unknown`.
