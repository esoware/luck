local shared = require("shared")

local a = {}

function a.value()
    return shared.base + 1
end

return a
