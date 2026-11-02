local mods, active, state = ...
local baseRequire = require
local registry, aliases, byId = {}, {}, {}

local function moduleKey(name)
    return name:gsub("\\", "/"):gsub("%.lua$", ""):lower()
end

local function execute(entry)
    if entry.failed then error(entry.failed) end
    if entry.done then return entry.value end
    if entry.loading then error("Circular Lua dependency: " .. entry.name) end
    entry.loading = true
    local ok, value = pcall(entry.chunk)
    entry.loading = false
    if not ok then
        entry.failed = "Loading " .. entry.name .. ": " .. tostring(value)
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

local function fail(mod, err)
    mod.failed = true
    state.mods[mod.id] = "failed: " .. tostring(err)
    print("[LuaBuddy] " .. mod.id .. " failed; no retry in this Lua environment: " .. tostring(err))
end

for _, mod in ipairs(mods) do
    byId[mod.id] = mod
    mod.entries = {}
    local ok, err = pcall(function()
        local plugin, message = loadstring(mod.plugin, "LuaBuddy/plugins/" .. mod.id)
        if not plugin then error(message) end
        for name, source in pairs(mod.sources) do
            local chunk, message = loadstring(source, mod.id .. "/" .. name)
            if not chunk then error("Compiling " .. name .. ": " .. tostring(message)) end
            mod.entries[#mod.entries + 1] = {name = name, chunk = chunk, owner = mod}
        end
        table.sort(mod.entries, function(a, b) return a.name:lower() < b.name:lower() end)
        mod.hooks = plugin()
        assert(type(mod.hooks) == "table", "Plugin must return a table of before/after functions")
        for _, hook in ipairs({"before", "after"}) do
            assert(mod.hooks[hook] == nil or type(mod.hooks[hook]) == "function", "Invalid plugin hook " .. hook)
        end
    end)
    if not ok then fail(mod, err) end
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
    for _, id in ipairs(mod.required) do
        assert(active[id] or (byId[id] and not byId[id].failed), "Required mod did not load: " .. id)
    end
end

for _, mod in ipairs(mods) do
    if not mod.failed then
        local ok, err = pcall(function()
            checkDependencies(mod)
            mod.context = {id = mod.id, active = active, state = state}
            for _, name in ipairs(mod.preload) do require(name) end
            if mod.hooks.before then mod.hooks.before(mod.context) end
        end)
        if not ok then fail(mod, err) end
    end
end

for _, phase in ipairs({"shared", "client"}) do
    for _, mod in ipairs(mods) do
        if not mod.failed then
            local ok, err = pcall(function()
                checkDependencies(mod)
                for _, entry in ipairs(mod.entries) do
                    if entry.name:sub(1, #phase + 1) == phase .. "/" and registry[moduleKey(entry.name)] == entry then
                        execute(entry)
                    end
                end
                if phase == "client" then
                    for _, id in ipairs(mod.required) do
                        assert(active[id] or state.mods[id] == "loaded", "Required mod did not load: " .. id)
                    end
                    if mod.hooks.after then mod.hooks.after(mod.context) end
                    state.mods[mod.id] = "loaded"
                    active[mod.id] = true
                    print("[LuaBuddy] " .. mod.id .. " restored before OnGameStart (" .. #mod.entries .. " Lua files).")
                end
            end)
            if not ok then fail(mod, err) end
        end
    end
end
state.require = localRequire
