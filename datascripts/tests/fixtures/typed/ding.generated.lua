local ____lyracore_handler
local events = { player = { onLevelUp = function(handler) ____lyracore_handler = handler end } }
local function ding(event)
    send_chat(event.player, "Ding " .. event.newLevel)
end

events.player.onLevelUp(ding)
assert(event.name == "on_levelup", "Event Binding does not match the Invocation")
assert(event.player and event.player.is_player, "on_levelup requires a Character")
return ____lyracore_handler(event)
