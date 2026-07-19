local b = require("b")

local a = {}

function a.value()
    return b.value() + 1
end

return a
