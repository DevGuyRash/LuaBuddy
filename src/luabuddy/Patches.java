package luabuddy;

import me.zed_0xff.zombie_buddy.Patch;

public final class Patches {
    private Patches() {}

    @Patch(className = "zombie.Lua.LuaEventManager", methodName = "triggerEvent", warmUp = true)
    public static class BeforeGameStart {
        @Patch.OnEnter
        public static void enter(@Patch.Argument(0) String event) {
            if ("OnGameStart".equals(event)) {
                Bootstrap.load();
                Bootstrap.started(false);
            }
        }

        @Patch.OnExit
        public static void exit(@Patch.Argument(0) String event) {
            if ("OnGameStart".equals(event)) Bootstrap.started(true);
        }
    }
}
