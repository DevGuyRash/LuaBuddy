package luabuddy;

import java.io.IOException;
import java.io.InputStreamReader;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.function.Function;

import se.krka.kahlua.j2se.KahluaTableImpl;
import se.krka.kahlua.luaj.compiler.LuaCompiler;
import se.krka.kahlua.vm.KahluaTable;
import zombie.Lua.LuaManager;
import zombie.ZomboidFileSystem;
import zombie.core.Language;
import zombie.core.Translator;
import zombie.gameStates.ChooseGameInfo;
import zombie.network.GameClient;
import zombie.network.GameServer;

public final class Bootstrap {
    private record Mod(ModProfile profile, ChooseGameInfo.Mod info, List<String> required, Set<String> after) {}

    private Bootstrap() {}

    public static void load() {
        if (GameServer.server || !GameClient.client || LuaManager.env == null) return;
        if (LuaManager.env.rawget("LuaBuddy") != null) return;
        KahluaTable state = table();
        state.rawset("version", "1.0.0");
        KahluaTable statuses = table();
        state.rawset("mods", statuses);
        LuaManager.env.rawset("LuaBuddy", state);
        try {
            String directory = System.getProperty("luabuddy.config");
            if (directory == null) throw new IOException("Missing -Dluabuddy.config=<profile directory>");
            Set<String> active = new LinkedHashSet<>(ZomboidFileSystem.instance.getModIDs());
            Map<String, Mod> mods = new LinkedHashMap<>();
            for (ModProfile profile : ModProfile.read(Path.of(directory))) {
                if (active.contains(profile.id())) {
                    statuses.rawset(profile.id(), "server-loaded");
                    System.out.println("[LuaBuddy] " + profile.id() + " already loaded by the server; skipping.");
                    continue;
                }
                try {
                    ChooseGameInfo.Mod info = profile.resolve();
                    List<String> required = new ArrayList<>(orEmpty(info.getRequire()));
                    required.addAll(profile.list("requires"));
                    Set<String> after = new LinkedHashSet<>(required);
                    after.addAll(orEmpty(info.getLoadAfter()));
                    after.addAll(profile.list("after"));
                    mods.put(profile.id(), new Mod(profile, info, required, after));
                } catch (IOException | RuntimeException e) {
                    failed(statuses, profile.id(), e);
                }
            }
            for (Mod mod : mods.values()) {
                Set<String> before = new LinkedHashSet<>(orEmpty(mod.info().getLoadBefore()));
                before.addAll(mod.profile().list("before"));
                for (String id : before) {
                    if (mods.containsKey(id)) mods.get(id).after().add(mod.profile().id());
                }
            }
            KahluaTable payload = table();
            Set<String> remaining = new LinkedHashSet<>(mods.keySet());
            Set<String> available = new LinkedHashSet<>(active);
            while (!remaining.isEmpty()) {
                boolean progress = false;
                for (String id : List.copyOf(remaining)) {
                    Mod mod = mods.get(id);
                    if (mod.after().stream().anyMatch(remaining::contains)) continue;
                    remaining.remove(id);
                    progress = true;
                    try {
                        List<String> missing = mod.required().stream().filter(dep -> !available.contains(dep)).toList();
                        if (!missing.isEmpty()) throw new IOException("Enable required mod profiles or server mods: " + missing);
                        KahluaTable entry = table();
                        entry.rawset("id", id);
                        entry.rawset("sources", strings(mod.profile().sources(ModProfile.roots(mod.info()), active)));
                        entry.rawset("preload", array(mod.profile().list("preload")));
                        entry.rawset("required", array(mod.required()));
                        entry.rawset("plugin", mod.profile().plugin());
                        loadTranslations(ModProfile.roots(mod.info()));
                        payload.rawset(payload.size() + 1, entry);
                        available.add(id);
                    } catch (IOException | RuntimeException e) {
                        failed(statuses, id, e);
                    }
                }
                if (!progress) {
                    for (String id : remaining) failed(statuses, id, new IOException("Cyclic mod order: " + remaining));
                    break;
                }
            }
            KahluaTable activeTable = table();
            for (String id : active) activeTable.rawset(id, true);
            if (!payload.isEmpty()) runLoader(payload, activeTable, state);
        } catch (IOException | RuntimeException e) {
            state.rawset("error", e.toString());
            System.err.println("[LuaBuddy] Cannot initialize: " + e);
        }
    }

    private static void failed(KahluaTable statuses, String id, Exception error) {
        statuses.rawset(id, "failed: " + error.getMessage());
        System.err.println("[LuaBuddy] " + id + " failed; no retry in this Lua environment: " + error);
    }

    private static List<String> orEmpty(List<String> values) {
        return values == null ? List.of() : values;
    }

    static void loadTranslations(List<Path> roots) throws IOException {
        Map<String, Map<String, String>> merged = new LinkedHashMap<>();
        try {
            Method read = Translator.class.getDeclaredMethod("tryFillMapFromFile", String.class,
                    String.class, Map.class, Language.class, Function.class);
            Method format = Translator.class.getDeclaredMethod("formatFixer", String.class);
            read.setAccessible(true);
            format.setAccessible(true);
            Function<String, String> fixer = value -> (String) invoke(format, value);
            Translator.forLanguageStack(language -> {
                for (String section : Translator.BY_NAME.keySet()) {
                    Map<String, String> values = merged.computeIfAbsent(section, ignored -> new LinkedHashMap<>());
                    for (Path root : roots) invoke(read, root.toString(), section, values, language, fixer);
                }
            });
        } catch (ReflectiveOperationException e) {
            throw new IOException("Unsupported game translation API; tested with Build 42.21", e);
        }
        for (var section : merged.entrySet()) Translator.BY_NAME.get(section.getKey()).putAll(section.getValue());
    }

    private static Object invoke(Method method, Object... args) {
        try {
            return method.invoke(null, args);
        } catch (IllegalAccessException | InvocationTargetException e) {
            throw new IllegalStateException("Calling the game's translation parser", e);
        }
    }

    private static void runLoader(KahluaTable mods, KahluaTable active, KahluaTable state) throws IOException {
        try (var stream = Bootstrap.class.getResourceAsStream("/bootstrap.lua")) {
            if (stream == null) throw new IOException("Missing bootstrap.lua in LuaBuddy JAR");
            var chunk = LuaCompiler.loadis(new InputStreamReader(stream, StandardCharsets.UTF_8), "LuaBuddy/bootstrap.lua", LuaManager.env);
            Object[] result = LuaManager.caller.pcall(LuaManager.thread, chunk, new Object[]{mods, active, state});
            if (result.length == 0 || !Boolean.TRUE.equals(result[0])) {
                throw new IOException("Lua bootstrap failed: " + (result.length > 1 ? result[1] : "no result"));
            }
        }
    }

    static KahluaTable array(List<String> values) {
        KahluaTable table = table();
        for (int i = 0; i < values.size(); i++) table.rawset(i + 1, values.get(i));
        return table;
    }

    static KahluaTable strings(Map<String, String> values) {
        KahluaTable table = table();
        values.forEach(table::rawset);
        return table;
    }

    static KahluaTable table() {
        return new KahluaTableImpl(new LinkedHashMap<>());
    }
}
