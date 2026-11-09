return {
    after = function()
        PZAPI.ModOptions:load()
        PZAPI.ModOptions:getOptions("EURY_CRAFTING"):apply()
    end,
}
