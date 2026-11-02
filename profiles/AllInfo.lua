return {
    after = function(context)
        PZAPI.ModOptions:load()
        assert(AllInfo and AllInfo.providers and #AllInfo.providers > 0, "AllInfo providers did not initialize")
        if context.active.DetailedDescriptionsForOccupationsAndTraits then
            print("[LuaBuddy] AllInfo trait screens omitted because DetailedDescriptionsForOccupationsAndTraits is active.")
        end
    end,
}
