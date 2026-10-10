local ____lyracore_handler
local events = { player = { onLogin = function(handler) ____lyracore_handler = handler end } }
local function ____tbl(t)
    return t
end
local function welcome(event)
    send_chat(event.player, "Welcome, " .. event.player.name)
end
events.player.onLogin(welcome)
assert(event.name == "on_login", "Event Binding does not match the Invocation")
assert(event.player and event.player.is_player, "on_login requires a Character")
return ____lyracore_handler(event)
