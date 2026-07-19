local counters = {}

local current <const> = { base = 10 }

function counters.bump(delta)
	return current.base + delta
end

return counters
