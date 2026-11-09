package luabuddy;

import java.io.IOException;
import java.io.PrintWriter;
import java.io.StringWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.AtomicMoveNotSupportedException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.time.Instant;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Set;
import java.util.UUID;

import org.json.JSONObject;
import se.krka.kahlua.vm.JavaFunction;
import se.krka.kahlua.vm.KahluaTable;
import zombie.ZomboidFileSystem;
import zombie.core.Core;

final class Diagnostics {
    static final String VERSION = "1.4.0";
    private static final Set<String> FIELDS = Set.of("status", "stage", "profile", "commonRoot", "versionRoot",
            "sourceCount", "requires", "preload", "replay", "plugin", "source", "error", "traceback", "javaTraceback");
    private final KahluaTable state;
    private final KahluaTable details;
    private final long started = System.nanoTime();
    private final Map<String, Object> report = new LinkedHashMap<>();

    Diagnostics(KahluaTable state, String directory, String operation, String target) {
        this.state = state;
        details = state.rawget("details") instanceof KahluaTable existing ? existing : Bootstrap.table();
        state.rawset("version", VERSION);
        state.rawset("details", details);
        state.rawset("error", null);
        state.rawset("traceback", null);
        state.rawset("reportError", null);
        state.rawset("debug", Boolean.getBoolean("luabuddy.debug"));
        state.rawset("stackTrace", (JavaFunction) (frame, count) -> frame.push(frame.get(0) instanceof Throwable error
                ? stackTrace(error) : String.valueOf(frame.get(0))));
        report.put("schemaVersion", 1);
        report.put("kind", "loader-initialization");
        report.put("version", VERSION);
        report.put("session", state.rawget("session") instanceof String session ? session : UUID.randomUUID().toString());
        report.put("operation", operation);
        if (target != null) report.put("target", target);
        report.put("startedAt", Instant.now().toString());
        report.put("gameVersion", Core.getInstance().getVersion());
        report.put("javaVersion", System.getProperty("java.version"));
        report.put("zombieBuddyVersion", frameworkVersion());
        report.put("configDirectory", directory == null ? "" : directory);
        report.put("debug", Boolean.getBoolean("luabuddy.debug"));
        state.rawset("session", report.get("session"));
        log("Starting session=" + report.get("session") + " version=" + VERSION + " game=" + report.get("gameVersion")
                + " java=" + report.get("javaVersion") + " zombiebuddy=" + report.get("zombieBuddyVersion")
                + " config=" + (directory == null ? "<missing>" : directory)
                + " debug=" + report.get("debug"));
    }

    KahluaTable mod(ModProfile profile, boolean publish) {
        KahluaTable detail = Bootstrap.table();
        detail.rawset("profile", profile.file().toAbsolutePath().normalize().toString());
        detail.rawset("status", "prepared");
        detail.rawset("stage", "resolve");
        if (publish) details.rawset(profile.id(), detail);
        return detail;
    }

    void failed(String id, KahluaTable detail, Exception error) {
        ((KahluaTable) state.rawget("mods")).rawset(id, "failed: " + error.getMessage());
        detail.rawset("status", "failed");
        detail.rawset("error", error.toString());
        detail.rawset("javaTraceback", stackTrace(error));
        log(id + " failed stage=" + detail.rawget("stage") + "; no retry in this Lua environment: " + error);
        error.printStackTrace(System.err);
    }

    void failed(Exception error) {
        state.rawset("error", error.toString());
        report.put("traceback", stackTrace(error));
        log("Initialization failed: " + error);
        error.printStackTrace(System.err);
    }

    void finish() {
        Map<String, Object> mods = new LinkedHashMap<>();
        Map<String, Integer> summary = new LinkedHashMap<>();
        for (String key : new String[]{"loaded", "serverLoaded", "failed", "disabled", "pending"}) summary.put(key, 0);
        var iterator = details.iterator();
        while (iterator.advance()) {
            if (!(iterator.getValue() instanceof KahluaTable detail)) continue;
            Map<String, Object> fields = new LinkedHashMap<>();
            for (String field : FIELDS) {
                Object value = detail.rawget(field);
                if (value instanceof String || value instanceof Number || value instanceof Boolean) fields.put(field, value);
            }
            mods.put(String.valueOf(iterator.getKey()), fields);
            String key = switch (String.valueOf(detail.rawget("status"))) {
                case "loaded" -> "loaded";
                case "server-loaded" -> "serverLoaded";
                case "failed" -> "failed";
                case "disabled" -> "disabled";
                default -> "pending";
            };
            summary.computeIfPresent(key, (ignored, count) -> count + 1);
        }
        double elapsed = (System.nanoTime() - started) / 1_000_000.0;
        report.put("mods", mods);
        report.put("summary", summary);
        report.put("completedAt", Instant.now().toString());
        report.put("durationMs", elapsed);
        if (state.rawget("error") != null) report.put("error", state.rawget("error"));
        if (state.rawget("traceback") instanceof String trace) report.put("traceback", trace);
        KahluaTable counts = Bootstrap.table();
        summary.forEach(counts::rawset);
        state.rawset("summary", counts);
        String result = state.rawget("error") != null || summary.get("failed") > 0 ? "failed"
                : summary.get("pending") > 0 ? "incomplete" : "completed";
        log("Finished session=" + report.get("session") + " result=" + result + " loaded=" + summary.get("loaded")
                + " server-loaded=" + summary.get("serverLoaded") + " failed=" + summary.get("failed")
                + " disabled=" + summary.get("disabled") + " pending=" + summary.get("pending")
                + " durationMs=" + Math.round(elapsed));
        if (mods.isEmpty() && state.rawget("error") == null) log("No local profiles configured.");
        writeReport();
    }

    private void writeReport() {
        Path temporary = null;
        try {
            Path file = Path.of(ZomboidFileSystem.instance.getCacheDir()).resolve("LuaBuddy/diagnostics.json");
            Files.createDirectories(file.getParent());
            temporary = Files.createTempFile(file.getParent(), ".diagnostics-", ".tmp");
            Files.writeString(temporary, new JSONObject(report).toString(2) + "\n", StandardCharsets.UTF_8);
            try {
                Files.move(temporary, file, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
            } catch (AtomicMoveNotSupportedException error) {
                Files.move(temporary, file, StandardCopyOption.REPLACE_EXISTING);
            }
            state.rawset("reportFile", file.toString());
            log("Last initialization report: " + file);
        } catch (IOException | RuntimeException error) {
            state.rawset("reportError", error.toString());
            log("WARNING: Cannot save initialization report; mod loading is unaffected: " + error);
        } finally {
            if (temporary != null) {
                try {
                    Files.deleteIfExists(temporary);
                } catch (IOException error) {
                    log("WARNING: Cannot remove report temporary file " + temporary + ": " + error);
                }
            }
        }
    }

    static void log(String message) {
        System.out.println("[LuaBuddy] " + message);
    }

    private static String frameworkVersion() {
        try {
            String version = Class.forName("me.zed_0xff.zombie_buddy.Patch", false,
                    Diagnostics.class.getClassLoader()).getPackage().getImplementationVersion();
            return version == null ? "unknown" : version;
        } catch (ClassNotFoundException | LinkageError error) {
            return "unavailable";
        }
    }

    static String stackTrace(Throwable error) {
        StringWriter text = new StringWriter();
        error.printStackTrace(new PrintWriter(text));
        return text.toString();
    }
}
