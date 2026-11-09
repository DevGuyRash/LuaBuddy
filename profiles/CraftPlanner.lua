return {
    after = function()
        PZAPI.ModOptions:load()
        for i = 0, getNumActivePlayers() - 1 do
            local player = getSpecificPlayer(i)
            if player and getPlayerInventory(i) and getPlayerLoot(i) and CraftPlanner.getState(player).visible then
                CraftPlanner.setVisible(player, true)
            end
        end
    end,
}
