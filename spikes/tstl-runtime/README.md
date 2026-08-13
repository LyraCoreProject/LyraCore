# TSTL runtime compatibility spikes (#96, #97)

Offline executable findings for the representative hook-shaped TypeScript program in `src/`.
Neither harness performs a live-client or realm operation.

## #96: Lua 5.4 output on piccolo

`./run-piccolo.sh` transpiles with TSTL 1.37.1, executes under piccolo 0.3.3 in
64-fuel slices, asserts the literal result, and builds for `wasm32-unknown-unknown`.
Observed: `PASS piccolo=0.3.3 fuel_per_tick=64 ticks=19 result=HOOK:18:6-12|HOOK:9:9|27`.

Unshimmed output fails with `type error, expected function, found nil`. The emitted globals and
piccolo compatibility table isolate the missing function to `table.concat`; the harness prepends
an eight-line implementation. The wasm build also needs both getrandom generations enabled and
`getrandom_backend="wasm_js"`, captured by the manifest and script. Conclusion: feasible with a
tiny explicit shim for this lualib subset; inventory helpers as the Tier-2 corpus grows.

## #97: Lua 5.1 output on Lua 5.0.3

`./run-lua50.sh` checksum-verifies and builds the official Lua 5.0.3 source, transpiles, applies
`downlevel-lua50.sed`, and asserts the same result. Raw output fails at its first `#` operator.
The five narrowly matched rewrites use `table.getn` and Lua 5.0's implicit `arg` table; classes,
closures, filter/map/reduce/join, and string operations then pass. Observed:
`PASS lua=5.0.3 result=HOOK:18:6-12|HOOK:9:9|27`.
