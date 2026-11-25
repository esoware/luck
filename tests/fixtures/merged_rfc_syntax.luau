export type Identifier = number | string
type NonNil = number | string

export const MAX_ID: number = 0x1fffffffffffff
export local current: Identifier = 1

export function convert<T>(value: T): T
    return value
end

local converted = convert<<number>>(current)
local mapped = converter:map<<number, string>>(converted)
current = converted
