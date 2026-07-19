local typed = require("./typed")

local p: typed.Point = { x = 1, y = 2 }
local result = typed.add(p.x, p.y)
print(result)
return result
