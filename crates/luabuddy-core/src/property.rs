//! The JVM system properties the launcher sets and the runtime reads.

/// The profile directory.
pub const CONFIG: &str = "luabuddy.config";
/// The directory of templates for mods without a profile.
pub const TEMPLATES: &str = "luabuddy.templates";
/// The native extension store, when it is not under the profile directory.
pub const EXTENSIONS: &str = "luabuddy.extensions";
/// Whether diagnostics include extra detail.
pub const DEBUG: &str = "luabuddy.debug";
/// Whether the manager opens before the world loads.
pub const EARLY_UI: &str = "luabuddy.earlyUI";

/// A `-D` launcher argument that sets `name` to `value`.
pub fn argument(name: &str, value: &str) -> String {
    format!("-D{name}={value}")
}
