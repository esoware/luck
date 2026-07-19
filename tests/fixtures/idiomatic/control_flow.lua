-- Branch initialization, upvalue captures, in-file global functions:
-- all idiomatic, all zero-diagnostic material under default settings.

local config
if os.getenv("MODE") == "fast" then
	config = { retries = 1 }
else
	config = { retries = 3 }
end
print(config.retries)

local cached = 1
local function getCached()
	return cached
end
print(getCached())
cached = 2
print(getCached())

-- Intentional global function (script style): setting_global warning is
-- correct here — Luacheck's 111 and Selene's unscoped_variables agree —
-- but calling it must NOT be an undefined_variable error.
-- luck: allow(setting_global)
function describeAll(items)
	local lines = {}
	for index, item in ipairs(items) do
		lines[index] = tostring(item)
	end
	return table.concat(lines, "\n")
end

print(describeAll({ 1, 2, 3 }))

local ok, err = pcall(function()
	error("expected")
end)
if not ok then
	print(err)
end
