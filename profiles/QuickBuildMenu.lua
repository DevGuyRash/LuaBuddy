return {
    after = function(context)
        PZAPI.ModOptions:load()
        if context.runtime then return end
        Events.OnGameStart.Add(function()
            local player = getPlayer()
            if not player then return end
            local favorites = {}
            local data = player:getModData()
            local recipes = ScriptManager.instance:getAllCraftRecipes()
            for i = 0, recipes:size() - 1 do
                local key = BaseCraftingLogic.getFavouriteModDataString(recipes:get(i))
                if data[key] == true then favorites[key] = true end
            end
            if table.isempty(favorites) then return end
            local function restore()
                Events.OnTick.Remove(restore)
                local current = player:getModData()
                local changed = false
                for key in pairs(favorites) do
                    if current[key] == nil then
                        current[key] = true
                        changed = true
                    end
                end
                if changed and isClient() then player:transmitModData() end
            end
            Events.OnTick.Add(restore)
        end)
    end,
}
