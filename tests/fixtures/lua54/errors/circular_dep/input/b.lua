local a = require("a")

local b = {}

function b.value()
    return a.value() + 1
end

return b
