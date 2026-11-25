local helper = require("./helper")

-- Resolved at runtime by the host's own `require`, since only then is the
-- name known.
local name = "./helper"
local same = require(name)

return helper.value + same.value
