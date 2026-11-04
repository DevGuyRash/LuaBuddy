return {
    after = function()
        PZAPI.ModOptions:load()
        if not NeatInventory.enabled then return end
        for i = 0, getNumActivePlayers() - 1 do
            for _, page in pairs({getPlayerInventory(i), getPlayerLoot(i)}) do
                if page and not page.ninvSearch then
                    page:clearChildren()
                    page:createChildren()
                end
            end
        end
    end,
}
