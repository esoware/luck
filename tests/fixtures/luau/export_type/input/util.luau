local M = {}
function M.add(a: number, b: number): number
	return a + b
end
export type Adder = typeof(M.add)
return M
