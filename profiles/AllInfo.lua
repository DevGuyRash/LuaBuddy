return {
    after = function(context)
        PZAPI.ModOptions:load()
        assert(AllInfo and AllInfo.providers and #AllInfo.providers > 0, "AllInfo providers did not initialize")
        local main = MainOptions and MainOptions.instance
        if context.runtime and main and main.tabs and not main.allInfoTabBuilt then
            main.allInfoTabBuilt = true
            AllInfo.OptionsTab.build(main)
        end
        if context.active.DetailedDescriptionsForOccupationsAndTraits then
            print("[LuaBuddy] AllInfo trait screens omitted because DetailedDescriptionsForOccupationsAndTraits is active.")
        end
    end,
}
