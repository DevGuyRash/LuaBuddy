package luabuddy;

import java.io.IOException;
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
import se.krka.kahlua.vm.JavaFunction;
import se.krka.kahlua.vm.LuaClosure;
import se.krka.kahlua.vm.Prototype;
import zombie.Lua.LuaManager;
import zombie.Lua.LuaEventManager;
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
        KahluaTable statuses = table();
        state.rawset("mods", statuses);
        LuaManager.env.rawset("LuaBuddy", state);
        String directory = System.getProperty("luabuddy.config");
        Diagnostics diagnostics = new Diagnostics(state, directory);
        state.rawset("compile", (JavaFunction) (frame, count) -> {
            try {
                return frame.push(compile((String) frame.get(0), (String) frame.get(1)));
            } catch (IOException | RuntimeException error) {
                frame.push(null, error.toString());
                frame.push(Diagnostics.stackTrace(error));
                return 3;
            }
        });
        state.rawset("callbacks", (JavaFunction) (frame, count) -> {
            String event = (String) frame.get(0);
            KahluaTable filenames = (KahluaTable) frame.get(1);
            KahluaTable callbacks = table();
            for (LuaClosure callback : List.copyOf(LuaEventManager.AddEvent(event).callbacks)) {
                if (Boolean.TRUE.equals(filenames.rawget(callback.prototype.filename))) {
                    callbacks.rawset(callbacks.size() + 1, callback);
                }
            }
            return frame.push(callbacks);
        });
        try {
            if (directory == null) throw new IOException("Missing -Dluabuddy.config=<profile directory>");
            Set<String> active = new LinkedHashSet<>(ZomboidFileSystem.instance.getModIDs());
            Map<String, Mod> mods = new LinkedHashMap<>();
            for (ModProfile profile : ModProfile.read(Path.of(directory))) {
                KahluaTable detail = diagnostics.mod(profile);
                if (!profile.enabled()) {
                    statuses.rawset(profile.id(), "disabled");
                    detail.rawset("status", "disabled");
                    detail.rawset("stage", "disabled");
                    if (Boolean.TRUE.equals(state.rawget("debug"))) Diagnostics.log(profile.id() + " disabled by profile.");
                    continue;
                }
                if (active.contains(profile.id())) {
                    statuses.rawset(profile.id(), "server-loaded");
                    detail.rawset("status", "server-loaded");
                    detail.rawset("stage", "server-loaded");
                    Diagnostics.log(profile.id() + " already loaded by the server; skipping.");
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
                    diagnostics.failed(profile.id(), detail, e);
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
                        KahluaTable detail = (KahluaTable) ((KahluaTable) state.rawget("details")).rawget(id);
                        detail.rawset("stage", "dependencies");
                        List<String> missing = mod.required().stream().filter(dep -> !available.contains(dep)).toList();
                        if (!missing.isEmpty()) throw new IOException("Enable required mod profiles or server mods: " + missing);
                        KahluaTable entry = table();
                        entry.rawset("id", id);
                        List<Path> roots = ModProfile.roots(mod.info());
                        detail.rawset("commonRoot", mod.info().getCommonDir());
                        detail.rawset("versionRoot", mod.info().getVersionDir());
                        detail.rawset("requires", String.join(";", mod.required()));
                        detail.rawset("preload", String.join(";", mod.profile().list("preload")));
                        detail.rawset("replay", String.join(";", mod.profile().list("replay")));
                        detail.rawset("plugin", mod.profile().pluginFilename());
                        detail.rawset("stage", "source-scan");
                        var files = mod.profile().sources(roots, active);
                        KahluaTable sources = table();
                        KahluaTable filenames = table();
                        files.forEach((name, source) -> {
                            sources.rawset(name, source.text());
                            filenames.rawset(name, source.filename());
                        });
                        detail.rawset("sourceCount", files.size());
                        if (Boolean.TRUE.equals(state.rawget("debug"))) Diagnostics.log(id + " roots=" + roots + " sources=" + files.size());
                        entry.rawset("sources", sources);
                        entry.rawset("filenames", filenames);
                        entry.rawset("preload", array(mod.profile().list("preload")));
                        entry.rawset("replay", array(mod.profile().list("replay")));
                        entry.rawset("required", array(mod.required()));
                        detail.rawset("stage", "plugin-read");
                        entry.rawset("plugin", mod.profile().plugin());
                        entry.rawset("pluginFilename", mod.profile().pluginFilename());
                        detail.rawset("stage", "translations");
                        loadTranslations(roots);
                        if (Boolean.parseBoolean(mod.profile().options().getProperty("mount", "false").trim())) {
                            detail.rawset("stage", "file-mount");
                            ZomboidFileSystem.instance.setModIdToDir(id, mod.info().getDir());
                            ZomboidFileSystem.instance.loadMod(id);
                        }
                        detail.rawset("stage", "prepared");
                        payload.rawset(payload.size() + 1, entry);
                        available.add(id);
                    } catch (IOException | RuntimeException e) {
                        diagnostics.failed(id, (KahluaTable) ((KahluaTable) state.rawget("details")).rawget(id), e);
                    }
                }
                if (!progress) {
                    for (String id : remaining) {
                        KahluaTable detail = (KahluaTable) ((KahluaTable) state.rawget("details")).rawget(id);
                        detail.rawset("stage", "dependencies");
                        diagnostics.failed(id, detail, new IOException("Cyclic mod order: " + remaining));
                    }
                    break;
                }
            }
            KahluaTable activeTable = table();
            for (String id : active) activeTable.rawset(id, true);
            if (!payload.isEmpty()) runLoader(payload, activeTable, state);
        } catch (IOException | RuntimeException e) {
            diagnostics.failed(e);
        } finally {
            diagnostics.finish();
        }
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
            var chunk = compile(new String(stream.readAllBytes(), StandardCharsets.UTF_8), "LuaBuddy/bootstrap.lua");
            Object[] result = LuaManager.caller.pcall(LuaManager.thread, chunk, new Object[]{mods, active, state});
            if (result.length == 0 || !Boolean.TRUE.equals(result[0])) {
                String trace = result.length > 2 ? String.valueOf(result[2]) : "";
                Throwable cause = result.length > 3 && result[3] instanceof Throwable error ? error : null;
                throw new IOException("Lua bootstrap failed: " + (result.length > 1 ? result[1] : "no result") + "\n" + trace, cause);
            }
        }
    }

    private static LuaClosure compile(String source, String filename) throws IOException {
        LuaClosure chunk = LuaCompiler.loadstring(source, filename, LuaManager.env);
        sourceMetadata(chunk.prototype, filename);
        return chunk;
    }

    private static void sourceMetadata(Prototype prototype, String filename) {
        prototype.file = filename.substring(filename.lastIndexOf('/') + 1);
        prototype.filename = filename;
        if (prototype.name == null) prototype.name = filename;
        for (Prototype child : prototype.prototypes) sourceMetadata(child, filename);
    }

    static KahluaTable array(List<String> values) {
        KahluaTable table = table();
        for (int i = 0; i < values.size(); i++) table.rawset(i + 1, values.get(i));
        return table;
    }

    static KahluaTable table() {
        return new KahluaTableImpl(new LinkedHashMap<>());
    }
}
