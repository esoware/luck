local typed = {}

export type Point = {x: number, y: number}

type InternalState = {
    count: number,
    active: boolean,
}

local function add(a: number, b: number): number
    return a + b
end

local function distance(p1: Point, p2: Point): number
    local dx: number = p2.x - p1.x
    local dy: number = p2.y - p1.y
    return (dx * dx + dy * dy) ^ 0.5
end

typed.add = add
typed.distance = distance

return typed
