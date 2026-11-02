local bootstrap = assert(loadfile(arg[1] or "resources/bootstrap.lua"))
local originalRequire = require
local calls = 0
local baseRequire = function(name)
    assert(name == "vanilla")
    calls = calls + 1
    return "native-result"
end

local function mod(id, sources, required, plugin)
    return {id = id, sources = sources, required = required or {}, plugin = plugin or "return {}", preload = {}}
end

local function run(mods)
    require = baseRequire
    local state = {mods = {}}
    bootstrap(mods, {}, state)
    assert(require == state.require, "the environment must retain the resolver for callbacks")
    return state
end

falseLoads, nilLoads = 0, 0
local state = run({mod("CacheFixture", {
    ["shared/Cache/False.lua"] = "falseLoads = falseLoads + 1; return false",
    ["shared/Cache/Nil.lua"] = "nilLoads = nilLoads + 1",
    ["client/Cache/Consumer.lua"] = "assert(require('vanilla') == 'native-result')",
})})
assert(state.mods.CacheFixture == "loaded")
assert(require("Cache/False.lua") == false and require("Cache\\False") == false)
assert(require("Cache/Nil") == nil and require("Cache/Nil") == nil)
assert(falseLoads == 1 and nilLoads == 1 and calls == 1, "cache false/nil results and delegate unknown modules")

effects = 0
state = run({
    mod("RuntimeFailure", {["client/Crash.lua"] = "effects = effects + 1; error('runtime fixture')"}),
    mod("Unrelated", {["client/Healthy.lua"] = "return 5"}),
})
assert(state.mods.RuntimeFailure:find("failed:", 1, true) == 1 and state.mods.Unrelated == "loaded")
assert(not pcall(require, "Crash") and effects == 1, "failed chunks cannot repeat their side effects")
assert(require("Healthy") == 5, "unrelated imports survive a mod failure")

state = run({mod("Cycle", {
    ["shared/Cycle/A.lua"] = "require('Cycle/B')",
    ["shared/Cycle/B.lua"] = "require('Cycle/A')",
})})
assert(state.mods.Cycle:find("Circular Lua dependency", 1, true), "cycles must report failure without recursion overflow")
assert(not pcall(require, "Cycle/A"), "a cycle remains failed for this environment")

state = run({
    mod("First", {["shared/Override.lua"] = "error('shadowed source must not execute')"}),
    mod("Last", {["shared/Override.lua"] = "return 17"}),
})
assert(state.mods.First == "loaded" and state.mods.Last == "loaded" and require("Override") == 17,
    "later mods replace files at the same virtual path")

state = run({mod("BadPlugin", {["client/Empty.lua"] = "return 0"}, {}, "return {before = true}")})
assert(state.mods.BadPlugin:find("Invalid plugin hook", 1, true), "malformed plugin hooks must fail their own mod")

require = originalRequire
print("PASS: retained require, false/nil cache, native delegation, runtime failure without retry, unrelated continuation, circular dependency, file overrides, invalid plugin")
