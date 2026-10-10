local function ding(event)
    send_chat(event.player, "Ding " .. event.newLevel)
end

events.player.onLevelUp(ding)
