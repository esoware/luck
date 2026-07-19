local foo = require("@root/foo")
local bar = require("@mid/bar")

local result = foo.value .. " " .. bar.value
print(result)
return result
