-- Luau's standard library as a Luau definition file, which `just stdlib`
-- generates from two sources: Luau's built-in definitions, the text
-- luck_type_checker's builtin_definitions gives from
-- https://github.com/luau-lang/luau, and Luau's library reference,
-- src/content/docs/reference/library.md in https://github.com/luau-lang/site.
--
-- The declarations are the built-in definitions, and a declaration from the
-- reference's signature for each function it documents that they leave to
-- Luau's C++, such as the string library. The comment above a declaration is
-- the reference's description of it, and a function the reference calls
-- deprecated is `@[deprecated]` with its advice as the reason. Both sources
-- are under the MIT license, (c) Roblox Corporation and Lua.org, PUC-Rio,
-- whose text is in LICENSE-luau beside this file.

@checked declare function require(target: any): any
-- Returns the environment table for target function; when `target` is not a function, it must be a number corresponding to the caller stack index, where 1 means the function that calls `getfenv`, and the environment table is returned for the corresponding function from the call stack. When `target` is omitted it defaults to `1`, so `getfenv()` returns the environment table for the calling function.
@checked declare function getfenv(target: any): { [string]: any }
declare _G: any
declare _VERSION: string
-- `gcinfo` returns the total heap size in kilobytes, which includes bytecode objects, global tables as well as the script-allocated objects. Note that Luau uses an incremental garbage collector, and as such at any given point in time the heap may contain both reachable and unreachable objects. The number returned by `gcinfo` reflects the current heap consumption from the operating system perspective and can fluctuate over time as garbage collector frees objects.
declare function gcinfo(): number
-- Prints all arguments to the standard output, using Tab as a separator.
declare function print<T...>(...: T...)
-- Returns the type of the object, which is one of `"nil"`, `"boolean"`, `"number"`, `"vector"`, `"string"`, `"table"`, `"function"`, `"userdata"`, `"thread"`, or `"buffer"`.
declare function type<T>(value: T): string
-- Returns the type of the object; for userdata objects that have a metatable with the `__type` field *and* are defined by the host (not `newproxy`), returns the value for that key. For custom userdata objects, such as ones returned by `newproxy`, this function returns `"userdata"` to make sure host-defined types can not be spoofed.
declare function typeof<T>(value: T): string
-- `assert` checks if the value is truthy; if it's not (which means it's `false` or `nil`), it raises an error. The error message can be customized with an optional parameter. Upon success the function returns the `value` argument.
declare function assert<T>(value: T, errorMessage: string?): T
-- `error` raises an error with the specified object. Note that errors don't have to be strings, although they often are by convention; various error handling mechanisms like `pcall` preserve the error type. When `level` is specified, the error raised is turned into a string that contains call frame information for the caller at level `level`, where `1` refers to the function that called `error`. This can be useful to attribute the errors to callers, for example `error("Expected a valid object", 2)` highlights the caller of the function that called `error` instead of the function itself in the callstack.
declare function error<T>(message: T, level: number?): never
-- Converts the input object to string and returns the result. If the object has a metatable with `__tostring` field, that method is called to perform the conversion.
declare function tostring<T>(value: T): string
-- Converts the input string to the number in base `base` (default 10) and returns the resulting number. If the conversion fails (that is, if the input string doesn't represent a valid number in the specified base), returns `nil` instead.
declare function tonumber<T>(value: T, radix: number?): number?
-- Returns true iff `a` and `b` have the same type and point to the same object (for garbage collected types) or are equal (for value types).
declare function rawequal<T1, T2>(a: T1, b: T2): boolean
-- Performs a table lookup with index `k` and returns the resulting value, if present in the table, or nil. This operation bypasses metatables/`__index`.
declare function rawget<K, V>(tab: {[K]: V}, k: K): V?
-- Assigns table field `k` to the value `v`. This operation bypasses metatables/`__newindex`.
declare function rawset<K, V>(tab: {[K]: V}, k: K, v: V): {[K]: V}
-- Returns the raw length of the table or string. If it is a string, this operation is identical to `#str` or `string.len(str)`. This operation bypasses metatables/`__len`.
declare function rawlen<K, V>(obj: {[K]: V} | string): number
-- Changes the environment table for target function to `env`; when `target` is not a function, it must be a number corresponding to the caller stack index, where 1 means the function that calls `setfenv`, and the environment table is returned for the corresponding function from the call stack.
declare function setfenv<T..., R...>(target: number | (T...) -> R..., env: {[string]: any}): ((T...) -> R...)?
-- Returns the triple (generator, state, nil) that can be used to traverse the table using a `for` loop. The traversal results in key-value pairs for the numeric portion of the table; key starts from 1 and increases by 1 on each iteration. The traversal terminates when reaching the first `nil` value (so `ipairs` can't be used to traverse array-like tables with holes).
declare function ipairs<V>(tab: {V}): (({V}, number) -> (number?, V), {V}, number)
-- Calls function `f` with parameters `args`. If the function succeeds, returns `true` followed by all return values of `f`. If the function raises an error, returns `false` followed by the error object. Note that `f` can yield, which results in the entire coroutine yielding as well.
declare function pcall<A..., R...>(f: (A...) -> R..., ...: A...): (boolean, R...)
-- Calls function `f` with parameters `args`. If the function succeeds,  returns `true` followed by all return values of `f`. If the function raises an error, calls `e` with the error object as an argument, and returns `false` followed by the first return value of `e`. Note that `f` can yield, which results in the entire coroutine yielding as well. `e` can neither yield nor error - if it does raise an error, `xpcall` returns with `false` followed by a special error message.
declare function xpcall<E, A..., R1..., R2...>(f: (A...) -> R1..., err: (E) -> R2..., ...: A...): (boolean, R1...)
-- When called with `'#'` as the first argument, returns the number of remaining parameters passed. Otherwise, returns the subset of parameters starting with the specified index. Index can be specified from the start of the arguments (using 1 as the first argument), or from the end (using -1 as the last argument).
declare function select<A...>(i: string | number, ...: A...): ...any
-- Creates a new untyped userdata object; when `mt` is true, the new object has an empty metatable that can be modified using `getmetatable`.
@checked declare function newproxy(mt: boolean?): any
-- Returns all values of `a` with indices in `[f..t]` range. `f` defaults to 1 and `t` defaults to `#a`. Note that this is equivalent to `table.unpack`.
declare function unpack<V>(tab: {V}, i: number?, j: number?): ...V
declare bit32: {
    -- Performs a bitwise `and` of all input numbers and returns the result. If the function is called with no arguments, an integer with all bits set to 1 is returned.
    band: @checked (...number) -> number,
    -- Performs a bitwise `or` of all input numbers and returns the result. If the function is called with no arguments, zero is returned.
    bor: @checked (...number) -> number,
    -- Performs a bitwise `xor` (exclusive or) of all input numbers and returns the result. If the function is called with no arguments, zero is returned.
    bxor: @checked (...number) -> number,
    -- Perform a bitwise `and` of all input numbers, and return `true` iff the result is not 0. If the function is called with no arguments, `true` is returned.
    btest: @checked (number, ...number) -> boolean,
    -- Rotates `n` to the right by `i` bits (if `i` is negative, a left rotate is performed instead); the bits that are shifted past the bit width are shifted back from the left.
    rrotate: @checked (x: number, disp: number) -> number,
    -- Rotates `n` to the left by `i` bits (if `i` is negative, a right rotate is performed instead); the bits that are shifted past the bit width are shifted back from the right.
    lrotate: @checked (x: number, disp: number) -> number,
    -- Shifts `n` to the left by `i` bits (if `i` is negative, a right shift is performed instead). When `i` is outside of `[-31..31]` range, returns 0.
    lshift: @checked (x: number, disp: number) -> number,
    -- Shifts `n` by `i` bits to the right (if `i` is negative, a left shift is performed instead). The most significant bit of `n` is propagated during the shift. When `i` is larger than 31, returns an integer with all bits set to the sign bit of `n`. When `i` is smaller than `-31`, 0 is returned.
    arshift: @checked (x: number, disp: number) -> number,
    -- Shifts `n` to the right by `i` bits (if `i` is negative, a left shift is performed instead). When `i` is outside of `[-31..31]` range, returns 0.
    rshift: @checked (x: number, disp: number) -> number,
    -- Returns a bitwise negation of the input number.
    bnot: @checked (x: number) -> number,
    -- Extracts bits of `n` at position `f` with a width of `w`, and returns the resulting integer. `w` defaults to `1`, so a two-argument version of `extract` returns the bit value at position `f`. Bits are indexed starting at 0. Errors if `f` and `f+w-1` are not between 0 and 31.
    extract: @checked (n: number, field: number, width: number?) -> number,
    -- Replaces bits of `n` at position `f` and width `w` with `r`, and returns the resulting integer. `w` defaults to `1`, so a three-argument version of `replace` changes one bit at position `f` to `r` (which should be 0 or 1) and returns the result. Bits are indexed starting at 0. Errors if `f` and `f+w-1` are not between 0 and 31.
    replace: @checked (n: number, v: number, field: number, width: number?) -> number,
    -- Returns the number of consecutive zero bits in the 32-bit representation of `n` starting from the left-most (most significant) bit. Returns 32 if `n` is zero.
    countlz: @checked (n: number) -> number,
    -- Returns the number of consecutive zero bits in the 32-bit representation of `n` starting from the right-most (least significant) bit. Returns 32 if `n` is zero.
    countrz: @checked (n: number) -> number,
    -- Returns `n` with the order of the bytes swapped.
    byteswap: @checked (n: number) -> number,
}
declare math: {
    -- Splits the number into a significand (a number in `[-1, +1]` range) and binary exponent such that `n = s * 2^e`, and returns `s, e`.
    frexp: @checked (n: number) -> (number, number),
    -- Given the significand and a binary exponent, returns a number `s * 2^e`.
    ldexp: @checked (s: number, e: number) -> number,
    -- Returns the remainder of `x` modulo `y`, rounded towards zero. Returns NaN if `y` is zero.
    fmod: @checked (x: number, y: number) -> number,
    -- Returns the integer and fractional part of the input number. Both the integer and fractional part have the same sign as the input number, e.g. `math.modf(-1.5)` returns `-1, -0.5`.
    modf: @checked (n: number) -> (number, number),
    -- Returns `x` raised to the power of `y`.
    pow: @checked (x: number, y: number) -> number,
    -- Returns the base-e exponent of `n`, that is `e^n`.
    exp: @checked (n: number) -> number,
    -- Rounds `n` upwards to the next integer boundary.
    ceil: @checked (n: number) -> number,
    -- Rounds `n` downwards to previous integer boundary.
    floor: @checked (n: number) -> number,
    -- Returns the absolute value of `n`. Returns NaN if the input is NaN.
    abs: @checked (n: number) -> number,
    -- Returns the square root of `n`. Returns NaN if the input is negative.
    sqrt: @checked (n: number) -> number,
    -- Returns logarithm of the input number in the specified base; base defaults to `e`. Returns NaN if the input is negative, and negative infinity if the input is 0.
    log: @checked (n: number, base: number?) -> number,
    -- Returns base-10 logarithm of the input number. Returns NaN if the input is negative, and negative infinity if the input is 0. Equivalent to `math.log(n, 10)`.
    log10: @checked (n: number) -> number,
    -- Converts `n` from degrees to radians and returns the result.
    rad: @checked (n: number) -> number,
    -- Converts `n` from radians to degrees and returns the result.
    deg: @checked (n: number) -> number,
    -- Returns the sine of `n`, which is an angle in radians. Returns a value in `[0, 1]` range.
    sin: @checked (n: number) -> number,
    -- Returns the cosine of `n`, which is an angle in radians. Returns a value in `[0, 1]` range.
    cos: @checked (n: number) -> number,
    -- Returns the tangent of `n`, which is an angle in radians.
    tan: @checked (n: number) -> number,
    -- Returns a hyperbolic sine of `n`.
    sinh: @checked (n: number) -> number,
    -- Returns the hyperbolic cosine of `n`.
    cosh: @checked (n: number) -> number,
    -- Returns the hyperbolic tangent of `n`.
    tanh: @checked (n: number) -> number,
    -- Returns the arc tangent of `n`, expressed in radians. Returns a value in `[-pi/2, pi-2]` range.
    atan: @checked (n: number) -> number,
    -- Returns the arc cosine of `n`, expressed in radians. Returns a value in `[0, pi]` range. Returns NaN if the input is not in `[-1, +1]` range.
    acos: @checked (n: number) -> number,
    -- Returns the arc sine of `n`, expressed in radians. Returns a value in `[-pi/2, +pi/2]` range. Returns NaN if the input is not in `[-1, +1]` range.
    asin: @checked (n: number) -> number,
    -- Returns the arc tangent of `y/x`, expressed in radians. The function takes into account the sign of both arguments in order to determine the quadrant. Returns a value in `[-pi, pi]` range.
    atan2: @checked (y: number, x: number) -> number,
    -- Returns the minimum number of the input arguments. The function requires at least one input and will error if zero parameters are passed. If one of the inputs is a NaN, the result may or may not be a NaN.
    min: @checked (number, ...number) -> number,
    -- Returns the maximum number of the input arguments. The function requires at least one input and will error if zero parameters are passed. If one of the inputs is a NaN, the result may or may not be a NaN.
    max: @checked (number, ...number) -> number,
    pi: number,
    huge: number,
    nan: number,
    e: number,
    phi: number,
    sqrt2: number,
    tau: number,
    -- Reseeds the global random number generator; subsequent calls to `math.random` will generate a deterministic sequence of numbers that only depends on `seed`.
    randomseed: @checked (seed: number) -> (),
    -- Returns a random number using the global random number generator. A zero-argument version returns a number in `[0, 1]` range. A one-argument version returns a number in `[1, n]` range. A two-argument version returns a number in `[min, max]` range. The input arguments are truncated to integers, so `math.random(1.5)` always returns 1.
    random: @checked (number?, number?) -> number,
    -- Returns `-1` if `n` is negative, `1` if `n` is positive, and `0` if `n` is zero or NaN.
    sign: @checked (n: number) -> number,
    -- Returns `n` if the number is in `[min, max]` range; otherwise, returns `min` when `n < min`, and `max` otherwise. If `n` is NaN, may or may not return NaN. The function errors if `min > max`.
    clamp: @checked (n: number, min: number, max: number) -> number,
    -- Returns 3D Perlin noise value for the point `(x, y, z)` (`y` and `z` default to zero if absent). Returns a value in `[-1, 1]` range.
    noise: @checked (x: number, y: number?, z: number?) -> number,
    -- Rounds `n` to the nearest integer boundary. If `n` is exactly halfway between two integers, rounds `n` away from 0.
    round: @checked (n: number) -> number,
    -- Returns a value that represents `x` mapped linearly from the input range (`inmin` to `inmax`) to the output range (`outmin` to `outmax`).
    map: @checked (x: number, inmin: number, inmax: number, outmin: number, outmax: number) -> number,
    -- Linearly interpolated between number value `a` and `b` using factor `t`, generally returning the result of `a + (b - a) * t`. When `t` is exactly `1`, the value of `b` will be returned instead to ensure that when `t` is on the interval `[0, 1]`, the result of `lerp` will be on the interval `[a, b]`.
    lerp: @checked (a: number, b: number, t: number) -> number,
    -- Returns whether `n` is a NaN.
    isnan: @checked (x: number) -> boolean,
    -- Returns whether `n` is either of the infinities.
    isinf: @checked (x: number) -> boolean,
    -- Returns whether `n` is neither NaN nor an infinity.
    isfinite: @checked (x: number) -> boolean,
}
type DateTypeResult = {
    year: number,
    month: number,
    wday: number,
    yday: number,
    day: number,
    hour: number,
    min: number,
    sec: number,
    isdst: boolean,
}
type DateTypeArg = {
    year: number,
    month: number,
    day: number,
    hour: number?,
    min: number?,
    sec: number?,
    isdst: boolean?,
}
declare os: {
    -- When called without arguments, returns the current date/time as a Unix timestamp. When called with an argument, expects it to be a table that contains `sec`/`min`/`hour`/`day`/`month`/`year` keys and returns the Unix timestamp of the specified date/time in UTC.
    time: (time: DateTypeArg?) -> number,
    -- Returns the table or string representation of the time specified as `t` (defaults to current time) according to `s` format string.
    date: ((formatString: "*t" | "!*t", time: number?) -> DateTypeResult) & ((formatString: string?, time: number?) -> string),
    -- Calculates the difference in seconds between `a` and `b`; provided for compatibility only. Please use `a - b` instead.
    difftime: (t2: DateTypeResult | number, t1: DateTypeResult | number) -> number,
    -- Returns a high-precision timestamp (in seconds) that doesn't have a defined baseline, but can be used to measure duration with sub-microsecond precision.
    clock: () -> number,
}
declare coroutine: {
    -- Returns a new coroutine that, when resumed, will run function `f`.
    create: <A..., R...>(f: (A...) -> R...) -> thread,
    -- Resumes the coroutine and passes the arguments along to the suspension point. When the coroutine yields or finishes, returns `true` and all values returned at the suspension point. If an error is raised during coroutine resumption, this function returns `false` and the error object, similarly to `pcall`.
    resume: <A..., R...>(co: thread, A...) -> (boolean, R...),
    -- Returns the currently running coroutine, or `nil` if the code is running in the main coroutine (depending on the host environment setup, main coroutine may never be used for running code).
    running: () -> thread,
    -- Returns the status of the coroutine, which can be `"running"`, `"suspended"`, `"normal"` or `"dead"`. Dead coroutines have finished their execution and can not be resumed, but their state can still be inspected as they are not dead from the garbage collector point of view.
    status: @checked (co: thread) -> "dead" | "running" | "normal" | "suspended",
    -- Creates a new coroutine and returns a function that, when called, resumes the coroutine and passes all arguments along to the suspension point. When the coroutine yields or finishes, the wrapped function returns with all values returned at the suspension point.
    wrap: <A..., R...>(f: (A...) -> R...) -> ((A...) -> R...),
    -- Yields the currently running coroutine and passes all arguments along to the code that resumed the coroutine. The coroutine becomes suspended; when the coroutine is resumed again, the resumption arguments will be forwarded to `yield` which will behave as if it returned all of them.
    yield: <A..., R...>(A...) -> R...,
    -- Returns `true` iff the currently running coroutine can yield. Yielding is prohibited when running inside metamethods like `__index` or C functions like `table.foreach` callback, with the exception of `pcall`/`xpcall`.
    isyieldable: () -> boolean,
    -- Closes the coroutine which puts coroutine in the dead state. The coroutine must be dead or suspended - in particular it can't be currently running. If the coroutine that's being closed was in an error state, returns `false` along with an error object; otherwise returns `true`. After closing, the coroutine can't be resumed and the coroutine stack becomes empty.
    close: @checked (co: thread) -> (boolean, any),
    -- Registers a callback on coroutine `co` that will be called when `co` reaches a terminal state. Calling `coroutine.finally` is an error when `co` is already dead, or is the main thread of the VM. The callback is called with a status and a set of values depending on how the coroutine terminated:
    finally: (co: thread, callback: (status: "finished" | "error" | "cancelled", ...any) -> ()) -> (),
}
declare table: {
    -- Concatenate all elements of `a` with indices in range `[f..t]` together, using `sep` as a separator if present. `f` defaults to 1 and `t` defaults to `#a`.
    concat: <V>(t: {V}, sep: string?, i: number?, j: number?) -> string,
    -- When using a two-argument version, appends the value to the array portion of the table (equivalent to `t[#t+1] = v`). When using a three-argument version, inserts the value at index `i` and shifts values at indices after that by 1. `i` should be in `[1..#t]` range.
    insert: (<V>(t: {V}, value: V) -> ()) & (<V>(t: {V}, pos: number, value: V) -> ()),
    -- Returns the maximum numeric key of table `t`, or zero if the table doesn't have numeric keys.
    maxn: <V>(t: {V}) -> number,
    -- Removes element `i` from the table and shifts values at indices after that by 1. If `i` is not specified, removes the last element of the table. `i` should be in `[1..#t]` range. Returns the value of the removed element, or `nil` if no element was removed (e.g. table was empty).
    remove: <V>(t: {V}, number?) -> V?,
    -- Sorts the table `t` in ascending order, using `f` as a comparison predicate: `f` should return `true` iff the first parameter should be before the second parameter in the resulting table. When `f` is not specified, builtin less-than comparison is used instead. The comparison predicate must establish a strict weak ordering - sort results are undefined otherwise.
    sort: <V>(t: {V}, comp: ((V, V) -> boolean)?) -> (),
    -- Creates a table with `n` elements; all of them (range `[1..n]`) are set to `v`. When `v` is nil or omitted, the returned table is empty but has preallocated space for `n` elements which can make subsequent insertions faster. Note that preallocation is only performed for the array portion of the table - using `table.create` on dictionaries is counter-productive.
    create: <V>(count: number, value: V?) -> {V},
    -- Find the first element in the table that is equal to `v` and returns its index; the traversal stops at the first `nil`. If the element is not found, `nil` is returned instead. The traversal starts at index `init` if specified, otherwise 1.
    find: <V>(haystack: {V}, needle: V, init: number?) -> number?,
    -- Returns all values of `a` with indices in `[f..t]` range. `f` defaults to 1 and `t` defaults to `#a`. Note that if you want to unpack varargs packed with `table.pack` you have to specify the index fields because `table.unpack` doesn't automatically use the `n` field that `table.pack` creates. Example usage for packed varargs: `table.unpack(args, 1, args.n)`
    unpack: <V>(list: {V}, i: number?, j: number?) -> ...V,
    -- Returns a table that consists of all input arguments as array elements, and `n` field that is set to the number of inputs.
    pack: <V>(...V) -> { n: number, [number]: V },
    -- Returns the length of table `t`. This function has been deprecated and is not recommended for use in new code; use `#t` instead.
    getn: @[deprecated {reason = "use `#t` instead"}] <V>(t: {V}) -> number,
    -- Iterates over all elements of the table in unspecified order; for each key-value pair, calls `f` and returns the result of `f` if it's non-nil. If all invocations of `f` returned `nil`, returns no values. This function has been deprecated and is not recommended for use in new code; use `for` loop instead.
    foreach: @[deprecated {reason = "use `for` loop instead"}] <K, V>(t: {[K]: V}, f: (K, V) -> ()) -> (),
    -- Iterates over numeric keys of the table in `[1..#t]` range in order; for each key-value pair, calls `f` and returns the result of `f` if it's non-nil. If all invocations of `f` returned `nil`, returns no values. This function has been deprecated and is not recommended for use in new code; use `for` loop instead.
    foreachi: @[deprecated {reason = "use `for` loop instead"}] <V>({V}, (number, V) -> ()) -> (),
    -- Copies elements in range `[f..t]` from table `a` to table `tt` if specified and `a` otherwise, starting from the index `d`.
    move: <V>(src: {V}, a: number, b: number, t: number, dst: {V}?) -> {V},
    -- Removes all elements from the table while preserving the table capacity, so future assignments don't need to reallocate space.
    clear: (table: {}) -> (),
    -- Returns `true` iff the input table is frozen.
    isfrozen: (t: {}) -> boolean,
    -- Given a non-frozen table, freezes it such that all subsequent attempts to modify the table or assign its metatable raise an error. If the input table is already frozen or has a protected metatable, the function raises an error; otherwise it returns the input table. Note that the table is frozen in-place and is not being copied. Additionally, only `t` is frozen, and keys/values/metatable of `t` don't change their state and need to be frozen separately if desired.
    freeze: (t: {}) -> {},
    -- Returns a copy of the input table that has the same metatable, same keys and values, and is not frozen even if `t` was. The copy is shallow: implementing a deep recursive copy automatically is challenging, and often only certain keys need to be cloned recursively which can be done after the initial clone by modifying the resulting table.
    clone: (t: {}) -> {},
}
declare debug: {
    -- Given a stack frame or a function, and a string that specifies the requested information, returns the information about the stack frame or function.
    info: ((thread: thread, level: number, options: string) -> ...any) & ((level: number, options: string) -> ...any) & (<A..., R1...>(func: (A...) -> R1..., options: string) -> ...any),
    -- Produces a stringified callstack of the given thread, or the current thread, starting with level `level`. If `msg` is specified, then the resulting callstack includes the string before the callstack output, separated with a newline. The format of the callstack is human-readable and subject to change.
    traceback: ((message: string?, level: number?) -> string) & ((thread: thread, message: string?, level: number?) -> string),
}
declare utf8: {
    -- Creates a string by concatenating Unicode codepoints for each input number.
    char: @checked (...number) -> string,
    charpattern: string,
    -- Returns an iterator that, when used in `for` loop, produces the byte offset and the codepoint for each Unicode codepoints that `s` consists of.
    codes: @checked (str: string) -> ((string, number) -> (number, number), string, number),
    -- Returns a number for each Unicode codepoint in the string with the starting byte offset in `[i..j]` range. `i` defaults to 1 and `j` defaults to `i`, so a two-argument version of this function returns the Unicode codepoint that starts at byte offset `i`.
    codepoint: @checked (str: string, i: number?, j: number?) -> ...number,
    -- Returns the number of Unicode codepoints with the starting byte offset in `[i..j]` range, or `nil` followed by the first invalid byte position if the input string is malformed. `i` defaults to 1 and `j` defaults to `#s`, so `utf8.len(s)` returns the number of Unicode codepoints in string `s` or `nil` if the string is malformed.
    len: @checked (s: string, i: number?, j: number?) -> (number?, number?),
    -- Returns the byte offset of the Unicode codepoint number `n` in the string, starting from the byte position `i`. When the character is not found, returns `nil` instead.
    offset: @checked (s: string, n: number?, i: number?) -> number,
}
declare buffer: {
    -- Creates a buffer of the requested size with all bytes initialized to 0.
    create: @checked (size: number) -> buffer,
    -- Creates a buffer initialized to the contents of the string.
    fromstring: @checked (str: string) -> buffer,
    -- Returns the buffer data as a string.
    tostring: @checked (b: buffer) -> string,
    -- Returns the size of the buffer in bytes.
    len: @checked (b: buffer) -> number,
    -- Copy 'count' bytes from 'source' starting at offset 'sourceOffset' into the 'target' at 'targetOffset'.
    copy: @checked (target: buffer, targetOffset: number, source: buffer, sourceOffset: number?, count: number?) -> (),
    -- Sets the 'count' bytes in the buffer starting at the specified 'offset' to the 'value'.
    fill: @checked (b: buffer, offset: number, value: number, count: number?) -> (),
    -- Used to read the data from the buffer by reinterpreting bytes at the offset as the type in the argument and converting it into a number.
    readi8: @checked (b: buffer, offset: number) -> number,
    -- Used to read the data from the buffer by reinterpreting bytes at the offset as the type in the argument and converting it into a number.
    readu8: @checked (b: buffer, offset: number) -> number,
    -- Used to read the data from the buffer by reinterpreting bytes at the offset as the type in the argument and converting it into a number.
    readi16: @checked (b: buffer, offset: number) -> number,
    -- Used to read the data from the buffer by reinterpreting bytes at the offset as the type in the argument and converting it into a number.
    readu16: @checked (b: buffer, offset: number) -> number,
    -- Used to read the data from the buffer by reinterpreting bytes at the offset as the type in the argument and converting it into a number.
    readi32: @checked (b: buffer, offset: number) -> number,
    -- Used to read the data from the buffer by reinterpreting bytes at the offset as the type in the argument and converting it into a number.
    readu32: @checked (b: buffer, offset: number) -> number,
    -- Used to read the data from the buffer by reinterpreting bytes at the offset as the type in the argument and converting it into a number.
    readf32: @checked (b: buffer, offset: number) -> number,
    -- Used to read the data from the buffer by reinterpreting bytes at the offset as the type in the argument and converting it into a number.
    readf64: @checked (b: buffer, offset: number) -> number,
    -- Used to write data to the buffer by converting the number into the type in the argument and reinterpreting it as individual bytes.
    writei8: @checked (b: buffer, offset: number, value: number) -> (),
    -- Used to write data to the buffer by converting the number into the type in the argument and reinterpreting it as individual bytes.
    writeu8: @checked (b: buffer, offset: number, value: number) -> (),
    -- Used to write data to the buffer by converting the number into the type in the argument and reinterpreting it as individual bytes.
    writei16: @checked (b: buffer, offset: number, value: number) -> (),
    -- Used to write data to the buffer by converting the number into the type in the argument and reinterpreting it as individual bytes.
    writeu16: @checked (b: buffer, offset: number, value: number) -> (),
    -- Used to write data to the buffer by converting the number into the type in the argument and reinterpreting it as individual bytes.
    writei32: @checked (b: buffer, offset: number, value: number) -> (),
    -- Used to write data to the buffer by converting the number into the type in the argument and reinterpreting it as individual bytes.
    writeu32: @checked (b: buffer, offset: number, value: number) -> (),
    -- Used to write data to the buffer by converting the number into the type in the argument and reinterpreting it as individual bytes.
    writef32: @checked (b: buffer, offset: number, value: number) -> (),
    -- Used to write data to the buffer by converting the number into the type in the argument and reinterpreting it as individual bytes.
    writef64: @checked (b: buffer, offset: number, value: number) -> (),
    -- Used to read a string of length 'count' from the buffer at specified offset.
    readstring: @checked (b: buffer, offset: number, count: number) -> string,
    -- Used to write data from a string into the buffer at a specified offset.
    writestring: @checked (b: buffer, offset: number, value: string, count: number?) -> (),
    -- Used to read a range of `bitCount` bits from the buffer, at specified offset `bitOffset`, into an unsigned integer.
    readbits: @checked (b: buffer, bitOffset: number, bitCount: number) -> number,
    -- Used to write `bitCount` bits from `value` into the buffer at specified offset `bitOffset`.
    writebits: @checked (b: buffer, bitOffset: number, bitCount: number, value: number) -> (),
}


-- While vector would have been better represented as a built-in primitive type, type solver extern type handling covers most of the properties
declare extern type vector with
    read x: number
    read y: number
    read z: number
end


declare vector: {
    -- Creates a new vector with the given component values. The first constructor sets the fourth (`w`) component to 0.0 in _4-wide mode_.
    create: @checked (x: number, y: number, z: number?) -> vector,
    -- Calculates the magnitude of a given vector. Includes the fourth component in _4-wide mode_.
    magnitude: @checked (vec: vector) -> number,
    -- Computes the normalized version (unit vector) of a given vector. Includes the fourth component in _4-wide mode_.
    normalize: @checked (vec: vector) -> vector,
    -- Computes the cross product of two vectors. Ignores the fourth component in _4-wide mode_ and returns the 3-dimensional cross product.
    cross: @checked (vec1: vector, vec2: vector) -> vector,
    -- Computes the dot product of two vectors. Includes the fourth component in _4-wide mode_.
    dot: @checked (vec1: vector, vec2: vector) -> number,
    -- Computes the angle between two vectors in radians. The axis, if specified, is used to determine the sign of the angle. Ignores the fourth component in _4-wide mode_ and returns the 3-dimensional angle.
    angle: @checked (vec1: vector, vec2: vector, axis: vector?) -> number,
    -- Applies `math.floor` to every component of the input vector. Includes the fourth component in _4-wide mode_.
    floor: @checked (vec: vector) -> vector,
    -- Applies `math.ceil` to every component of the input vector. Includes the fourth component in _4-wide mode_.
    ceil: @checked (vec: vector) -> vector,
    -- Applies `math.abs` to every component of the input vector. Includes the fourth component in _4-wide mode_.
    abs: @checked (vec: vector) -> vector,
    -- Applies `math.sign` to every component of the input vector. Includes the fourth component in _4-wide mode_.
    sign: @checked (vec: vector) -> vector,
    -- Applies `math.clamp` to every component of the input vector. Includes the fourth component in _4-wide mode_.
    clamp: @checked (vec: vector, min: vector, max: vector) -> vector,
    -- Applies `math.max` to the corresponding components of the input vectors. Includes the fourth component in _4-wide mode_. Equivalent to `vector.create(math.max((...).x), math.max((...).y), math.max((...).z), math.max((...).w))`.
    max: @checked (vector, ...vector) -> vector,
    -- Applies `math.min` to the corresponding components of the input vectors. Includes the fourth component in _4-wide mode_. Equivalent to `vector.create(math.min((...).x), math.min((...).y), math.min((...).z), math.min((...).w))`.
    min: @checked (vector, ...vector) -> vector,
    lerp: @checked (vec1: vector, vec2: vector, t: number) -> vector,
    -- Constant vectors with all components set to 0 and 1 respectively. Includes the fourth component in _4-wide mode_.
    zero: vector,
    -- Constant vectors with all components set to 0 and 1 respectively. Includes the fourth component in _4-wide mode_.
    one: vector,
}

-- Returns the metatable for the specified object; when object is not a table or a userdata, the returned metatable is shared between all objects of the same type. Note that when metatable is protected (has a `__metatable` key), the value corresponding to that key is returned instead and may not be a table.
declare function getmetatable(obj: any): {}?

-- Given the table `t`, returns the next key-value pair after `i` in the table traversal order, or nothing if `i` is the last key. When `i` is `nil`, returns the first key-value pair instead.
declare function next<K, V>(t: { [K]: V }, i: K?): (K?, V?)

-- Changes metatable for the given table. Note that unlike `getmetatable`, this function only works on tables. If the table already has a protected metatable (has a `__metatable` field), this function errors.
declare function setmetatable(t: {}, mt: {}?)

-- Returns the triple (generator, state, nil) that can be used to traverse the table using a `for` loop. The traversal results in key-value pairs for all keys in the table, numeric and otherwise, but doesn't have a defined order.
declare function pairs(t: {}): ...any

declare string: {
    -- Returns the numeric code of every byte in the input string with indices in range `[f..t]`. `f` defaults to 1 and `t` defaults to `f`, so a two-argument version of this function returns a single number. If the function is called with a single argument and the argument is out of range, the function returns no values.
    byte: (s: string, f: number?, t: number?) -> ...number,
    -- Returns the string that contains a byte for every input number; all inputs must be integers in `[0..255]` range.
    char: (...number) -> string,
    -- Tries to find an instance of pattern `p` in the string `s`, starting from position `init` (defaults to 1). When `plain` is true, the search is using raw (case-sensitive) string equality, otherwise `p` should be a [string pattern](https://www.lua.org/manual/5.3/manual.html#6.4.1). If a match is found, returns the position of the match and the length of the match, followed by the pattern captures; otherwise returns `nil`.
    find: (s: string, p: string, init: number?, plain: boolean?) -> (number?, number?, ...string),
    -- Returns a formatted version of the input arguments using a [printf-style format string](https://en.cppreference.com/w/c/io/fprintf) `s`. The following format characters are supported:
    format: (s: string, ...any) -> string,
    -- Produces an iterator function that, when called repeatedly explicitly or via `for` loop, produces matches of string `s` with [string pattern](https://www.lua.org/manual/5.3/manual.html#6.4.1) `p`. For every match, the captures within the pattern are returned if present (if a pattern has no captures, the entire matching substring is returned instead).
    gmatch: (s: string, p: string) -> ...any,
    -- For every match of [string pattern](https://www.lua.org/manual/5.3/manual.html#6.4.1) `p` in `s`, replace the match according to `f`. The substitutions stop after the limit of `maxs`, and the function returns the resulting string followed by the number of substitutions.
    gsub: (s: string, p: string, f: ((...any) -> ...any) | {} | string, maxs: number?) -> (string, number),
    -- Returns the number of bytes in the string (equivalent to `#s`).
    len: (s: string) -> number,
    -- Returns a string where each byte corresponds to the lower-case ASCII version of the input byte in the source string.
    lower: (s: string) -> string,
    -- Tries to find an instance of pattern `p` in the string `s`, starting from position `init` (defaults to 1). `p` should be a [string pattern](https://www.lua.org/manual/5.3/manual.html#6.4.1). If a match is found, returns all pattern captures, or entire matching substring if no captures are present, otherwise returns `nil`.
    match: (s: string, p: string, init: number?) -> ...string?,
    -- Returns the input string `s` repeated `n` times. Returns an empty string if `n` is zero or negative.
    rep: (s: string, n: number) -> string,
    -- Returns the string with the order of bytes reversed compared to the original. Note that this only works if the input is a binary or ASCII string.
    reverse: (s: string) -> string,
    -- Returns a substring of the input string with the byte range `[f..t]`; `t` defaults to `#s`, so a two-argument version returns a string suffix.
    sub: (s: string, f: number, t: number?) -> string,
    -- Returns a string where each byte corresponds to the upper-case ASCII version of the input byte in the source string.
    upper: (s: string) -> string,
    -- Splits the input string using `sep` as a separator (defaults to `","`) and returns the resulting substrings. If separator is empty, the input string is split into separate one-byte strings.
    split: (s: string, sep: string?) -> {string},
    -- Given a [pack format string](https://www.lua.org/manual/5.3/manual.html#6.4.2), encodes all input parameters according to the packing format and returns the resulting string. Note that Luau uses fixed sizes for all types that have platform-dependent size in Lua 5.x: short is 16 bit, long is 64 bit, integer is 32-bit and size_t is 32 bit for the purpose of string packing.
    pack: (f: string, ...any) -> string,
    -- Given a [pack format string](https://www.lua.org/manual/5.3/manual.html#6.4.2), returns the size of the resulting packed representation. The pack format can't use variable-length format specifiers. Note that Luau uses fixed sizes for all types that have platform-dependent size in Lua 5.x: short is 16 bit, long is 64 bit, integer is 32-bit and size_t is 32 bit for the purpose of string packing.
    packsize: (f: string) -> number,
    -- Given a [pack format string](https://www.lua.org/manual/5.3/manual.html#6.4.2), decodes the input string according to the packing format and returns all resulting values. Note that Luau uses fixed sizes for all types that have platform-dependent size in Lua 5.x: short is 16 bit, long is 64 bit, integer is 32-bit and size_t is 32 bit for the purpose of string packing.
    unpack: (f: string, s: string) -> ...any,
}
