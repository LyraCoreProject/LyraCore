local function answer(event)
    grant_xp(event.actor, 3)
    assert(event.actor.level > 0, "invalid level")
    return event.actor.level - 12
end

events.package.on("answer", answer)
