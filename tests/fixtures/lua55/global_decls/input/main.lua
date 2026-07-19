local counters = require("counters")

global total
total = 0

for _, delta in ipairs({ 1, 2, 3 }) do
	total = total + counters.bump(delta)
end

print(total)
