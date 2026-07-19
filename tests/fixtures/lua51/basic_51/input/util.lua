local util = {}

function util.sum(...)
	local total = 0
	for _, value in ipairs({ ... }) do
		total = total + value
	end
	return total
end

return util
