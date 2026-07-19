local c = require("c")

local b = {}

function b.compute()
    return c.compute() + 1
end

return b
