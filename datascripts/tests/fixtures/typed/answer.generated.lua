local ____lyracore_handler
local events = { package = { on = function(name, handler) ____lyracore_handler = handler end } }
local function answer(event)
    grant_xp(event.actor, 3)
    assert(event.actor.level > 0, "invalid level")
    return event.actor.level - 12
end

events.package.on("answer", answer)
assert(event.name == "example.scripts.answer", "Event Binding does not match the Invocation")
return ____lyracore_handler(event)
