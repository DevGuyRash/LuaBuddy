local state = ...
if not (PZAPI and PZAPI.ModOptions) then return end
if PZAPI.ModOptions:getOptions("LuaBuddy") then return end

require "ISUI/ISModalRichText"
require "ISUI/ISTextBox"

local function show(dialog, ignoreReturn)
    local main = MainOptions and MainOptions.instance
    local destroy = dialog.destroy
    dialog.destroy = function(self)
        if self.entry then self.entry:unfocus() end
        if main and main.modal == self then main.modal = nil end
        destroy(self)
    end
    dialog.isKeyConsumed = function(_, key)
        return key == Keyboard.KEY_ESCAPE or key == Keyboard.KEY_RETURN
    end
    dialog.onKeyRelease = function(self, key)
        if not self:getIsVisible() then return end
        if key == Keyboard.KEY_ESCAPE then self:onClick(self.no or self.ok)
        elseif key == Keyboard.KEY_RETURN then
            if ignoreReturn then ignoreReturn = false; return end
            self:onClick(self.yes or self.ok)
        end
    end
    dialog:initialise()
    dialog:setCapture(true)
    dialog:setAlwaysOnTop(true)
    dialog:setWantKeyEvents(true)
    dialog:addToUIManager()
    if main then
        main.modal = dialog
        if main.joyfocus then
            dialog.prevFocus = main.joyfocus.focus
            main.joyfocus.focus = dialog
            updateJoypadFocus(main.joyfocus)
        end
    end
end

local function message(text, ignoreReturn)
    local w = math.min(600, getCore():getScreenWidth() - 40)
    local h = math.min(220, getCore():getScreenHeight() - 40)
    local dialog = ISModalRichText:new((getCore():getScreenWidth() - w) / 2,
        (getCore():getScreenHeight() - h) / 2, w, h, text, false)
    show(dialog, ignoreReturn)
end

state.checkOptions = function()
    local main = MainOptions and MainOptions.instance
    if main and main.gameOptions.changed then
        return false, "Apply or discard pending Options changes before loading or reloading profiles."
    end
    return true
end

local function belongsTo(control, panel)
    while control do
        if control == panel then return true end
        control = control.parent
    end
    return false
end

state.refreshOptions = function()
    local main = MainOptions and MainOptions.instance
    if not (main and main.tabs) then return end
    local title = getText("UI_mainscreen_mods")
    local panel = main.tabs:getView(title)
    if not panel then return end
    local scroll = panel:getYScroll()
    for i = #main.gameOptions.options, 1, -1 do
        local option = main.gameOptions.options[i]
        if belongsTo(option.control, panel) then main.gameOptions:remove(option) end
    end
    for i = #MainOptions.keyText, 1, -1 do
        local binding = MainOptions.keyText[i]
        if binding.isModBind and belongsTo(binding.btn, panel) then table.remove(MainOptions.keyText, i) end
    end
    main.tabs:removeView(panel)
    main:addModOptionsPanel()
    main.gameOptions:toUI()
    main.tabs:activateView(title)
    main.tabs:getView(title):setYScroll(scroll)
end

local function run(id, ignoreReturn)
    local ok, result
    if id then ok, result = state.reload(id) else ok, result = state.refresh() end
    message((ok and "LuaBuddy: " or "LuaBuddy could not complete the operation: ") .. result, ignoreReturn)
end

local options = PZAPI.ModOptions:create("LuaBuddy", "LuaBuddy")
options:addDescription("Load newly enabled profiles during play. Reload requires a profile cleanup hook. Apply pending Options changes first.")
options:addButton("refresh", "Load new profiles", "Read configured profiles without re-running loaded mods.", function() run() end)
options:addButton("reload", "Reload a profile", "Reload one cooperating profile. Other profiles may require reconnecting.", function()
    local ready, reason = state.checkOptions()
    if not ready then message(reason); return end
    local w = math.min(500, getCore():getScreenWidth() - 40)
    local dialog = ISTextBox:new((getCore():getScreenWidth() - w) / 2,
        math.max(20, (getCore():getScreenHeight() - 180) / 2), w, 180, "Configured mod ID to reload", "", nil, function(_, button)
        if button.internal == "OK" and not button.parent.submitted then
            button.parent.submitted = true
            button.parent.entry:unfocus()
            run(button.parent.entry:getText(), button.parent.submitFromKeyboard)
        end
    end)
    show(dialog)
    dialog.entry.onCommandEntered = function()
        if dialog:getIsVisible() then
            dialog.submitFromKeyboard = true
            dialog:onClick(dialog.yes)
        end
    end
    dialog.entry.onOtherKey = function(_, key)
        if key == Keyboard.KEY_ESCAPE and dialog:getIsVisible() then dialog:onClick(dialog.no) end
    end
    dialog.entry:focus()
end)
