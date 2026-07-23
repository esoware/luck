export type Identifier = integer | string
type NonNil = ~nil

export const MAX_ID: integer = 0x7fffffffffffffffi
export local current: Identifier = 1i

export function convert<T>(value: T): T
    return value
end

local converted = convert<<integer>>(current)
local mapped = converter:map<<integer, string>>(converted)
current = converted
