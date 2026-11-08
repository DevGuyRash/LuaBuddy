return {
    after = function()
        PZAPI.ModOptions:load()
        if not NeatInventory.enabled then return end
        for i = 0, getNumActivePlayers() - 1 do
            for _, page in pairs({getPlayerInventory(i), getPlayerLoot(i)}) do
                if page and not page.ninvSearch then
                    local pinned = page.pin
                    page:clearChildren()
                    page:recalcSize()
                    page:createChildren()
                    if pinned then page:setPinned() end
                end
            end
        end
    end,
}
