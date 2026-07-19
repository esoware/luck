local shared = require("shared")

local b = {}

function b.value()
    return shared.base + 2
end

return b
