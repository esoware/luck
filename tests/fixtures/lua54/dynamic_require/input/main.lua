local helper = require("helper")

-- Resolved at runtime: the name is only known then, but "helper" is in the
-- bundle, so the loader still serves it from there.
local name = "helper"
local same = require(name)

return helper.value + same.value
