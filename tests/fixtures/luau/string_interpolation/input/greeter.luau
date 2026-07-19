local greeter = {}

function greeter.hello(name: string): string
    local msg = `hello {name}`
    return msg
end

function greeter.farewell(name: string, time: string): string
    return `goodbye {name}, see you at {time}`
end

return greeter
