return {
    after = function()
        PZAPI.ModOptions:load()
        for i = 0, getNumActivePlayers() - 1 do
            local ui = getPlayerMechanicsUI(i)
            if ui then
                local width, height = ui:getWidth(), ui:getHeight()
                NeatMechanics.Window.setup(ui)
                ui:setWidth(width)
                ui:setHeight(height)
            end
            local data = getPlayerData(i)
            if data and data.equipped then NeatMechanics.Sidebar.add(data.equipped) end
        end
    end,
}
