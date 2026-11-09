local mods, active, state, operation = ...
if state.execute then
    if mods then return state.execute(mods, active, operation) end
    return
end
local baseRequire = require
local compile = state.compile or loadstring
local registry, aliases, byId = {}, {}, {}
local running = {}
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
    state.details[mod.id] = mod.detail
    running[mod.id] = nil
    print("[LuaBuddy] " .. mod.id .. " failed stage=" .. mod.detail.stage
        .. "; no retry in this Lua environment: " .. tostring(err))
    if mod.detail.traceback then print("[LuaBuddy] " .. mod.id .. " Lua traceback:\n" .. mod.detail.traceback) end
    if mod.detail.javaTraceback then print("[LuaBuddy] " .. mod.id .. " Java traceback:\n" .. mod.detail.javaTraceback) end
end

local function contains(values, value)
    for _, candidate in ipairs(values or {}) do
        if candidate == value then return true end
    end
    return false
end

local function replay(mod, event, callbacks)
    for _, callback in ipairs(callbacks or state.callbacks(event, mod.sourceFiles)) do
        stage(mod, "replay-" .. event, getFilenameOfClosure(callback))
        callback()
    end
end

state.execute = function(mods, available, operation)
    operation = operation or "startup"
    local runtime = operation ~= "startup"
    local replacing = operation == "reload"
    local changed = false
    for id in pairs(running) do running[id] = nil end
    for id, value in pairs(available or {}) do running[id] = value end

    local function reject(mod, err, trace, cause)
        mod.failed = true
        if replacing then
            state.error = "Reload rejected: " .. tostring(err)
            if trace then state.traceback = tostring(trace) end
            print("[LuaBuddy] " .. state.error)
        else
            fail(mod, err, trace, cause)
        end
    end

    for _, mod in ipairs(mods) do
        mod.detail = mod.detail or state.details[mod.id] or {status = "prepared"}
        mod.entries = {}
        mod.sourceFiles = {}
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
                mod.sourceFiles[filename] = true
            end
            table.sort(mod.entries, function(a, b) return a.name:lower() < b.name:lower() end)
            mod.pluginChunk = plugin
            for id in pairs(running) do
                assert(not contains(mod.before, id), (runtime and "Runtime" or "Startup") .. " order requires "
                    .. mod.id .. " before " .. id .. "; already loaded; reconnect with a compatible selection")
            end
            if runtime then
                local old = replacing and byId[mod.id] or nil
                if replacing then
                    assert(old and state.mods[mod.id] == "loaded", "Only a locally loaded profile can reload")
                    assert(type(old.hooks.unload) == "function", "Profile has no unload hook; reconnect to reload " .. mod.id)
                    assert(ZombieBuddy and ZombieBuddy.Events and ZombieBuddy.Events.getByFile,
                        "ZombieBuddy callback ownership API is unavailable")
                end
                for id, existing in pairs(byId) do
                    if existing ~= old and running[id] then
                        assert(not contains(existing.after, mod.id), "Runtime order requires " .. mod.id .. " before " .. id .. "; reconnect")
                        if replacing then
                            assert(not contains(existing.required, mod.id) and not contains(old.before, id),
                                "Running profile " .. id .. " depends on the current generation; reconnect")
                            for filename in pairs(mod.sourceFiles) do
                                assert(not existing.sourceFiles[filename], "Source is shared with " .. id .. "; reconnect")
                            end
                        end
                    end
                end
                for _, entry in ipairs(mod.entries) do
                    local previous = registry[moduleKey(entry.name)]
                    assert(not previous or previous.owner == old,
                        "Runtime source conflicts with a loaded profile: " .. entry.name .. "; reconnect")
                    local alias = aliases[moduleKey(entry.name:sub(entry.name:find("/") + 1))]
                    local overrides = entry.name:sub(1, 7) == "shared/" or (alias and alias.name:sub(1, 7) == "client/")
                    assert(not alias or alias.owner == old or not overrides,
                        "Runtime import conflicts with a loaded profile: " .. entry.name .. "; reconnect")
                end
            end
        end)
        mod.sources, mod.plugin, mod.filenames = nil, nil, nil
        if not ok then reject(mod, err, trace, cause) end
    end

    for _, mod in ipairs(mods) do
        if not mod.failed then
            local old = replacing and byId[mod.id] or nil
            if old then
                local ok, consent, reason, cause = pcall(old.hooks.unload, old.context)
                if ok and consent == false then
                    reject(mod, reason or "Cleanup refused; the running generation was retained")
                elseif not ok or consent ~= true then
                    changed = true
                    stage(old, "unload", old.pluginFilename)
                    fail(old, ok and "unload must return true after cleanup; reconnect" or consent, reason, cause)
                    mod.failed = true
                else
                    changed = true
                    local cleaned, err, trace, cause = pcall(function()
                        stage(old, "unload", old.pluginFilename)
                        for filename in pairs(old.sourceFiles) do
                            for event, callbacks in pairs(ZombieBuddy.Events.getByFile(filename)) do
                                for _, callback in ipairs(callbacks) do Events[event].Remove(callback) end
                            end
                        end
                        for key, entry in pairs(registry) do
                            if entry.owner == old then registry[key] = nil end
                        end
                        for key, entry in pairs(aliases) do
                            if entry.owner == old then aliases[key] = nil end
                        end
                        running[mod.id] = nil
                    end)
                    if not cleaned then
                        fail(old, err, trace, cause)
                        mod.failed = true
                    end
                end
            end
            if not mod.failed then
                byId[mod.id] = mod
                state.details[mod.id] = mod.detail
                local ok, err, trace, cause = pcall(function()
                    changed = true
                    stage(mod, "plugin-initialize", mod.pluginFilename)
                    mod.hooks = mod.pluginChunk()
                    mod.pluginChunk = nil
                    assert(type(mod.hooks) == "table", "Plugin must return a table of hook functions")
                    for _, hook in ipairs({"before", "after", "unload"}) do
                        assert(mod.hooks[hook] == nil or type(mod.hooks[hook]) == "function", "Invalid plugin hook " .. hook)
                    end
                    if mod.publish then
                        local ready, message, javaTrace = mod.publish()
                        if not ready then
                            mod.detail.javaTraceback = javaTrace
                            error(message)
                        end
                        mod.publish = nil
                    end
                end)
                if not ok then fail(mod, err, trace, cause) end
            end
        end
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

    local function checkDependencies(mod)
        stage(mod, "dependencies")
        for _, id in ipairs(mod.required) do
            assert(running[id] or (byId[id] and not byId[id].failed), "Required mod did not load: " .. id)
        end
    end

    for _, mod in ipairs(mods) do
        if not mod.failed then
            local ok, err, trace, cause = pcall(function()
                checkDependencies(mod)
                mod.context = {id = mod.id, active = running, state = state, runtime = runtime, operation = operation}
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
                            assert(running[id] or state.mods[id] == "loaded", "Required mod did not load: " .. id)
                        end
                        for _, name in ipairs(mod.replay or {}) do
                            if name == "OnGameBoot" then replay(mod, name) end
                        end
                        stage(mod, "after", mod.pluginFilename)
                        if mod.hooks.after then mod.hooks.after(mod.context) end
                        state.mods[mod.id] = "loaded"
                        mod.detail.status = "loaded"
                        stage(mod, "complete")
                        running[mod.id] = true
                        print("[LuaBuddy] " .. mod.id .. (runtime and " loaded during play (" or " restored before OnGameStart (") .. #mod.entries .. " Lua files).")
                    end
                end)
                if not ok then fail(mod, err, trace, cause) end
            end
        end
    end
    if runtime then
        for _, event in ipairs({"OnGameStart", "OnLoad"}) do
            local files, owners, requested = {}, {}, false
            for _, mod in ipairs(mods) do
                if not mod.failed and contains(mod.replay, event) then
                    requested = true
                    for filename in pairs(mod.sourceFiles) do
                        files[filename], owners[filename] = true, mod
                    end
                end
            end
            for _, callback in ipairs(requested and state.callbacks(event, files) or {}) do
                local mod = owners[getFilenameOfClosure(callback)]
                if not mod.failed then
                    local ok, err, trace, cause = pcall(function()
                        if not contains(state.callbacks(event, mod.sourceFiles), callback) then return end
                        checkDependencies(mod)
                        replay(mod, event, {callback})
                    end)
                    if not ok then fail(mod, err, trace, cause) end
                end
            end
        end
    end
    local loaded, failed = 0, 0
    for _, mod in ipairs(mods) do
        if runtime and not mod.failed then
            local ok, err, trace, cause = pcall(function()
                checkDependencies(mod)
                stage(mod, "complete")
            end)
            if not ok then fail(mod, err, trace, cause) end
        end
        if mod.failed then failed = failed + 1 else loaded = loaded + 1 end
    end
    if failed > 0 then return false, state.error or (loaded .. " profiles loaded; " .. failed .. " failed or refused. See LuaBuddy diagnostics."), changed end
    return true, loaded == 0 and "No new enabled profiles to load."
        or (loaded .. (loaded == 1 and " profile loaded." or " profiles loaded.")), changed
end
state.require = localRequire
require = localRequire
if mods then return state.execute(mods, active, operation) end
