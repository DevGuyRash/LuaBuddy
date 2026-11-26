# In-game mod management

The interface is Rust code in `crates/luabuddy-host/src/ui/`. It drives the game's own Kahlua UI classes through JNI and adds no first-party Lua.

## Entry points

A **LUABUDDY** line joins the game's main menu and pause menu beside the game's own mod entries, with the game's hover fade and controller navigation. It shows a count, in the warning color, only while selected mods could not load in the current environment. **Options > Mods** keeps one **Open LuaBuddy** button. The window opens over the menu that opened it and gives the menu back when it closes.

## The window

**My mods** lists every installed mod identity and every enabled profile, grouped by section: needs attention, running, waiting for a server or ready to load, next launch, off, loaded by the game or server, and folders the game can't read. The last two start folded. Section titles carry the state, so a row's second line repeats it only when the row differs, and otherwise shows version and author. Mods are read through the game's metadata parser a few at a time per frame, so opening takes milliseconds however many mods are installed.

Selecting a mod shows a page with its state, the reason for it and what resolves it, the error for a failure, what late loading can't activate, its requirements and their states, Workshop state, folder, and its description with Workshop BBCode and the game's rich-text commands removed. The page is drawn directly rather than as rich text, so upstream text cannot inject formatting.

Actions come from one pure model (`ui/model.rs`) of a mod's observed state and where the player is: the main menu, single player, or a server, whether startup has finished, whether Options edits are pending, and whether Steam is available. The action the mod's state calls for, such as **Try again** after a failure or **Reload** for a running mod with a cleanup contract, becomes the button under the page. **More...** and a right-click open every other applicable action in the game's own `ISContextMenu`, with submenus for related choices. An action that applies but can't run yet, such as loading while Options edits are pending, appears unavailable with its reason as a tooltip. A controller gets the same actions as one flat menu, because the game moves controllers between submenus through player slots that the main menu doesn't have.

`ISContextMenu` normally belongs to a player and does not exist in the main menu, so the window owns its menu and answers the two lookups that expect a player. **Workshop** appears only when the game's Steam client is initialized. **Activity** lists the journal: one row per action with its outcome, and every step when searched.

## Behavior worth knowing

Turning a mod on writes its profile, from the bundled template when there is one, together with the profiles of the mods it needs, all or none. Turning a mod off that enabled mods need offers to turn those off with it. A profile's `enabled` line is replaced rather than appended, so toggling does not grow the file.

Pending native Options edits block selection changes and loading, because loading rebuilds the Mods page of Options. That rebuild removes only records belonging to the old page and restores its tab position, ID and scroll.

Escape closes an open menu, then leaves the search box, then closes the window. Arrow keys move through the list. On a controller, A runs the main action, X opens the menu, Y opens **Tools**, B closes, and the bumpers switch tabs.

## Verification scope

The rebuilt window was exercised with the real game, 42.22.2 on Linux, in a disposable cache with 190 installed mods, at the main menu and joined to a loopback multiplayer server. Checks covered opening and closing from the main and pause menus, filtering, folding, selection, the context menu and submenus, turning mods on and off with the profile file contents checked, loading on a server, retrying a fixed syntax error without reconnecting, reload of a cooperating fixture, a reload refused while a dependent runs, an unreadable profile beside healthy ones, and the Activity view. Workshop browsing and delivery ran only in their Steam-unavailable state; no account action was taken. A physical controller was not available.
