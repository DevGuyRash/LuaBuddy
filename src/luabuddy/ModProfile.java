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
import java.util.Map;
import java.util.Properties;
import java.util.Set;
import java.util.TreeMap;

import zombie.gameStates.ChooseGameInfo;

record ModProfile(String id, Path file, Properties options) {
    static List<ModProfile> read(Path directory) throws IOException {
        List<ModProfile> profiles = new ArrayList<>();
        Set<String> ids = new LinkedHashSet<>();
        try (var files = Files.list(directory)) {
            for (Path file : files.filter(path -> path.toString().endsWith(".properties")).sorted().toList()) {
                Properties options = new Properties();
                try (var reader = Files.newBufferedReader(file, StandardCharsets.UTF_8)) {
                    options.load(reader);
                }
                String id = options.getProperty("id", "").trim();
                if (id.isEmpty()) throw new IOException("Missing id in " + file);
                if (!ids.add(id)) throw new IOException("Duplicate profile for " + id);
                for (String key : options.stringPropertyNames()) {
                    if (!Set.of("id", "path", "enabled", "exclude", "preload", "plugin", "requires", "after", "before").contains(key)
                            && !key.startsWith("excludeWhen.")) {
                        throw new IOException("Unknown profile option " + key + " in " + file);
                    }
                }
                String enabled = options.getProperty("enabled", "true").trim();
                if (!enabled.equals("true") && !enabled.equals("false")) {
                    throw new IOException("enabled must be true or false in " + file);
                }
                if (enabled.equals("true")) profiles.add(new ModProfile(id, file, options));
            }
        }
        return profiles;
    }

    List<String> list(String key) {
        return Arrays.stream(options.getProperty(key, "").split(";")).map(String::trim).filter(value -> !value.isEmpty()).toList();
    }

    ChooseGameInfo.Mod resolve() throws IOException {
        String configured = options.getProperty("path", "").trim();
        ChooseGameInfo.Mod info = configured.isEmpty() ? ChooseGameInfo.getModDetails(id)
                : ChooseGameInfo.readModInfo(file.getParent().resolve(configured).toRealPath().toString());
        if (info == null) throw new IOException("Mod not installed: " + id + ". Install it or set path to its mod directory.");
        if (!id.equals(info.getId())) throw new IOException("Expected " + id + " but found " + info.getId());
        if (!info.isAvailableSelf()) throw new IOException("Mod is unavailable for this game version: " + id);
        return info;
    }

    Map<String, String> sources(List<Path> roots, Set<String> active) throws IOException {
        List<String> excludes = new ArrayList<>(list("exclude"));
        for (String id : active) excludes.addAll(list("excludeWhen." + id));
        List<PathMatcher> matchers = excludes.stream().map(pattern -> FileSystems.getDefault().getPathMatcher("glob:" + pattern)).toList();
        Map<String, String> sources = new TreeMap<>(String.CASE_INSENSITIVE_ORDER);
        for (Path root : roots) {
            for (String phase : List.of("shared", "client")) {
                Path directory = root.resolve("media/lua/" + phase);
                if (!Files.isDirectory(directory)) continue;
                try (var files = Files.walk(directory)) {
                    for (Path path : files.filter(Files::isRegularFile).sorted().toList()) {
                        String relative = phase + "/" + directory.relativize(path).toString().replace('\\', '/');
                        if (!relative.endsWith(".lua") || relative.startsWith("shared/Translate/")) continue;
                        if (matchers.stream().anyMatch(matcher -> matcher.matches(Path.of(relative)))) continue;
                        sources.put(relative, Files.readString(path, StandardCharsets.UTF_8));
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

    static List<Path> roots(ChooseGameInfo.Mod info) {
        return List.of(Path.of(info.getCommonDir()), Path.of(info.getVersionDir())).stream().distinct().toList();
    }
}
