return {
    after = function()
        for i = 0, getNumActivePlayers() - 1 do
            local data = getPlayerData(i)
            if data and data.vehicleDashboard then
                data.vehicleDashboard:removeFromUIManager()
                local player = getSpecificPlayer(i)
                local dashboard = ISVehicleDashboard:new(i, player)
                dashboard:initialise()
                dashboard:instantiate()
                data.vehicleDashboard = dashboard
                local vehicle = player:getVehicle()
                if vehicle and vehicle:isDriver(player) then dashboard:setVehicle(vehicle) end
            end
        end
    end,
}
