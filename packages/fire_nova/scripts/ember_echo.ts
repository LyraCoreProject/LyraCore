// @event on_cast_resolved
// @id 100200
// @enabled false
//
// The worked example of a Runtime Script. It ships SWITCHED OFF: `on_cast_resolved` fires for every
// cast on a realm, and a demonstration should not talk to every player who casts anything. Switch
// it on by changing the directive above and running `lyracore packages build`.
//
// What it shows: reading the event's entities, a Host Verb, and a Script Answer.

/// How loudly the echo lands, as a share of the caster's remaining health.
function echoShare(caster: Entity): number {
  if (caster.max_health === 0) return 0;
  return Math.floor((caster.health * 100) / caster.max_health);
}

function script(): number | void {
  const caster = event.actor;
  if (!caster || !caster.is_player) return;

  const share = echoShare(caster);
  const words = ["fire", "nova", "echo"];
  send_chat(caster, words.join(" ") + ": " + share);
  return share;
}
