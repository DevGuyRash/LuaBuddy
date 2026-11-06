package luabuddy;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.FileSystems;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.PathMatcher;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Properties;
import java.util.Set;
import java.util.TreeMap;

import zombie.gameStates.ChooseGameInfo;
import zombie.ZomboidFileSystem;
import zombie.core.znet.SteamWorkshop;
import zombie.core.znet.SteamUtils;

record ModProfile(String id, Path file, Properties options) {
    record Source(String text, String filename) {}
    static List<ModProfile> read(Path directory) throws IOException {
        List<ModProfile> profiles = new ArrayList<>();
        Set<String> ids = new LinkedHashSet<>();
        try (var files = Files.list(directory)) {
            for (Path file : files.filter(path -> path.toString().endsWith(".properties")).sorted().toList()) {
                Properties options = new Properties();
                try (var reader = Files.newBufferedReader(file, StandardCharsets.UTF_8)) {
                    options.load(reader);
                } catch (IOException | IllegalArgumentException error) {
                    throw new IOException("Reading profile " + file + ": " + error.getMessage(), error);
                }
                String id = options.getProperty("id", "").trim();
                if (id.isEmpty()) throw new IOException("Missing id in " + file);
                if (!ids.add(id)) throw new IOException("Duplicate profile for " + id);
                for (String key : options.stringPropertyNames()) {
                    if (!Set.of("id", "path", "enabled", "exclude", "preload", "plugin", "requires", "after", "before", "mount", "replay", "workshop").contains(key)
                            && !key.startsWith("excludeWhen.")) {
                        throw new IOException("Unknown profile option " + key + " in " + file);
                    }
                }
                String enabled = options.getProperty("enabled", "true").trim();
                if (!enabled.equals("true") && !enabled.equals("false")) {
                    throw new IOException("enabled must be true or false in " + file);
                }
                String mount = options.getProperty("mount", "false").trim();
                if (!mount.equals("true") && !mount.equals("false")) {
                    throw new IOException("mount must be true or false in " + file);
                }
                for (String event : options.getProperty("replay", "").split(";")) {
                    if (!event.isBlank() && !Set.of("OnGameBoot", "OnLoad", "OnGameStart").contains(event.trim())) {
                        throw new IOException("replay supports only OnGameBoot, OnLoad and runtime OnGameStart callbacks in " + file);
                    }
                }
                String workshop = options.getProperty("workshop", "").trim();
                if (!workshop.isEmpty() && (!workshop.matches("[0-9]{1,18}") || Long.parseLong(workshop) == 0)) {
                    throw new IOException("workshop must be a positive numeric item ID in " + file);
                }
                profiles.add(new ModProfile(id, file, options));
            }
        }
        return profiles;
    }

    List<String> list(String key) {
        return Arrays.stream(options.getProperty(key, "").split(";")).map(String::trim).filter(value -> !value.isEmpty()).toList();
    }

    boolean enabled() {
        return options.getProperty("enabled", "true").trim().equals("true");
    }

    ChooseGameInfo.Mod resolve() throws IOException {
        String configured = options.getProperty("path", "").trim();
        String workshop = options.getProperty("workshop", "").trim();
        ChooseGameInfo.Mod info = null;
        if (configured.isEmpty()) {
            if (workshop.isEmpty()) {
                info = ChooseGameInfo.getModDetails(id);
            } else {
                String known = ZomboidFileSystem.instance.getModDir(id);
                List<String> folders = new ArrayList<>();
                ZomboidFileSystem.instance.getAllModFolders(folders);
                if (known != null) folders.add(0, known);
                for (String folder : folders) {
                    ChooseGameInfo.Mod candidate = ChooseGameInfo.readModInfo(folder);
                    if (candidate != null && id.equals(candidate.getId())) {
                        info = candidate;
                        break;
                    }
                }
            }
        }
        if (info == null && (!configured.isEmpty() || !workshop.isEmpty())) {
            if (configured.isEmpty() && !SteamUtils.isSteamModeEnabled()) {
                throw new IOException("Steam is unavailable for Workshop item " + workshop + "; set path to its root directory.");
            }
            String installed = configured.isEmpty() ? SteamWorkshop.instance.GetItemInstallFolder(Long.parseLong(workshop))
                    : file.getParent().resolve(configured).toAbsolutePath().normalize().toString();
            if (installed == null) throw new IOException("Workshop item " + workshop + " not available; install it through Steam or set path to its root directory.");
            Path root = Path.of(installed).toAbsolutePath().normalize();
            if (!workshop.isEmpty() && Files.isRegularFile(root.resolve("mod.info"))) {
                ChooseGameInfo.Mod flat = ZomboidFileSystem.instance.getModInfoForDir(root.toString());
                if (!Files.isRegularFile(Path.of(flat.getCommonDir(), "mod.info"))
                        && !Files.isRegularFile(Path.of(flat.getVersionDir(), "mod.info"))) {
                    flat.commonDir = root.toString();
                    flat.versionDir = root.toString();
                    flat.baseFile.setWithCatch(root.toFile(), root.toFile());
                    flat.mediaFile.setWithCatch(root.resolve("media").toFile(), root.resolve("media").toFile());
                }
            }
            info = ChooseGameInfo.readModInfo(root.toString());
        }
        if (info == null) throw new IOException("Mod not installed: " + id + ". Install it or set path to its mod directory.");
        if (!id.equals(info.getId())) throw new IOException("Expected " + id + " but found " + info.getId());
        if (!info.isAvailableSelf()) throw new IOException("Mod is unavailable for this game version: " + id);
        if (!workshop.isEmpty()) ZomboidFileSystem.instance.setModIdToDir(id, info.getDir());
        return info;
    }

    Map<String, Source> sources(List<Path> roots, Set<String> active) throws IOException {
        List<String> excludes = new ArrayList<>(list("exclude"));
        for (String id : active) excludes.addAll(list("excludeWhen." + id));
        List<PathMatcher> matchers = excludes.stream().map(pattern -> FileSystems.getDefault().getPathMatcher("glob:" + pattern)).toList();
        Map<String, Source> sources = new TreeMap<>(String.CASE_INSENSITIVE_ORDER);
        for (Path root : roots) {
            for (String phase : List.of("shared", "client")) {
                Path directory = root.resolve("media/lua/" + phase);
                if (!Files.isDirectory(directory)) continue;
                try (var files = Files.walk(directory)) {
                    for (Path path : files.filter(Files::isRegularFile).sorted().toList()) {
                        String relative = phase + "/" + directory.relativize(path).toString().replace('\\', '/');
                        String normalized = relative.toLowerCase(Locale.ROOT);
                        if (!normalized.endsWith(".lua") || normalized.startsWith("shared/translate/")) continue;
                        if (matchers.stream().anyMatch(matcher -> matcher.matches(Path.of(relative)))) continue;
                        sources.put(relative, new Source(Files.readString(path, StandardCharsets.UTF_8),
                                path.toAbsolutePath().normalize().toString().replace('\\', '/')));
                    }
                }
            }
        }
        if (sources.isEmpty()) throw new IOException("No client or shared Lua files in " + roots);
        return sources;
    }

    String plugin() throws IOException {
        String name = options.getProperty("plugin", "").trim();
        return name.isEmpty() ? "return {}" : Files.readString(file.getParent().resolve(name), StandardCharsets.UTF_8);
    }

    String pluginFilename() {
        String name = options.getProperty("plugin", "").trim();
        return name.isEmpty() ? "LuaBuddy/plugins/" + id : file.getParent().resolve(name).toAbsolutePath().normalize().toString().replace('\\', '/');
    }

    static List<Path> roots(ChooseGameInfo.Mod info) {
        return List.of(Path.of(info.getCommonDir()), Path.of(info.getVersionDir())).stream().distinct().toList();
    }
}
