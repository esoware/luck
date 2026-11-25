type User = { name: string, email: string? }

local users: { [number]: User } = {}

local function find(id: number): User?
	return users[id]
end

if local user = find(1) then
	if local email = user.email then
		print(email)
	end
elseif const fallback: User? = find(2) then
	print(fallback.name)
else
	print("nobody")
end

local queue = { 3, 2, 1 }
while const id = table.remove(queue) do
	print(id)
end

local label = if local user = find(3) then user.name elseif local guest = find(4) then guest.name else "anonymous"
print(label)

-- A long binding breaks before its `then`.
if local someone_with_a_long_name = find(5) or find(6) or find(7) or find(8) or find(9) then
	print(someone_with_a_long_name.name)
end

-- `const` with no name after it is a variable.
local const = true
if const then
	print(const)
end
