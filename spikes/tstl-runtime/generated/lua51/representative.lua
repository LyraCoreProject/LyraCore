-- Lua Library inline imports
local function __TS__Class(self)
    local c = {prototype = {}}
    c.prototype.__index = c.prototype
    c.prototype.constructor = c
    return c
end

local function __TS__ArrayFilter(self, callbackfn, thisArg)
    local result = {}
    local len = 0
    for i = 1, #self do
        if callbackfn(thisArg, self[i], i - 1, self) then
            len = len + 1
            result[len] = self[i]
        end
    end
    return result
end

local function __TS__ArrayMap(self, callbackfn, thisArg)
    local result = {}
    for i = 1, #self do
        result[i] = callbackfn(thisArg, self[i], i - 1, self)
    end
    return result
end

local function __TS__CountVarargs(...)
    return select("#", ...)
end

local function __TS__ArrayReduce(self, callbackFn, ...)
    local len = #self
    local k = 0
    local accumulator = nil
    if __TS__CountVarargs(...) ~= 0 then
        accumulator = ...
    elseif len > 0 then
        accumulator = self[1]
        k = 1
    else
        error("Reduce of empty array with no initial value", 0)
    end
    for i = k + 1, len do
        accumulator = callbackFn(
            nil,
            accumulator,
            self[i],
            i - 1,
            self
        )
    end
    return accumulator
end

local function __TS__New(target, ...)
    local instance = setmetatable({}, target.prototype)
    instance:____constructor(...)
    return instance
end
-- End of Lua Library inline imports
HookCounter = __TS__Class()
HookCounter.name = "HookCounter"
function HookCounter.prototype.____constructor(self, prefix)
    self.prefix = prefix
    self.total = 0
end
function HookCounter.prototype.hook(self, values)
    local normalized = __TS__ArrayMap(
        __TS__ArrayFilter(
            values,
            function(____, value) return value >= 2 end
        ),
        function(____, value) return value * 3 end
    )
    local sum = __TS__ArrayReduce(
        normalized,
        function(____, acc, value) return acc + value end,
        0
    )
    local function closeOver(____, suffix)
        return (((string.upper(self.prefix) .. ":") .. tostring(sum)) .. ":") .. string.lower(suffix)
    end
    self.total = self.total + sum
    return closeOver(
        nil,
        table.concat(normalized, "-")
    )
end
function HookCounter.prototype.count(self)
    return self.total
end
handler = __TS__New(HookCounter, "hook")
first = handler:hook({1, 2, 4})
second = handler:hook({3})
SPIKE_RESULT = (((first .. "|") .. second) .. "|") .. tostring(handler:count())
