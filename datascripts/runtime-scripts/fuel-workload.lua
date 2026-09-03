-- Lua Library inline imports
local function __TS__ArrayMap(self, callbackfn, thisArg)
    local result = {}
    for i = 1, #self do
        result[i] = callbackfn(thisArg, self[i], i - 1, self)
    end
    return result
end
-- End of Lua Library inline imports
local function ____tbl(t)
    return t
end
local function script()
    local names = {}
    do
        local i = 1
        while i <= 20 do
            names[#names + 1] = "unit" .. tostring(i)
            i = i + 1
        end
    end
    local shouted = __TS__ArrayMap(
        names,
        function(____, name) return string.upper(name) end
    )
    local roster = table.concat(shouted, ",")
    local actor = event.actor
    if actor and #roster > 0 then
        grant_xp(actor, 25)
    end
    return #roster
end
return script()
