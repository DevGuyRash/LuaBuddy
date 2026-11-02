local mods, active, state = ...
local baseRequire = require
local compile = state.compile or loadstring
local registry, aliases, byId = {}, {}, {}
state.details = state.details or {}

local function moduleKey(name)
    local key = name:gsub("\\", "/"):lower()
    return (key:gsub("%.lua$", ""))
end

local function stage(mod, name, source)
    mod.detail.stage = name
    mod.detail.source = source
    if state.debug then
        print("[LuaBuddy] " .. mod.id .. " stage=" .. name .. (source and " source=" .. source or ""))
    end
end

local function capture(mod, trace, cause)
    if trace and not mod.detail.traceback then mod.detail.traceback = tostring(trace) end
    if cause and state.stackTrace and not mod.detail.javaTraceback then
        mod.detail.javaTraceback = state.stackTrace(cause)
    end
end

local function execute(entry)
    if entry.failed then error(entry.failed) end
    if entry.done then return entry.value end
    if entry.loading then error("Circular Lua dependency: " .. entry.name) end
    entry.loading = true
    local ok, value, trace, cause = pcall(entry.chunk)
    entry.chunk = nil
    entry.loading = false
    if not ok then
        capture(entry.owner, trace, cause)
        entry.failed = "Loading " .. entry.filename .. ": " .. tostring(value)
        error(entry.failed)
    end
    entry.done, entry.value = true, value
    return value
end

local function localRequire(name)
    local entry = aliases[moduleKey(name)]
    if not entry then return baseRequire(name) end
    if entry.owner.failed then error("Mod unavailable: " .. entry.owner.id) end
    return execute(entry)
end

local function fail(mod, err, trace, cause)
    mod.failed = true
    capture(mod, trace, cause)
    mod.detail.status = "failed"
    mod.detail.error = tostring(err)
    state.mods[mod.id] = "failed: " .. tostring(err)
    print("[LuaBuddy] " .. mod.id .. " failed stage=" .. mod.detail.stage
        .. "; no retry in this Lua environment: " .. tostring(err))
    if mod.detail.traceback then print("[LuaBuddy] " .. mod.id .. " Lua traceback:\n" .. mod.detail.traceback) end
    if mod.detail.javaTraceback then print("[LuaBuddy] " .. mod.id .. " Java traceback:\n" .. mod.detail.javaTraceback) end
end

for _, mod in ipairs(mods) do
    byId[mod.id] = mod
    mod.detail = state.details[mod.id] or {status = "prepared"}
    state.details[mod.id] = mod.detail
    mod.entries = {}
    mod.pluginFilename = mod.pluginFilename or "LuaBuddy/plugins/" .. mod.id
    local ok, err, trace, cause = pcall(function()
        stage(mod, "plugin-compile", mod.pluginFilename)
        local plugin, message, javaTrace = compile(mod.plugin, mod.pluginFilename)
        if not plugin then
            mod.detail.javaTraceback = javaTrace
            error(message)
        end
        for name, source in pairs(mod.sources) do
            local filename = mod.filenames and mod.filenames[name] or mod.id .. "/" .. name
            stage(mod, "compile", filename)
            local chunk, message, javaTrace = compile(source, filename)
            if not chunk then
                mod.detail.javaTraceback = javaTrace
                error("Compiling " .. filename .. ": " .. tostring(message))
            end
            mod.entries[#mod.entries + 1] = {name = name, filename = filename, chunk = chunk, owner = mod}
        end
        table.sort(mod.entries, function(a, b) return a.name:lower() < b.name:lower() end)
        stage(mod, "plugin-initialize", mod.pluginFilename)
        mod.hooks = plugin()
        assert(type(mod.hooks) == "table", "Plugin must return a table of before/after functions")
        for _, hook in ipairs({"before", "after"}) do
            assert(mod.hooks[hook] == nil or type(mod.hooks[hook]) == "function", "Invalid plugin hook " .. hook)
        end
    end)
    mod.sources, mod.plugin, mod.filenames = nil, nil, nil
    if not ok then fail(mod, err, trace, cause) end
end

for _, phase in ipairs({"shared", "client"}) do
    for _, mod in ipairs(mods) do
        if not mod.failed then
            for _, entry in ipairs(mod.entries) do
                if entry.name:sub(1, #phase + 1) == phase .. "/" then
                    local key = moduleKey(entry.name)
                    registry[key] = entry
                    local alias = moduleKey(entry.name:sub(#phase + 2))
                    if phase == "shared" or not aliases[alias] or aliases[alias].name:sub(1, 7) == "client/" then
                        aliases[alias] = entry
                    end
                end
            end
        end
    end
end

require = localRequire
local function checkDependencies(mod)
    stage(mod, "dependencies")
    for _, id in ipairs(mod.required) do
        assert(active[id] or (byId[id] and not byId[id].failed), "Required mod did not load: " .. id)
    end
end

for _, mod in ipairs(mods) do
    if not mod.failed then
        local ok, err, trace, cause = pcall(function()
            checkDependencies(mod)
            mod.context = {id = mod.id, active = active, state = state}
            for _, name in ipairs(mod.preload) do
                stage(mod, "preload", name)
                require(name)
            end
            stage(mod, "before", mod.pluginFilename)
            if mod.hooks.before then mod.hooks.before(mod.context) end
        end)
        if not ok then fail(mod, err, trace, cause) end
    end
end

for _, phase in ipairs({"shared", "client"}) do
    for _, mod in ipairs(mods) do
        if not mod.failed then
            local ok, err, trace, cause = pcall(function()
                checkDependencies(mod)
                for _, entry in ipairs(mod.entries) do
                    if entry.name:sub(1, #phase + 1) == phase .. "/" and registry[moduleKey(entry.name)] == entry then
                        stage(mod, phase, entry.filename)
                        execute(entry)
                    end
                end
                if phase == "client" then
                    checkDependencies(mod)
                    for _, id in ipairs(mod.required) do
                        assert(active[id] or state.mods[id] == "loaded", "Required mod did not load: " .. id)
                    end
                    stage(mod, "after", mod.pluginFilename)
                    if mod.hooks.after then mod.hooks.after(mod.context) end
                    state.mods[mod.id] = "loaded"
                    mod.detail.status = "loaded"
                    stage(mod, "complete")
                    active[mod.id] = true
                    print("[LuaBuddy] " .. mod.id .. " restored before OnGameStart (" .. #mod.entries .. " Lua files).")
                end
            end)
            if not ok then fail(mod, err, trace, cause) end
        end
    end
end
state.require = localRequire
