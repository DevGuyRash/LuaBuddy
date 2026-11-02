package luabuddy;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;

import se.krka.kahlua.luaj.compiler.LuaCompiler;
import se.krka.kahlua.vm.KahluaTable;
import zombie.Lua.LuaManager;
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
        write("mods/Consumer/42/media/lua/client/Fixture/Phase.lua", "phaseCount = phaseCount + 1; return 'client'");
        write("mods/Consumer/42/media/lua/client/Fixture/Consumer.lua",
                "assert(libraryReady); assert(require('Fixture/Library').number == 41); "
                + "assert(require('native-fixture') == 'native'); "
                + "function lateImport() return require('Fixture/Overlay.lua') end");
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
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.Library == 'loaded'); assert(LuaBuddy.mods.Consumer == 'loaded'); "
                + "assert(lateImport() == 42); assert(phaseCount == 2); assert(require('Fixture/Phase') == 'shared')");
        KahluaTable state = (KahluaTable) LuaManager.env.rawget("LuaBuddy");
        Bootstrap.load();
        if (LuaManager.env.rawget("LuaBuddy") != state) throw new AssertionError("Initialize once per Lua environment");
        lua("assert(phaseCount == 2)");
        fresh();
        Bootstrap.load();
        lua("assert(phaseCount == 2); assert(LuaBuddy.mods.Consumer == 'loaded')");
        ZomboidFileSystem.instance.getModIDs().addAll(List.of("Library", "Consumer"));
        fresh();
        Bootstrap.load();
        lua("assert(LuaBuddy.mods.Library == 'server-loaded'); assert(LuaBuddy.mods.Consumer == 'server-loaded'); assert(phaseCount == nil)");
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
    }
}
