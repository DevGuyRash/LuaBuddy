package luabuddy;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;

import org.json.JSONObject;
import se.krka.kahlua.luaj.compiler.LuaCompiler;
import se.krka.kahlua.vm.KahluaTable;
import se.krka.kahlua.vm.LuaClosure;
import zombie.Lua.LuaManager;
import zombie.Lua.LuaEventManager;
import zombie.ZomboidFileSystem;
import zombie.core.Languages;
import zombie.core.Translator;
import zombie.network.GameClient;
import zombie.network.GameServer;

public final class EngineCheck {
    private static Path root;

    private EngineCheck() {}

    private static void write(String relative, String value) throws Exception {
        Path path = root.resolve(relative);
        Files.createDirectories(path.getParent());
        Files.writeString(path, value, StandardCharsets.UTF_8);
    }

    private static void lua(String source) throws Exception {
        var chunk = LuaCompiler.loadstring(source, "LuaBuddy/test", LuaManager.env);
        Object[] result = LuaManager.caller.pcall(LuaManager.thread, chunk, new Object[]{});
        if (!Boolean.TRUE.equals(result[0])) throw new AssertionError(result[1]);
    }

    private static void fresh() throws Exception {
        LuaManager.init();
        lua("require = function(name) assert(name == 'native-fixture'); return 'native' end");
    }

    private static JSONObject report() throws Exception {
        return new JSONObject(Files.readString(root.resolve("cache/LuaBuddy/diagnostics.json")));
    }

    public static void main(String[] args) throws Exception {
        Path repo = Path.of(args[0]);
        root = Path.of(args[1]);
        zombie.core.random.RandStandard.INSTANCE.init();
        zombie.core.random.RandLua.INSTANCE.init();
        ZomboidFileSystem.instance.setCacheDir(root.resolve("cache").toString());
        ZomboidFileSystem.instance.init();
        Languages.instance.init();
        write("mods/Library/42/mod.info", "name=Library\nid=Library\nversionMin=42.0\n");
        write("mods/Library/42/media/lua/shared/Fixture/Library.lua", "return { number = 41 }");
        write("mods/Consumer/42/mod.info", "name=Consumer\nid=Consumer\nrequire=Library\nversionMin=42.0\n");
        write("mods/Consumer/common/media/lua/shared/Fixture/Overlay.lua", "error('common must be replaced')");
        write("mods/Consumer/42/media/lua/shared/Fixture/Overlay.lua", "return 42");
        write("mods/Consumer/42/media/lua/shared/Fixture/Phase.lua", "phaseCount = (phaseCount or 0) + 1; return 'shared'");
        write("mods/Consumer/42/media/lua/shared/Fixture/Upper.LUA", "return 73");
        write("mods/Consumer/42/media/lua/client/Fixture/Phase.lua", "phaseCount = phaseCount + 1; return 'client'");
        write("mods/Consumer/42/media/lua/client/Fixture/Consumer.lua",
                "assert(libraryReady); assert(require('Fixture/Library').number == 41); "
                + "assert(require('native-fixture') == 'native'); "
                + "assert(require('Fixture/Upper') == 73 and require('FIXTURE/UPPER.LUA') == 73); "
                + "function lateImport() return require('Fixture/Overlay.lua') end; "
                + "function makeNested() return function() return 9 end end; nestedCallback = makeNested()");
        write("config/a-consumer.properties", "id=Consumer\npath=../mods/Consumer\n");
        ZomboidFileSystem.instance.setModIdToDir("Library", root.resolve("mods/Library").toString());
        write("config/z-library.properties", "id=Library\nplugin=library.lua\n");
        write("config/library.lua", "return { after = function() libraryReady = true end }");
        System.setProperty("luabuddy.config", root.resolve("config").toString());
        fresh();
        GameServer.server = true;
        Bootstrap.load();
        if (LuaManager.env.rawget("LuaBuddy") != null) throw new AssertionError("Dedicated server isolation");
        GameServer.server = false;
        GameClient.client = false;
        Bootstrap.load();
        if (LuaManager.env.rawget("LuaBuddy") != null) throw new AssertionError("Single player isolation");
        GameClient.client = true;
        if (Boolean.getBoolean("luabuddy.verifyAgent")) {
            LuaEventManager.triggerEvent("OnGameStart");
            if (LuaManager.env.rawget("LuaBuddy") == null) throw new AssertionError("ZombieBuddy startup patch did not execute");
        } else {
            Bootstrap.load();
        }
        lua("assert(LuaBuddy.mods.Library == 'loaded'); assert(LuaBuddy.mods.Consumer == 'loaded'); "
                + "assert(lateImport() == 42); assert(phaseCount == 2); assert(require('Fixture/Phase') == 'shared')");
        String filename = root.resolve("mods/Consumer/42/media/lua/client/Fixture/Consumer.lua").toAbsolutePath().normalize().toString().replace('\\', '/');
        for (String name : List.of("lateImport", "makeNested", "nestedCallback")) {
            LuaClosure callback = (LuaClosure) LuaManager.env.rawget(name);
            if (!filename.equals(LuaManager.GlobalObject.getFilenameOfClosure(callback))
                    || !"Consumer.lua".equals(callback.prototype.file)) {
                throw new AssertionError("Native filename metadata for " + name);
            }
        }
        JSONObject successful = report();
        String session = successful.getString("session");
        if (successful.getJSONObject("summary").getInt("loaded") != 2
                || successful.getJSONObject("mods").getJSONObject("Consumer").getInt("sourceCount") != 5
                || !successful.getJSONObject("mods").getJSONObject("Consumer").getString("stage").equals("complete")) {
            throw new AssertionError("Successful initialization diagnostic summary");
        }
        KahluaTable state = (KahluaTable) LuaManager.env.rawget("LuaBuddy");
        Bootstrap.load();
        if (LuaManager.env.rawget("LuaBuddy") != state) throw new AssertionError("Initialize once per Lua environment");
        lua("assert(phaseCount == 2)");
        if (!session.equals(report().getString("session"))) throw new AssertionError("Same environment must retain its report");
        fresh();
        Bootstrap.load();
        lua("assert(phaseCount == 2); assert(LuaBuddy.mods.Consumer == 'loaded')");
        if (session.equals(report().getString("session"))) throw new AssertionError("Reconnect must replace the report with a new session");
        try (var files = Files.list(root.resolve("cache/LuaBuddy"))) {
            if (!files.map(path -> path.getFileName().toString()).toList().equals(List.of("diagnostics.json"))) {
                throw new AssertionError("One report and no stale temporary files");
            }
        }
        ZomboidFileSystem.instance.getModIDs().addAll(List.of("Library", "Consumer"));
        fresh();
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.Library == 'server-loaded'); assert(LuaBuddy.mods.Consumer == 'server-loaded'); assert(phaseCount == nil)");
        if (report().getJSONObject("summary").getInt("serverLoaded") != 2) throw new AssertionError("Server-loaded report");
        ZomboidFileSystem.instance.getModIDs().clear();
        write("config/library.lua", "return { after = function() error('dependency hook failed') end }");
        fresh();
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.Library:find('failed:', 1, true) == 1); "
                + "assert(LuaBuddy.mods.Consumer:find('failed:', 1, true) == 1); assert(lateImport == nil)");
        write("config/library.lua", "return {}");
        write("mods/Consumer/42/media/lua/shared/Fixture/Bad.lua", "invalid lua !");
        fresh();
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.Library == 'loaded'); assert(LuaBuddy.mods.Consumer:find('failed:',1,true) == 1); assert(phaseCount == nil)");
        System.out.println("PASS: native discovery, dependency hooks/failures, common/version overlay, shared/client phases, late require, compile isolation, SP/server isolation, reconnect, server-loaded skip");

        List<Path> roots = List.of(root.resolve("mods/Consumer/common"), root.resolve("mods/Consumer/42"));
        write("mods/Consumer/common/media/lua/shared/Translate/EN/UI.json", "{\"UI_LuaBuddyOverlay\":\"common\",\"UI_LuaBuddyFallback\":\"fallback %1\"}");
        write("mods/Consumer/42/media/lua/shared/Translate/EN/UI.json", "{\"UI_LuaBuddyOverlay\":\"version\"}");
        write("mods/Consumer/42/media/lua/shared/Translate/DE/UI.json", "{\"UI_LuaBuddyOverlay\":\"Version DE\"}");
        for (String[] test : new String[][]{{"EN", "version"}, {"DE", "Version DE"}, {"FI", "version"}}) {
            Translator.setLanguage(Languages.instance.getByName(test[0]));
            Translator.BY_NAME.get("UI").clear();
            Bootstrap.loadTranslations(roots);
            if (!test[1].equals(Translator.getText("UI_LuaBuddyOverlay"))
                    || !"fallback 100".equals(Translator.getText("UI_LuaBuddyFallback", 100))) {
                throw new AssertionError("Translation overlay, fallback, and placeholder: " + test[0]);
            }
        }
        System.out.println("PASS: native translations, version over common, German, English, fallback, %1 placeholder");

        Path profileDirectory = root.resolve("bundled-profiles");
        Files.createDirectories(profileDirectory);
        for (String id : List.of("AllInfo", "AutoAll")) {
            Files.copy(repo.resolve("profiles/" + id + ".properties"), profileDirectory.resolve(id + ".properties"));
        }
        ModProfile allInfo = ModProfile.read(profileDirectory).stream().filter(profile -> profile.id().equals("AllInfo")).findFirst().orElseThrow();
        write("allinfo/media/lua/client/AllInfo/SelfTest.lua", "error('not for startup')");
        write("allinfo/media/lua/client/AllInfo/character/Creation.lua", "return 1");
        write("allinfo/media/lua/client/AllInfo/character/InfoScreen.lua", "return 2");
        write("allinfo/media/lua/client/AllInfo/providers/MeleeWeapon.lua", "return 3");
        var filtered = allInfo.sources(List.of(root.resolve("allinfo")), java.util.Set.of("DetailedDescriptionsForOccupationsAndTraits"));
        if (!filtered.keySet().equals(java.util.Set.of("client/AllInfo/providers/MeleeWeapon.lua"))) {
            throw new AssertionError("AllInfo conflict exclusions preserve inventory information");
        }
        System.out.println("PASS: optional AllInfo profile excludes SelfTest and overlapping trait screens");

        write("blocked/42/mod.info", "name=Blocked\nid=Blocked\nversionMin=42.0\n");
        write("blocked/42/media/lua/client/Blocked.lua", "error('blocked mod must not execute')");
        write("missing-config/blocked.properties", "id=Blocked\npath=../blocked\nrequires=MissingDependency\n");
        System.setProperty("luabuddy.config", root.resolve("missing-config").toString());
        fresh();
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.Blocked:find('MissingDependency', 1, true)); assert(LuaBuddy.error == nil)");
        write("cycle-a/42/mod.info", "name=CycleA\nid=CycleA\nversionMin=42.0\n");
        write("cycle-a/42/media/lua/client/A.lua", "error('cyclic mod order must not execute')");
        write("cycle-b/42/mod.info", "name=CycleB\nid=CycleB\nversionMin=42.0\n");
        write("cycle-b/42/media/lua/client/B.lua", "error('cyclic mod order must not execute')");
        write("cycle-config/a.properties", "id=CycleA\npath=../cycle-a\nafter=CycleB\n");
        write("cycle-config/b.properties", "id=CycleB\npath=../cycle-b\nafter=CycleA\n");
        System.setProperty("luabuddy.config", root.resolve("cycle-config").toString());
        fresh();
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.CycleA:find('Cyclic mod order',1,true)); assert(LuaBuddy.mods.CycleB:find('Cyclic mod order',1,true)); assert(LuaBuddy.error == nil)");
        System.out.println("PASS: missing required profiles and cyclic mod order fail before executing mod sources");
        optionalInitializationCases();
        quickBuildFavoriteCases(repo);
        runtimeCases();
        diagnosticCases();
    }

    private static void quickBuildFavoriteCases(Path repo) throws Exception {
        var keep = new zombie.scripting.entity.components.crafting.CraftRecipe();
        keep.Load("LuaBuddyFavoriteKeep", "craftRecipe LuaBuddyFavoriteKeep { }");
        var changed = new zombie.scripting.entity.components.crafting.CraftRecipe();
        changed.Load("LuaBuddyFavoriteChanged", "craftRecipe LuaBuddyFavoriteChanged { }");
        var recipes = new java.util.ArrayList<>(List.of(keep, changed));
        for (String scenario : List.of("removed", "unchanged", "empty", "runtime")) {
            fresh();
            LuaEventManager.register(LuaManager.platform, LuaManager.env);
            var startup = LuaEventManager.AddEvent("OnGameStart");
            var tick = LuaEventManager.AddEvent("OnTick");
            for (var event : List.of(startup, tick)) {
                event.callbacks.clear();
                event.register(LuaManager.platform, (KahluaTable) LuaManager.env.rawget("Events"));
            }
            LuaManager.env.rawset("favoriteRecipes", recipes);
            lua("ScriptManager = {instance = {getAllCraftRecipes = function() return favoriteRecipes end}}; "
                    + "local key = BaseCraftingLogic.getFavouriteModDataString; "
                    + "keepKey = key('LuaBuddyFavoriteKeep'); changedKey = key('LuaBuddyFavoriteChanged'); "
                    + "orphanKey = key('LuaBuddyRemovedRecipe'); "
                    + "favoriteData = {[keepKey] = " + !scenario.equals("empty") + ", [changedKey] = " + !scenario.equals("empty") + ", [orphanKey] = true, foreign = 'old'}; "
                    + "settingsLoads = 0; transmissions = 0; foreignTicks = 0; "
                    + "PZAPI = {ModOptions = {load = function() settingsLoads = settingsLoads + 1 end}}; "
                    + "getPlayer = function() return {getModData = function() return favoriteData end, "
                    + "transmitModData = function() transmissions = transmissions + 1 end} end; "
                    + "Events.OnTick.Add(function() foreignTicks = foreignTicks + 1 end); "
                    + "Events.OnGameStart.Add(function() "
                    + "local function cleanup() "
                    + (scenario.equals("removed") ? "favoriteData[keepKey] = nil; " : "")
                    + "favoriteData[changedKey] = false; favoriteData[orphanKey] = nil; favoriteData.foreign = 'new'; "
                    + "Events.OnTick.Remove(cleanup) end; Events.OnTick.Add(cleanup) end)");
            var source = repo.resolve("profiles/QuickBuildMenu.lua");
            var chunk = LuaCompiler.loadstring(Files.readString(source), source.toString(), LuaManager.env);
            Object[] result = LuaManager.caller.pcall(LuaManager.thread, chunk, new Object[]{});
            if (!Boolean.TRUE.equals(result[0])) throw new AssertionError(result[1]);
            LuaManager.env.rawset("quickBuildHook", result[1]);
            lua("quickBuildHook.after({runtime = " + scenario.equals("runtime") + "}); assert(settingsLoads == 1)");
            if (scenario.equals("runtime")) {
                if (startup.callbacks.size() != 1 || tick.callbacks.size() != 1) {
                    throw new AssertionError("Runtime options restore must not register startup protection");
                }
                lua("assert(transmissions == 0 and favoriteData[keepKey] == true)");
                continue;
            }
            startup.trigger(LuaManager.env, LuaManager.caller, new Object[]{});
            tick.trigger(LuaManager.env, LuaManager.caller, new Object[]{});
            lua("assert(favoriteData[keepKey] == " + !scenario.equals("empty") + ", 'Valid favorite: " + scenario + "'); "
                    + "assert(favoriteData[changedKey] == false and favoriteData[orphanKey] == nil); "
                    + "assert(favoriteData.foreign == 'new' and foreignTicks == 1); "
                    + "assert(transmissions == " + (scenario.equals("removed") ? 1 : 0) + ")");
            if (tick.callbacks.size() != 1) throw new AssertionError("Temporary favorite callbacks must drain");
            tick.trigger(LuaManager.env, LuaManager.caller, new Object[]{});
            lua("assert(foreignTicks == 2); assert(transmissions == " + (scenario.equals("removed") ? 1 : 0) + ")");
        }
        System.out.println("PASS: Quick Build native favorite keys, startup ordering/self-removal, false/orphan isolation, no-change sends, runtime callback isolation");
    }

    private static void optionalInitializationCases() throws Exception {
        write("replay/42/mod.info", "name=Replay\nid=Replay\nversionMin=42.0\n");
        write("foreign/42/mod.info", "name=Foreign\nid=Foreign\nversionMin=42.0\n");
        write("replay/common/media/ui/replay.png", "common");
        write("replay/42/media/ui/replay.png", "version");
        write("replay/42/media/lua/server/Server.lua", "error('server Lua must not run')");
        write("foreign/42/media/lua/shared/Foreign.lua", "Events.OnLoad.Add(function() foreignRuns = (foreignRuns or 0) + 1 end)");
        write("replay/42/media/lua/shared/A.lua", "require('Foreign'); require('Nested'); "
                + "local callback = function() assert(clientReady); loadRuns = (loadRuns or 0) + 1 end; "
                + "Events.OnLoad.Add(callback); Events.OnLoad.Add(callback); "
                + "local cancelled = function() error('removed callback must not replay') end; "
                + "Events.OnLoad.Add(cancelled); Events.OnLoad.Remove(cancelled); "
                + "Events.OnGameBoot.Add(function() bootRuns = (bootRuns or 0) + 1 end)");
        write("replay/42/media/lua/shared/Nested.lua", "Events.OnLoad.Add(function() nestedRuns = (nestedRuns or 0) + 1 end)");
        write("replay/42/media/lua/client/Ready.lua", "assert(loadRuns == nil); clientReady = true");
        write("replay-config/a.properties", "id=Replay\npath=../replay\nmount=true\nreplay=OnGameBoot;OnLoad\nplugin=ready.lua\n");
        write("replay-config/z.properties", "id=Foreign\npath=../foreign\n");
        write("replay-config/ready.lua", "return { after = function() assert(bootRuns == 1 and loadRuns == 2 and nestedRuns == 1 and foreignRuns == nil) end }");
        System.setProperty("luabuddy.config", root.resolve("replay-config").toString());
        fresh();
        LuaEventManager.register(LuaManager.platform, LuaManager.env);
        for (String name : List.of("OnGameBoot", "OnLoad")) {
            var event = LuaEventManager.AddEvent(name);
            event.callbacks.clear();
            event.register(LuaManager.platform, (KahluaTable) LuaManager.env.rawget("Events"));
        }
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.Replay == 'loaded' and LuaBuddy.mods.Foreign == 'loaded', tostring(LuaBuddy.mods.Replay) .. '; ' .. tostring(LuaBuddy.mods.Foreign)); "
                + "assert(foreignRuns == nil); assert(bootRuns == 1 and loadRuns == 2 and nestedRuns == 1)");
        String mounted = ZomboidFileSystem.instance.getString("media/ui/replay.png");
        if (!Files.readString(Path.of(mounted)).equals("version") || !ZomboidFileSystem.instance.getModIDs().isEmpty()) {
            throw new AssertionError("Native mount uses the selected version without activating a mod");
        }
        LuaEventManager.triggerEvent("OnLoad");
        lua("assert(foreignRuns == 1 and loadRuns == 4 and nestedRuns == 2)");
        write("replay/42/media/lua/shared/A.lua", "Events.OnLoad.Add(function() error('replay fixture failure') end)");
        write("replay-config/ready.lua", "return {}");
        fresh();
        LuaEventManager.register(LuaManager.platform, LuaManager.env);
        for (String name : List.of("OnGameBoot", "OnLoad")) {
            var event = LuaEventManager.AddEvent(name);
            event.callbacks.clear();
            event.register(LuaManager.platform, (KahluaTable) LuaManager.env.rawget("Events"));
        }
        Bootstrap.load();
        JSONObject failed = report().getJSONObject("mods").getJSONObject("Replay");
        if (!failed.getString("stage").equals("replay-OnLoad") || !failed.getString("source").endsWith("A.lua")
                || !failed.getString("error").contains("replay fixture failure")) {
            throw new AssertionError("Replay failures retain native callback source attribution");
        }
        lua("assert(LuaBuddy.mods.Foreign == 'loaded')");
        for (String invalid : List.of("mount=yes", "replay=OnTick")) {
            write("invalid-optional/profile.properties", "id=Invalid\n" + invalid + "\n");
            try {
                ModProfile.read(root.resolve("invalid-optional"));
                throw new AssertionError("Invalid optional initialization setting accepted: " + invalid);
            } catch (java.io.IOException expected) {
                if (!expected.getMessage().contains("invalid-optional")) throw expected;
            }
        }
        write("flat/mod.info", "name=Flat\nid=Flat\nversionMin=42.0\n");
        write("flat/media/lua/shared/Flat.lua", "flatLoaded = true");
        write("flat-config/flat.properties", "id=Flat\npath=../flat\nworkshop=123\n");
        System.setProperty("luabuddy.config", root.resolve("flat-config").toString());
        fresh();
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.Flat == 'loaded'); assert(flatLoaded)");
        if (!report().getJSONObject("mods").getJSONObject("Flat").getString("versionRoot").equals(root.resolve("flat").toRealPath().toString())) {
            throw new AssertionError("Flat Workshop metadata and Lua must use the upstream root in place");
        }
        write("unavailable-workshop/profile.properties", "id=NotInstalledWorkshop\nworkshop=123\n");
        System.setProperty("luabuddy.config", root.resolve("unavailable-workshop").toString());
        fresh();
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.NotInstalledWorkshop:find('Steam is unavailable', 1, true))");
        write("retry-flat/mod.info", "name=Retry\nid=NotInstalledWorkshop\nversionMin=42.0\n");
        write("retry-flat/media/lua/client/Retry.lua", "workshopRetryLoaded = true");
        write("unavailable-workshop/profile.properties", "id=NotInstalledWorkshop\nworkshop=123\npath=../retry-flat\n");
        fresh();
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.NotInstalledWorkshop == 'loaded'); assert(workshopRetryLoaded)");
        if (zombie.gameStates.ChooseGameInfo.getModDetails("NotInstalledWorkshop") == null) {
            throw new AssertionError("A failed Workshop lookup must not poison subsequent native metadata discovery");
        }
        write("mapped/42/mod.info", "name=MappedNative\nid=MappedNative\nversionMin=42.0\n");
        write("mapped/42/media/lua/client/Mapped.lua", "mappedNativeLoaded = true");
        write("mapped-config/profile.properties", "id=MappedNative\nworkshop=123\n");
        ZomboidFileSystem.instance.setModIdToDir("MappedNative", root.resolve("mapped").toString());
        System.setProperty("luabuddy.config", root.resolve("mapped-config").toString());
        fresh();
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.MappedNative == 'loaded', tostring(LuaBuddy.mods.MappedNative)); assert(mappedNativeLoaded)");
        System.out.println("PASS: Workshop profiles honor native registered directories outside enumerated mod folders without Steam");
        if (!System.getProperty("os.name").startsWith("Windows")) {
            write("alias-source/42/mod.info", "name=Alias\nid=Alias\nversionMin=42.0\n");
            write("alias-source/42/media/lua/client/Alias.lua", "aliasLoaded = true");
            write("alias-source/42/media/ui/alias.png", "asset");
            Path alias = Files.createSymbolicLink(root.resolve("alias-mod"), root.resolve("alias-source"));
            var nativeInfo = zombie.gameStates.ChooseGameInfo.readModInfo(alias.toString());
            write("alias-config/profile.properties", "id=Alias\npath=../alias-mod\nmount=true\n");
            System.setProperty("luabuddy.config", root.resolve("alias-config").toString());
            fresh();
            Bootstrap.load();
            lua("assert(LuaBuddy.mods.Alias == 'loaded'); assert(aliasLoaded)");
            if (!report().getJSONObject("mods").getJSONObject("Alias").getString("versionRoot").equals(nativeInfo.getVersionDir())
                    || !ZomboidFileSystem.instance.getString("media/ui/alias.png").equals(alias.resolve("42/media/ui/alias.png").toString())) {
                throw new AssertionError("Explicit paths must preserve native symlink aliases for image prefix validation");
            }
            System.out.println("PASS: explicit symlink paths retain native directory and asset aliases");
        }
        System.out.println("PASS: opt-in flat Workshop roots use native metadata, sources stay in place, no-Steam lookup fails safely");
        System.out.println("PASS: opt-in native mounting, common/version assets, no activation/server Lua, isolated callback replay, native registration/removal, nested and foreign imports, replay failure attribution");
    }

    private static void runtimeStart(String directory) throws Exception {
        Files.createDirectories(root.resolve(directory));
        System.setProperty("luabuddy.config", root.resolve(directory).toString());
        fresh();
        LuaEventManager.register(LuaManager.platform, LuaManager.env);
        for (String name : List.of("OnGameStart", "OnTick")) {
            var event = LuaEventManager.AddEvent(name);
            event.callbacks.clear();
            event.register(LuaManager.platform, (KahluaTable) LuaManager.env.rawget("Events"));
        }
        lua("Events.OnGameStart.Add(function() foreignStarts = (foreignStarts or 0) + 1; "
                + "if LuaBuddy then startupRequest = LuaBuddy.refresh() end end); "
                + "Events.OnTick.Add(function() foreignTicks = (foreignTicks or 0) + 1 end)");
        if (Boolean.getBoolean("luabuddy.verifyAgent")) LuaEventManager.triggerEvent("OnGameStart");
        else {
            Bootstrap.load();
            LuaEventManager.triggerEvent("OnGameStart");
            Bootstrap.started(true);
        }
        lua("assert(LuaBuddy.ready); assert(foreignStarts == 1); assert(startupRequest == false); "
                + "savedRequire = require; savedSession = LuaBuddy.session");
    }

    private static void runtimeMod(String folder, String id, String metadata, String relative, String source) throws Exception {
        write(folder + "/42/mod.info", "name=" + id + "\nid=" + id + "\nversionMin=42.0\n" + metadata);
        write(folder + "/42/media/lua/" + relative, source);
    }

    private static String leafSource(int version) {
        return "leafRuns = (leafRuns or 0) + 1; local m = {version = " + version + ", ticks = 0}; RuntimeLeaf = m; "
                + "function m.tick() m.ticks = m.ticks + 1 end; Events.OnTick.Add(m.tick); return m";
    }

    private static void runtimeCases() throws Exception {
        runtimeMod("runtime-library", "RuntimeLibrary", "", "shared/Runtime/Library.lua",
                "libraryRuns = (libraryRuns or 0) + 1; "
                + "Events.OnGameStart.Add(function() libraryStarts = (libraryStarts or 0) + 1 end); return {value = 11}");
        write("runtime-library/42/media/lua/shared/Runtime/False.lua", "falseRuns = (falseRuns or 0) + 1; return false");
        write("runtime-library/42/media/lua/shared/Runtime/Nil.lua", "nilRuns = (nilRuns or 0) + 1");
        write("runtime-library/42/media/lua/client/Runtime/ClientOnly.lua", "return 'old-client'");
        write("runtime-config/library.properties", "id=RuntimeLibrary\npath=../runtime-library\nreplay=OnGameStart\n");
        runtimeStart("runtime-config");
        lua("assert(libraryRuns == 1 and libraryStarts == 1); libraryObject = require('Runtime/Library'); "
                + "assert(require('Runtime/False') == false and require('Runtime/Nil') == nil); "
                + "assert(LuaBuddy.refresh()); assert(LuaBuddy.refresh()); assert(require == savedRequire); "
                + "assert(require('Runtime/Library') == libraryObject); assert(libraryRuns == 1 and falseRuns == 1 and nilRuns == 1)");
        runtimeMod("runtime-child", "RuntimeChild", "require=RuntimeLibrary\n", "client/Runtime/Child.lua",
                "assert(require('Runtime/Library') == libraryObject); childRuns = (childRuns or 0) + 1; "
                + "Events.OnGameStart.Add(function() assert(peerReady); childStarts = (childStarts or 0) + 1 end); return 29");
        runtimeMod("runtime-peer", "RuntimePeer", "", "client/Runtime/Peer.lua", "peerRuns = (peerRuns or 0) + 1");
        write("runtime-config/child.properties", "id=RuntimeChild\npath=../runtime-child\nreplay=OnGameStart\n");
        write("runtime-config/peer.properties", "id=RuntimePeer\npath=../runtime-peer\nafter=RuntimeChild\nplugin=peer.lua\n");
        write("runtime-config/peer.lua", "return {after = function(context) assert(context.runtime and context.operation == 'refresh'); "
                + "local ok, reason = context.state.refresh(); assert(not ok and reason:find('already running', 1, true)); peerReady = true end}");
        lua("assert(LuaBuddy.refresh()); assert(savedRequire('Runtime/Child') == 29); assert(childRuns == 1 and childStarts == 1); "
                + "assert(peerRuns == 1 and foreignStarts == 1 and libraryStarts == 1); assert(LuaBuddy.session == savedSession)");
        write("runtime-config/library.properties", "id=RuntimeLibrary\nenabled=false\n");
        lua("assert(LuaBuddy.refresh()); assert(LuaBuddy.mods.RuntimeLibrary == 'loaded'); assert(libraryRuns == 1)");
        Files.delete(root.resolve("runtime-config/library.properties"));
        lua("assert(LuaBuddy.refresh()); assert(require('Runtime/Library') == libraryObject)");

        runtimeMod("runtime-leaf", "RuntimeLeaf", "", "shared/Runtime/Leaf.lua", leafSource(1));
        write("runtime-leaf/42/media/lua/server/DoNotRun.lua", "error('runtime server source executed')");
        write("runtime-config/leaf.properties", "id=RuntimeLeaf\npath=../runtime-leaf\nmount=true\nplugin=leaf.lua\n");
        write("runtime-config/leaf.lua", "local hookTick; return {after = function() hookTick = function() hookTicks = (hookTicks or 0) + 1 end; "
                + "Events.OnTick.Add(hookTick) end, unload = function() if refuseCleanup then return false, 'busy' end; "
                + "Events.OnTick.Remove(hookTick); leafUnloads = (leafUnloads or 0) + 1; RuntimeLeaf = nil; return true end}");
        lua("assert(LuaBuddy.refresh()); oldLeaf = require('Runtime/Leaf'); assert(oldLeaf.version == 1); assert(leafRuns == 1)");
        LuaEventManager.triggerEvent("OnTick");
        lua("assert(oldLeaf.ticks == 1 and hookTicks == 1 and foreignTicks == 1)");
        write("runtime-leaf/42/media/lua/shared/Runtime/Leaf.lua", leafSource(2));
        lua("assert(LuaBuddy.reload('RuntimeLeaf')); assert(require == savedRequire); assert(require('Runtime/Leaf') == RuntimeLeaf); "
                + "assert(RuntimeLeaf ~= oldLeaf and RuntimeLeaf.version == 2); assert(leafUnloads == 1 and leafRuns == 2)");
        LuaEventManager.triggerEvent("OnTick");
        lua("assert(RuntimeLeaf.ticks == 1 and oldLeaf.ticks == 1); assert(hookTicks == 2 and foreignTicks == 2); "
                + "assert(libraryRuns == 1 and falseRuns == 1 and nilRuns == 1)");
        JSONObject reloaded = report();
        if (!reloaded.getString("operation").equals("reload") || !reloaded.getString("target").equals("RuntimeLeaf")
                || !reloaded.getJSONObject("mods").getJSONObject("RuntimeLibrary").getString("status").equals("loaded")) {
            throw new AssertionError("Runtime reports retain actual running generations and operation metadata");
        }
        lua("refuseCleanup = true; local old = RuntimeLeaf; local ok = LuaBuddy.reload('RuntimeLeaf'); "
                + "assert(not ok and RuntimeLeaf == old and require('Runtime/Leaf') == old); "
                + "assert(leafUnloads == 1 and LuaBuddy.mods.RuntimeLeaf == 'loaded'); refuseCleanup = false");
        String unmapped = ZomboidFileSystem.instance.getString("media/ui/runtime-unpublished.png");
        write("runtime-leaf/42/media/ui/runtime-unpublished.png", "must stay unpublished after compile rejection");
        write("runtime-leaf/42/media/lua/shared/Runtime/Leaf.lua", "return )");
        lua("local old = RuntimeLeaf; local ok = LuaBuddy.reload('RuntimeLeaf'); assert(not ok); "
                + "assert(RuntimeLeaf == old and require('Runtime/Leaf') == old and old.version == 2); "
                + "assert(leafUnloads == 1 and leafRuns == 2 and LuaBuddy.mods.RuntimeLeaf == 'loaded')");
        if (!ZomboidFileSystem.instance.getString("media/ui/runtime-unpublished.png").equals(unmapped)) {
            throw new AssertionError("Compile rejection must not publish candidate native mappings");
        }
        write("runtime-leaf/42/media/lua/shared/Runtime/Leaf.lua", leafSource(3));
        lua("assert(LuaBuddy.reload('RuntimeLeaf')); assert(RuntimeLeaf.version == 3 and leafRuns == 3 and leafUnloads == 2); "
                + "assert(LuaBuddy.error == nil and LuaBuddy.session == savedSession)");
        runtimeMod("runtime-consumer", "RuntimeConsumer", "require=RuntimeLeaf\n", "client/Runtime/Consumer.lua",
                "assert(require('Runtime/Leaf').version == 3); consumerRuns = (consumerRuns or 0) + 1");
        write("runtime-config/consumer.properties", "id=RuntimeConsumer\npath=../runtime-consumer\n");
        write("runtime-config/library.properties", "id=RuntimeLibrary\npath=../runtime-library\nreplay=OnGameStart\n");
        lua("assert(LuaBuddy.refresh()); local old = RuntimeLeaf; assert(not LuaBuddy.reload('RuntimeLeaf')); "
                + "assert(RuntimeLeaf == old and leafUnloads == 2 and consumerRuns == 1); "
                + "assert(not LuaBuddy.reload('RuntimeLibrary')); assert(require('Runtime/Library') == libraryObject)");
        if (Boolean.getBoolean("luabuddy.verifyAgent")) {
            LuaEventManager.triggerEvent("OnGameStart");
            lua("assert(startupRequest == false and LuaBuddy.ready); assert(libraryRuns == 1)");
        }
        Object oldRefresh = LuaManager.env.rawget("LuaBuddy") instanceof KahluaTable state ? state.rawget("refresh") : null;
        LuaClosure foreignProbe = LuaCompiler.loadstring("return LuaBuddy.refresh()", "runtime-thread", LuaManager.env);
        java.util.concurrent.atomic.AtomicReference<Object[]> result = new java.util.concurrent.atomic.AtomicReference<>();
        Thread wrongThread = new Thread(() -> result.set(LuaManager.caller.pcall(LuaManager.thread, foreignProbe, new Object[]{})));
        wrongThread.setDaemon(true);
        wrongThread.start();
        wrongThread.join(5000);
        if (wrongThread.isAlive() || result.get() == null || !Boolean.TRUE.equals(result.get()[0])
                || !Boolean.FALSE.equals(result.get()[1]) || !String.valueOf(result.get()[2]).contains("game thread")) {
            throw new AssertionError("Foreign-thread API requests must refuse before mutation");
        }
        runtimeStart("runtime-empty");
        LuaManager.env.rawset("oldRefresh", oldRefresh);
        lua("assert(not oldRefresh()); assert(require == savedRequire); assert(LuaBuddy.refresh())");
        if (zombie.gameStates.ChooseGameInfo.getModDetails("RuntimeInstalledLater") != null) throw new AssertionError("New mod fixture already exists");
        runtimeMod("cache/mods/RuntimeInstalledLater", "RuntimeInstalledLater", "", "client/Runtime/InstalledLater.lua", "installedLater = true");
        write("runtime-empty/new.properties", "id=RuntimeInstalledLater\n");
        lua("assert(LuaBuddy.refresh()); assert(installedLater and LuaBuddy.mods.RuntimeInstalledLater == 'loaded')");

        runtimeRejectionCases();
        System.out.println("PASS: runtime append/no-op identity, runtime-only scoped startup, cooperative reload/callback cleanup, preflight preservation, native discovery refresh, owner/session/startup guards, running selection and diagnostics");
    }

    private static void runtimeRejectionCases() throws Exception {
        for (String rule : List.of("before", "reverse", "path", "alias")) {
            String directory = "runtime-reject-" + rule;
            write(directory + "/library.properties", "id=RuntimeLibrary\npath=../runtime-library\n" + (rule.equals("reverse") ? "after=RuntimeBlocked\n" : ""));
            runtimeStart(directory);
            String source = rule.equals("path") ? "shared/Runtime/Library.lua" : rule.equals("alias") ? "shared/Runtime/ClientOnly.lua" : "client/Runtime/Blocked.lua";
            runtimeMod("runtime-blocked", "RuntimeBlocked", "", source, "blockedRuntimeExecuted = true");
            write(directory + "/new.properties", "id=RuntimeBlocked\npath=../runtime-blocked\n" + (rule.equals("before") ? "before=RuntimeLibrary\n" : ""));
            lua("assert(not LuaBuddy.refresh()); assert(blockedRuntimeExecuted == nil); assert(libraryRuns == 1); "
                    + "assert(LuaBuddy.mods.RuntimeLibrary == 'loaded'); assert(LuaBuddy.mods.RuntimeBlocked:find('failed:', 1, true) == 1)");
            Files.delete(root.resolve("runtime-blocked/42/media/lua/" + source));
        }

        for (String cleanup : List.of("throw", "nil", "activation")) {
            String directory = "runtime-cleanup-" + cleanup;
            write("runtime-leaf/42/media/lua/shared/Runtime/Leaf.lua", leafSource(1));
            write(directory + "/leaf.properties", "id=RuntimeLeaf\npath=../runtime-leaf\nplugin=leaf.lua\n");
            String behavior = cleanup.equals("throw") ? "error('cleanup failed')" : cleanup.equals("nil") ? "return nil" : "return true";
            write(directory + "/leaf.lua", "return {unload = function(context) assert(context.runtime == false and context.operation == 'startup'); "
                    + "cleanupCalls = (cleanupCalls or 0) + 1; " + behavior + " end}");
            runtimeStart(directory);
            if (cleanup.equals("activation")) write("runtime-leaf/42/media/lua/shared/Runtime/Leaf.lua", "leafRuns = leafRuns + 1; error('activation failed')");
            lua("assert(not LuaBuddy.reload('RuntimeLeaf')); assert(cleanupCalls == 1); "
                    + "assert(LuaBuddy.mods.RuntimeLeaf:find('failed:', 1, true) == 1); assert(not pcall(require, 'Runtime/Leaf')); "
                    + "local count = leafRuns; assert(not LuaBuddy.reload('RuntimeLeaf')); LuaBuddy.refresh(); "
                    + "assert(leafRuns == count and cleanupCalls == 1)");
        }

        runtimeStart("runtime-init-failure");
        runtimeMod("runtime-init-base", "RuntimeInitBase", "", "client/Runtime/InitBase.lua",
                "initBaseRuns = (initBaseRuns or 0) + 1; Events.OnGameStart.Add(function() error('runtime startup failed') end)");
        runtimeMod("runtime-init-dependent", "RuntimeInitDependent", "require=RuntimeInitBase\n", "client/Runtime/InitDependent.lua", "initDependentRuns = (initDependentRuns or 0) + 1");
        runtimeMod("runtime-init-unrelated", "RuntimeInitUnrelated", "", "client/Runtime/InitUnrelated.lua", "initUnrelatedRuns = (initUnrelatedRuns or 0) + 1");
        write("runtime-init-failure/a.properties", "id=RuntimeInitBase\npath=../runtime-init-base\nreplay=OnGameStart\n");
        write("runtime-init-failure/b.properties", "id=RuntimeInitDependent\npath=../runtime-init-dependent\n");
        write("runtime-init-failure/z.properties", "id=RuntimeInitUnrelated\npath=../runtime-init-unrelated\n");
        lua("assert(not LuaBuddy.refresh()); assert(LuaBuddy.mods.RuntimeInitBase:find('failed:', 1, true) == 1); "
                + "assert(LuaBuddy.mods.RuntimeInitDependent:find('failed:', 1, true) == 1); "
                + "assert(LuaBuddy.mods.RuntimeInitUnrelated == 'loaded'); "
                + "assert(initBaseRuns == 1 and initDependentRuns == 1 and initUnrelatedRuns == 1); "
                + "assert(not LuaBuddy.refresh()); assert(initBaseRuns == 1 and initDependentRuns == 1)");
        System.out.println("PASS: runtime ordering/path/alias refusal, cleanup uncertainty/activation quarantine, dependency startup-failure propagation and unrelated continuation");
    }

    private static void diagnosticCases() throws Exception {
        System.clearProperty("luabuddy.config");
        fresh();
        Bootstrap.load();
        if (!report().getString("error").contains("luabuddy.config")
                || !report().getString("configDirectory").isEmpty()
                || !report().getString("traceback").contains("IOException")) {
            throw new AssertionError("Missing configuration diagnostics");
        }
        write("invalid-config/broken.properties", "id=Broken\npath=" + '\\' + "uZZZZ\n");
        System.setProperty("luabuddy.config", root.resolve("invalid-config").toString());
        fresh();
        Bootstrap.load();
        if (!report().getString("error").contains("broken.properties")
                || !report().getString("traceback").contains("IllegalArgumentException")) {
            throw new AssertionError("Malformed configuration must identify the profile and original cause");
        }
        Files.createDirectories(root.resolve("empty-config"));
        System.setProperty("luabuddy.config", root.resolve("empty-config").toString());
        fresh();
        Bootstrap.load();
        if (!report().getJSONObject("mods").isEmpty() || report().has("error")) throw new AssertionError("Empty configuration report");
        write("disabled-config/disabled.properties", "id=NotInstalled\nenabled=false\n");
        System.setProperty("luabuddy.config", root.resolve("disabled-config").toString());
        fresh();
        Bootstrap.load();
        if (report().getJSONObject("summary").getInt("disabled") != 1 || report().has("error")) {
            throw new AssertionError("Disabled profiles do not require installed files");
        }

        write("diagnostic-mod/42/mod.info", "name=DiagnosticFixture\nid=DiagnosticFixture\nversionMin=42.0\n");
        write("diagnostic-mod/42/media/lua/client/DiagnosticFixture.lua",
                "local function bottom() error('native diagnostic fixture') end; local function middle() bottom() end; middle()");
        write("diagnostic-config/fixture.properties", "id=DiagnosticFixture\npath=../diagnostic-mod\nplugin=hooks.lua\n");
        write("diagnostic-config/hooks.lua", "return {}");
        System.setProperty("luabuddy.config", root.resolve("diagnostic-config").toString());
        fresh();
        Bootstrap.load();
        JSONObject failed = report().getJSONObject("mods").getJSONObject("DiagnosticFixture");
        if (!failed.getString("status").equals("failed") || !failed.getString("stage").equals("client")
                || !failed.getString("source").endsWith("DiagnosticFixture.lua")
                || !failed.getString("error").contains("native diagnostic fixture")
                || !failed.getString("traceback").contains("DiagnosticFixture.lua")) {
            throw new AssertionError("Nested native runtime diagnostic trace: " + failed);
        }
        write("diagnostic-mod/42/media/lua/client/DiagnosticFixture.lua", "diagnosticFixtureLoaded = true");
        for (String hook : List.of("before", "after")) {
            write("diagnostic-config/hooks.lua", "return { " + hook + " = function() error('" + hook + " fixture') end }");
            fresh();
            Bootstrap.load();
            JSONObject detail = report().getJSONObject("mods").getJSONObject("DiagnosticFixture");
            if (!detail.getString("stage").equals(hook) || !detail.getString("source").endsWith("hooks.lua")
                    || !detail.getString("traceback").contains("hooks.lua")) {
                throw new AssertionError("Plugin hook diagnostic attribution: " + detail);
            }
        }
        write("diagnostic-config/hooks.lua", "return {}");
        Path blocked = root.resolve("cache/LuaBuddy");
        Path previous = root.resolve("previous-report");
        Files.move(blocked, previous);
        Files.writeString(blocked, "blocked report directory");
        fresh();
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.DiagnosticFixture == 'loaded'); assert(diagnosticFixtureLoaded); assert(LuaBuddy.reportError)");
        if (!Files.readString(blocked).equals("blocked report directory")) throw new AssertionError("Report failures must preserve unrelated data");
        Files.delete(blocked);
        Files.move(previous, blocked);
        System.setProperty("luabuddy.debug", "true");
        fresh();
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.DiagnosticFixture == 'loaded'); assert(LuaBuddy.debug)");
        if (!report().getBoolean("debug") || report().getJSONObject("summary").getInt("loaded") != 1) {
            throw new AssertionError("Debug report must reflect the current initialization");
        }
        System.clearProperty("luabuddy.debug");
        System.out.println("PASS: uppercase Lua modules, native nested closure filenames, complete/reconnect/disabled/empty/failure reports, Lua hook traces, write failure isolation, debug logging");
    }
}
