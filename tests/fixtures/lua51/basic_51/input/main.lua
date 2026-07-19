local util = require("util")

local values = { 3, 1, 4, 1, 5 }
local total = util.sum(unpack(values))
print(string.format("total=%d", total))
print(math.fmod(total, 7))

local function count(...)
	return select("#", ...)
end
print(count(unpack(values)))
