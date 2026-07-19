-- The canonical Lua module pattern. A linter that warns anywhere in this
-- file with default settings has a false-positive bug.
local M = {}

M.version = "1.0.0"

function M.greet(name)
	return "hello, " .. name
end

function M:describe()
	return "module " .. self.version
end

local Counter = {}
Counter.__index = Counter

function Counter.new(start)
	local instance = setmetatable({}, Counter)
	instance.count = start or 0
	return instance
end

function Counter:increment()
	self.count = self.count + 1
	return self.count
end

M.Counter = Counter

return M
