local c = require("c")

local b = {}

function b.get_value()
    return c.base_value() * 2
end

return b
