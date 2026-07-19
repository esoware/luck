local d = require("d")

local c = {}

function c.compute()
    return d.compute() + 1
end

return c
