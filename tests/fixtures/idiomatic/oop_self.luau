-- Methods that ignore `self`, write-then-read locals, and shadowing that
-- is deliberate. All idiomatic; default lint must stay silent.

local Shape = {}
Shape.__index = Shape

function Shape.new(kind)
	return setmetatable({ kind = kind }, Shape)
end

-- Ignores self on purpose: interface consistency.
function Shape:typeName()
	return "shape"
end

function Shape:kindName()
	return self.kind
end

local shape = Shape.new("circle")
print(shape:typeName(), shape:kindName())

local result
local attempts = 0
repeat
	attempts = attempts + 1
	if attempts >= 2 then
		result = attempts * 10
	end
until result ~= nil
print(result)
