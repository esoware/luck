local utils = require("./utils")
local helper = require("./helper")

local sum = utils.add(2, 3)
local greeting = helper.greet("world")
print(sum)
print(greeting)
return { sum = sum, greeting = greeting }
