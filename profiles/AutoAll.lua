return {
    after = function()
        assert(AutoAll and AutoAll.coreLoaded and AutoAll.mechLoaded and AutoAll.mechUIHooked,
            "AutoAll Mechanics did not initialize")
    end,
}
