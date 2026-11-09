return {
    after = function(context)
        PZAPI.ModOptions:load()
        if not NeatInventory.enabled then return end
        for i = 0, getNumActivePlayers() - 1 do
            for name, page in pairs({["inventory" .. i] = getPlayerInventory(i), ["loot" .. i] = getPlayerLoot(i)}) do
                if page and not page.ninvSearch then
                    local pinned = page.pin
                    local visible = page:getIsVisible()
                    local layout
                    if context.runtime then
                        layout = {}
                        page:SaveLayout(name, layout)
                    end
                    page:clearChildren()
                    page:recalcSize()
                    page:createChildren()
                    ISLayoutManager.TryRestore(name)
                    if layout then page:RestoreLayout(name, layout) end
                    page:setVisible(visible)
                    if pinned then page:setPinned() else page:collapse() end
                end
            end
        end
    end,
}
