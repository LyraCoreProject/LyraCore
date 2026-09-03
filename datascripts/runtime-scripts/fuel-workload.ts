// @event on_login
// @id 100999
//
// The transpiler-shaped workload the Runtime Script Host's Fuel Budget is sized against, in the
// language a Package author writes it in.
//
// It is not installed anywhere. The directives above are there only so a test can copy this file
// straight into a scratch Package and get a real Runtime Script out; no Package ships it. Its ONLY job is to be compiled by the pinned
// toolchain and to have its Lua committed next to it, so two tests can meet on the same bytes:
// `datascripts/tests/runtime-scripts.test.ts` recompiles it and refuses a stale `.lua`, and the
// Module's `the_generated_representative_script_fits_the_fuel_budget` runs that same `.lua` on the
// Host. What the budget is sized against is therefore real transpiler output, not a hand-written
// impression of it.
//
// It reaches for the shapes the lua library has to supply: an array built by pushing, a
// higher-order map, string work, a join, and a Host Verb.

function script(): number | void {
  const names: string[] = [];
  for (let i = 1; i <= 20; i += 1) {
    names.push("unit" + i);
  }
  const shouted = names.map((name) => name.toUpperCase());
  const roster = shouted.join(",");
  const actor = event.actor;
  if (actor && roster.length > 0) {
    grant_xp(actor, 25);
  }
  return roster.length;
}
