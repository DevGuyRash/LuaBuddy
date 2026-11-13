use luabuddy::{
    InstallOptions, InstallationStatus, Package, find_game, install, installation_status,
    recover_install, rollback, uninstall, verify_install, verify_package,
};
use serde_json::Value;
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, String>;
const PANEL_WIDTH: &str = "820";

pub(crate) struct Options {
    pub package: Option<PathBuf>,
    pub game: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Install,
    Maintenance,
    Verify,
    Repair,
    Rollback,
    Recover,
    Remove,
    ChooseGame,
    ChoosePackage,
}

struct MenuItem {
    action: Action,
    id: &'static str,
    label: &'static str,
    description: &'static str,
}

const INSTALL: MenuItem = MenuItem {
    action: Action::Install,
    id: "install",
    label: "Install / update",
    description: "Use this verified package for the next normal Steam launch",
};
const MAINTENANCE: MenuItem = MenuItem {
    action: Action::Maintenance,
    id: "maintenance",
    label: "Maintenance",
    description: "Verify, repair, recover, roll back, or remove LuaBuddy",
};
const CHOOSE_GAME: MenuItem = MenuItem {
    action: Action::ChooseGame,
    id: "game",
    label: "Choose game folder",
    description: "Select another Project Zomboid installation",
};
const CHOOSE_PACKAGE: MenuItem = MenuItem {
    action: Action::ChoosePackage,
    id: "package",
    label: "Choose package folder",
    description: "Select another extracted LuaBuddy package",
};
const VERIFY: MenuItem = MenuItem {
    action: Action::Verify,
    id: "verify",
    label: "Verify installed files",
    description: "Check installation integrity without changing files",
};
const REPAIR: MenuItem = MenuItem {
    action: Action::Repair,
    id: "repair",
    label: "Repair",
    description: "Restore this package and preserve modified files for recovery",
};
const ROLLBACK: MenuItem = MenuItem {
    action: Action::Rollback,
    id: "rollback",
    label: "Roll back",
    description: "Select the previous installed native generation",
};
const RECOVER: MenuItem = MenuItem {
    action: Action::Recover,
    id: "recover",
    label: "Recover interrupted setup",
    description: "Recover a pending installation transaction",
};
const REMOVE: MenuItem = MenuItem {
    action: Action::Remove,
    id: "remove",
    label: "Remove LuaBuddy",
    description: "Remove native activation while retaining profiles and Workshop mods",
};

enum Dialogs {
    Zenity(PathBuf),
    KDialog(PathBuf),
}

impl Dialogs {
    fn detect() -> Result<Self> {
        if !cfg!(target_os = "linux") {
            return Err("Graphical setup currently supports native Linux only.".into());
        }
        if !graphical_session() {
            return Err("Graphical setup needs a Linux desktop session. Open the extracted luabuddy executable from your file manager, or use luabuddy help for CLI commands.".into());
        }
        let kde = env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .split(':')
            .any(|part| part.eq_ignore_ascii_case("KDE"));
        let names = if kde {
            ["kdialog", "zenity"]
        } else {
            ["zenity", "kdialog"]
        };
        for name in names {
            if let Some(path) = program(name) {
                return Ok(if name == "zenity" {
                    Self::Zenity(path)
                } else {
                    Self::KDialog(path)
                });
            }
        }
        Err("Graphical setup needs the desktop's Zenity or KDialog utility. Neither is available in this session. No installation files were changed.".into())
    }

    fn command(&self, title: &str) -> Command {
        let path = match self {
            Self::Zenity(path) | Self::KDialog(path) => path,
        };
        let mut command = Command::new(path);
        command.arg("--title").arg(title).stdin(Stdio::null());
        command
    }

    fn menu(
        &self,
        title: &str,
        text: &str,
        items: &[&MenuItem],
        cancel_label: &str,
    ) -> Result<Option<Action>> {
        let mut command = self.command(title);
        match self {
            Self::Zenity(_) => {
                let text = markup_text(text);
                command.args([
                    "--list",
                    "--radiolist",
                    "--text",
                    &text,
                    "--column",
                    "",
                    "--column",
                    "ID",
                    "--column",
                    "Action",
                    "--column",
                    "What it does",
                    "--hide-column=2",
                    "--print-column=2",
                    "--width",
                    PANEL_WIDTH,
                    "--height=470",
                    "--ok-label=Continue",
                    "--cancel-label",
                    cancel_label,
                ]);
                for (index, item) in items.iter().enumerate() {
                    command.args([
                        if index == 0 { "TRUE" } else { "FALSE" },
                        item.id,
                        item.label,
                        item.description,
                    ]);
                }
            }
            Self::KDialog(_) => {
                command.arg("--menu").arg(format!(
                    "<qt><table width=\"{PANEL_WIDTH}\"><tr><td>{}</td></tr></table></qt>",
                    rich_body(text)
                ));
                for item in items {
                    command
                        .arg(item.id)
                        .arg(format!("{}: {}", item.label, item.description));
                }
                if let Some(first) = items.first() {
                    command.arg("--default").arg(first.id);
                }
                command.arg("--cancel-label").arg(cancel_label);
            }
        }
        let Some(choice) = selection(command)? else {
            return Ok(None);
        };
        items
            .iter()
            .find(|item| item.id == choice)
            .map(|item| Some(item.action))
            .ok_or_else(|| "The desktop dialog returned an unknown setup action.".into())
    }

    fn folder(&self, title: &str, initial: &Path) -> Result<Option<PathBuf>> {
        let mut command = self.command(title);
        match self {
            Self::Zenity(_) => {
                command
                    .args(["--file-selection", "--directory", "--filename"])
                    .arg(directory_hint(initial))
                    .args(["--width=760", "--height=560"]);
            }
            Self::KDialog(_) => {
                command.arg("--getexistingdirectory").arg(initial);
            }
        }
        selection(command).map(|choice| choice.map(PathBuf::from))
    }

    fn confirm(&self, title: &str, text: &str, accept: &str) -> Result<bool> {
        let prompt;
        let text = if needs_details(text) {
            if !self.details(title, text, "Continue")? {
                return Ok(false);
            }
            prompt = format!("Continue with {accept}?");
            &prompt
        } else {
            text
        };
        let mut command = self.command(title);
        match self {
            Self::Zenity(_) => {
                let text = display_text(text);
                command.args([
                    "--question",
                    "--no-markup",
                    "--text",
                    &text,
                    "--width=660",
                    "--ok-label",
                    accept,
                    "--cancel-label=Cancel",
                ]);
            }
            Self::KDialog(_) => {
                command
                    .arg("--yesno")
                    .arg(rich_text(text))
                    .arg("--yes-label")
                    .arg(accept)
                    .args(["--no-label", "Cancel"]);
            }
        }
        self.message_box(command)
    }

    fn message(&self, error: bool, title: &str, text: &str) -> Result<()> {
        if needs_details(text) {
            return self.details(title, text, "Close").map(|_| ());
        }
        let mut command = self.command(title);
        match self {
            Self::Zenity(_) => {
                let text = display_text(text);
                command.args([
                    if error { "--error" } else { "--info" },
                    "--no-markup",
                    "--text",
                    &text,
                    "--width=680",
                ]);
            }
            Self::KDialog(_) => {
                command
                    .arg(if error { "--error" } else { "--msgbox" })
                    .arg(rich_text(text));
            }
        }
        self.message_box(command).map(|_| ())
    }

    fn message_box(&self, mut command: Command) -> Result<bool> {
        let output = command
            .output()
            .map_err(|error| format!("Could not open the desktop message box: {error}"))?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            Some(2) if matches!(self, Self::KDialog(_)) => Ok(false),
            _ => Err(dialog_failure(output)),
        }
    }

    fn details(&self, title: &str, text: &str, accept: &str) -> Result<bool> {
        let mut command = self.command(title);
        let input = match self {
            Self::Zenity(_) => {
                command.args([
                    "--text-info",
                    "--width",
                    PANEL_WIDTH,
                    "--height=540",
                    "--ok-label",
                    accept,
                    "--cancel-label=Back",
                ]);
                text.to_owned()
            }
            Self::KDialog(_) => {
                command.args(["--textbox", "-", PANEL_WIDTH, "540"]);
                text.split('\n')
                    .map(|line| {
                        format!(
                            "<p style=\"white-space:pre-wrap\">{}</p>",
                            escape_markup(line)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };
        Ok(selection_with_input(command, &input)?.is_some())
    }

    fn working(&self, text: &str) -> Result<Progress> {
        let mut command = self.command("LuaBuddy setup");
        match self {
            Self::Zenity(_) => {
                let text = display_text(text);
                command
                    .args([
                        "--progress",
                        "--pulsate",
                        "--auto-close",
                        "--no-cancel",
                        "--no-markup",
                        "--text",
                        &text,
                        "--width=620",
                    ])
                    .stdin(Stdio::piped());
            }
            Self::KDialog(_) => {
                command.arg("--passivepopup").arg(rich_text(text)).arg("60");
            }
        }
        let child = command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Could not open setup progress: {error}"))?;
        Ok(Progress(Some(child)))
    }
}

struct Progress(Option<Child>);

impl Progress {
    fn finish(mut self) -> Result<()> {
        let Some(mut child) = self.0.take() else {
            return Ok(());
        };
        if let Some(mut input) = child.stdin.take() {
            let _ = input.write_all(b"100\n");
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                Ok(None) => break,
                Err(error) => {
                    self.0 = Some(child);
                    return Err(format!("Could not inspect setup progress: {error}"));
                }
            }
        }
        let killed = child.kill();
        if let Err(error) = child.wait() {
            self.0 = Some(child);
            return Err(format!("Could not close setup progress: {error}"));
        }
        if let Err(error) = killed
            && error.kind() != std::io::ErrorKind::InvalidInput
        {
            return Err(format!("Could not stop setup progress: {error}"));
        }
        Ok(())
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn program(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths).find_map(|directory| {
            let path = if directory.is_absolute() {
                directory.join(name)
            } else {
                env::current_dir().ok()?.join(directory).join(name)
            };
            let metadata = path.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 == 0 {
                    return None;
                }
            }
            Some(path)
        })
    })
}

pub(crate) fn graphical_session() -> bool {
    cfg!(target_os = "linux")
        && ["DISPLAY", "WAYLAND_DISPLAY"]
            .iter()
            .any(|key| env::var_os(key).is_some_and(|value| !value.is_empty()))
}

fn selection(mut command: Command) -> Result<Option<String>> {
    let output = command
        .output()
        .map_err(|error| format!("Could not open the desktop dialog: {error}"))?;
    dialog_selection(output)
}

fn selection_with_input(mut command: Command, text: &str) -> Result<Option<String>> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not open the desktop details dialog: {error}"))?;
    let written = match child.stdin.take() {
        Some(mut input) => input.write_all(text.as_bytes()),
        None => Err(std::io::Error::other("Details input was not available")),
    };
    if let Err(error) = &written
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        let _ = child.kill();
        let waited = child.wait();
        return Err(match waited {
            Ok(_) => format!("Could not send complete dialog details: {error}"),
            Err(cleanup) => {
                format!("Could not send dialog details: {error}; could not reap dialog: {cleanup}")
            }
        });
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("Could not finish the desktop details dialog: {error}"))?;
    if written.is_err() && output.status.success() {
        return Err("The desktop dialog closed without receiving its complete details.".into());
    }
    dialog_selection(output)
}

fn dialog_selection(output: Output) -> Result<Option<String>> {
    match output.status.code() {
        Some(0) => String::from_utf8(output.stdout)
            .map(|text| Some(text.strip_suffix('\n').unwrap_or(&text).to_owned()))
            .map_err(|_| "The desktop dialog returned a non-UTF-8 selection.".into()),
        Some(1) => Ok(None),
        _ => Err(dialog_failure(output)),
    }
}

fn dialog_failure(output: Output) -> String {
    let details: String = String::from_utf8_lossy(&output.stderr)
        .trim()
        .chars()
        .take(2048)
        .collect();
    format!("The desktop dialog failed ({}). {details}", output.status)
}

fn display_text(text: &str) -> String {
    const COLUMNS: usize = 72;
    let mut display = String::with_capacity(text.len());
    let mut line = Vec::with_capacity(COLUMNS + 1);
    for character in text.chars() {
        if character == '\n' {
            display.extend(line.drain(..));
            display.push('\n');
            continue;
        }
        line.push(character);
        if line.len() > COLUMNS {
            let cut = line[..COLUMNS]
                .iter()
                .rposition(|character| character.is_whitespace() || *character == '/')
                .map_or(COLUMNS, |index| index + 1);
            display.extend(line.drain(..cut));
            display.push('\n');
        }
    }
    display.extend(line);
    display
}

fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn markup_text(text: &str) -> String {
    escape_markup(&display_text(text))
}

fn rich_text(text: &str) -> String {
    format!("<qt>{}</qt>", rich_body(text))
}

fn rich_body(text: &str) -> String {
    markup_text(text).replace('\n', "<br>")
}

fn needs_details(text: &str) -> bool {
    text.chars().count() > 900 || text.lines().count() > 10
}

fn directory_hint(path: &Path) -> PathBuf {
    let mut hint = path.as_os_str().to_os_string();
    hint.push(std::path::MAIN_SEPARATOR_STR);
    PathBuf::from(hint)
}

fn choose_package(dialogs: &Dialogs, requested: Option<PathBuf>) -> Result<Option<Package>> {
    let mut candidate = match requested {
        Some(path) => path,
        None => env::current_exe()
            .map_err(|error| format!("Could not locate this LuaBuddy executable: {error}"))?
            .parent()
            .ok_or("The LuaBuddy executable has no parent folder.")?
            .to_owned(),
    };
    loop {
        match verify_package(&candidate) {
            Ok(package) => return Ok(Some(package)),
            Err(error) => {
                if !dialogs.confirm(
                    "Choose a verified LuaBuddy package",
                    &format!("This package could not be verified:\n{}\n\n{error}\n\nChoose the extracted folder containing luabuddy and manifest.json. No installation files were changed.", candidate.display()),
                    "Choose package folder",
                )? {
                    return Ok(None);
                }
                let Some(selected) =
                    dialogs.folder("Choose LuaBuddy package folder", &candidate)?
                else {
                    return Ok(None);
                };
                candidate = selected;
            }
        }
    }
}

fn game(dialogs: &Dialogs, requested: Option<&Path>) -> Result<Option<PathBuf>> {
    match find_game(requested) {
        Ok(game) => return Ok(Some(game)),
        Err(error) => dialogs.message(
            true,
            "Choose Project Zomboid",
            &format!("Project Zomboid could not be selected automatically.\n\n{error}\n\nChoose its installation folder in the next window."),
        )?,
    }
    let mut initial = requested
        .map(Path::to_owned)
        .unwrap_or_else(|| env::var_os("HOME").map(PathBuf::from).unwrap_or_default());
    loop {
        let Some(selected) = dialogs.folder("Choose Project Zomboid game folder", &initial)? else {
            return Ok(None);
        };
        match find_game(Some(&selected)) {
            Ok(game) => return Ok(Some(game)),
            Err(error) => dialogs.message(true, "Invalid game folder", &error.to_string())?,
        }
        initial = selected;
    }
}

fn status_text(status: &Result<Option<InstallationStatus>>, package: &Package) -> String {
    match status {
        Ok(None) => "LuaBuddy is not installed in this game folder.".into(),
        Ok(Some(status)) if !status.problems.is_empty() => format!(
            "Installed version {} needs attention.\nInspection reported {} problem{}.\nUse Maintenance > Verify installed files for full details, or Repair to restore owned files.",
            status.package_version,
            status.problems.len(),
            if status.problems.len() == 1 { "" } else { "s" }
        ),
        Ok(Some(status)) if status.package_digest == package.digest => format!(
            "This package is already installed ({}). Install / update can check it again without changing existing selections.",
            status.package_version
        ),
        Ok(Some(status)) => format!(
            "Installed version {}. Install / update selects the supplied verified package.",
            status.package_version
        ),
        Err(_) => "Installation metadata could not be inspected.\nUse Maintenance > Verify installed files for the full error, or Recover interrupted setup. You can also choose another game folder.".into(),
    }
}

fn maintenance(
    dialogs: &Dialogs,
    status: &Result<Option<InstallationStatus>>,
) -> Result<Option<Action>> {
    let mut items = vec![&VERIFY, &RECOVER];
    if let Ok(Some(status)) = status {
        items.insert(1, &REPAIR);
        if status.previous_version.is_some() {
            items.push(&ROLLBACK);
        }
        items.push(&REMOVE);
    }
    dialogs.menu(
        "LuaBuddy maintenance",
        "Choose a maintenance action. Profiles and Workshop mods are preserved. Native changes take effect after a normal game restart.",
        &items,
        "Back",
    )
}

fn perform(action: Action, package: &Package, game: &Path) -> Result<Value> {
    match action {
        Action::Install | Action::Repair => install(
            game,
            &InstallOptions {
                package: package.directory.clone(),
                profiles: None,
                mods: Vec::new(),
                debug: None,
                early_ui: None,
                repair: action == Action::Repair,
            },
        ),
        Action::Verify => verify_install(game),
        Action::Rollback => rollback(game),
        Action::Recover => recover_install(game),
        Action::Remove => uninstall(game),
        _ => return Err("The selected action does not change or inspect an installation.".into()),
    }
    .map_err(|error| error.to_string())
}

fn show_result(dialogs: &Dialogs, action: Action, value: &Value) -> Result<()> {
    if action == Action::Verify {
        let problems = value
            .get("problems")
            .and_then(Value::as_array)
            .ok_or("Verification returned no problem list.")?;
        let text = if problems.is_empty() {
            "Installed files and launcher settings passed verification.\n\nThis checks installation integrity. In-game diagnostics shows whether selected mods loaded.".to_owned()
        } else {
            format!(
                "Verification found problems:\n\n{}\n\nUse Repair with the appropriate verified package when restoring owned files.",
                problems
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        return dialogs.message(!problems.is_empty(), "LuaBuddy verification", &text);
    }
    if action == Action::Recover {
        let recovered = value
            .get("recovered")
            .and_then(Value::as_bool)
            .ok_or("Recovery returned no outcome.")?;
        return dialogs.message(false, "LuaBuddy recovery", if recovered {
            "The interrupted installation was recovered. Review the updated setup status before continuing. Fully restart the game through Steam if launcher settings changed."
        } else {
            "There was no interrupted installation to recover. No installation files changed."
        });
    }
    let changed = value
        .get("changed")
        .and_then(Value::as_bool)
        .ok_or("Setup returned no change status.")?;
    let recovered = match value.get("recoveredInterruptedOperation") {
        Some(value) => value
            .as_bool()
            .ok_or("Setup returned an invalid recovery outcome.")?,
        None => false,
    };
    let recovery = if recovered {
        "An interrupted setup was recovered before this action.\n\n"
    } else {
        ""
    };
    let result = match (action, changed) {
        (Action::Install, true) => "LuaBuddy is installed for the next normal Steam launch.",
        (Action::Install, false) => {
            "This package is already selected. No further installation changes were needed."
        }
        (Action::Repair, true) => "LuaBuddy's owned installation files were repaired.",
        (Action::Repair, false) => "No further repair was needed.",
        (Action::Rollback, true) => {
            "The previous native generation is selected for the next normal Steam launch."
        }
        (Action::Rollback, false) => "No further rollback changes were needed.",
        (Action::Remove, true) => {
            "LuaBuddy's native activation was removed from future game launches."
        }
        (Action::Remove, false) => {
            "LuaBuddy is already removed. No further removal changes were needed."
        }
        _ => return Err("Setup returned an unexpected action outcome.".into()),
    };
    let preservation = if action == Action::Remove {
        "Profiles, Workshop mods, and recovery files were retained."
    } else {
        "Existing profiles and settings were preserved. No mods were selected automatically."
    };
    dialogs.message(false, "LuaBuddy setup complete", &format!("{recovery}{result}\n\n{preservation}\n\nFully close Project Zomboid, then launch it normally through Steam. Changes do not replace code in an already running game."))
}

pub(crate) fn run(options: Options) -> Result<()> {
    let dialogs = Dialogs::detect()?;
    let Some(mut package) = choose_package(&dialogs, options.package)? else {
        return Ok(());
    };
    let Some(mut game) = game(&dialogs, options.game.as_deref())? else {
        return Ok(());
    };
    loop {
        let status = installation_status(&game).map_err(|error| error.to_string());
        let text = format!(
            "Package: LuaBuddy {}\nGame folder: {}\n\n{}\n\nClose Project Zomboid before applying setup changes. Launch normally through Steam afterward.",
            package.manifest.version,
            game.display(),
            status_text(&status, &package)
        );
        let mut items = vec![&MAINTENANCE, &CHOOSE_GAME, &CHOOSE_PACKAGE];
        if status.is_ok() {
            items.insert(0, &INSTALL);
        }
        let Some(mut action) = dialogs.menu("LuaBuddy setup", &text, &items, "Close")? else {
            return Ok(());
        };
        match action {
            Action::ChooseGame => {
                if let Some(selected) =
                    dialogs.folder("Choose Project Zomboid game folder", &game)?
                {
                    match find_game(Some(&selected)) {
                        Ok(selected) => game = selected,
                        Err(error) => {
                            dialogs.message(true, "Invalid game folder", &error.to_string())?
                        }
                    }
                }
                continue;
            }
            Action::ChoosePackage => {
                if let Some(selected) =
                    dialogs.folder("Choose LuaBuddy package folder", &package.directory)?
                    && let Some(selected) = choose_package(&dialogs, Some(selected))?
                {
                    package = selected;
                }
                continue;
            }
            Action::Maintenance => {
                let Some(selected) = maintenance(&dialogs, &status)? else {
                    continue;
                };
                action = selected;
            }
            _ => (),
        }
        if action == Action::Remove && !dialogs.confirm(
            "Remove LuaBuddy?",
            "Remove LuaBuddy's native activation from this game?\n\nYour profiles, Workshop mods, recovery files, and unrelated launcher settings are retained. The running game is not stopped. Restart normally after removal.",
            "Remove LuaBuddy",
        )? {
            continue;
        }
        if action == Action::Rollback && !dialogs.confirm(
            "Roll back LuaBuddy?",
            "Select the previous installed native generation?\n\nProfile files are retained. The previous generation's profile-folder selection is restored. Restart normally after rollback.",
            "Roll back",
        )? {
            continue;
        }
        let progress = dialogs.working("Applying the selected setup action.\n\nClosing a progress view does not cancel the installation transaction.")?;
        let result = perform(action, &package, &game);
        let cleanup = progress.finish();
        match (result, cleanup) {
            (Ok(value), Ok(())) => show_result(&dialogs, action, &value)?,
            (Err(error), Ok(())) => dialogs.message(true, "LuaBuddy setup failed", &error)?,
            (Ok(_), Err(error)) => dialogs.message(true, "Check setup status", &format!("The selected action finished, but its progress window did not close normally.\n\n{error}\n\nReview the installation status before repeating the action."))?,
            (Err(error), Err(cleanup)) => dialogs.message(true, "LuaBuddy setup failed", &format!("{error}\n\nProgress cleanup also failed: {cleanup}"))?,
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::dialog_selection;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};

    #[test]
    fn native_dialog_output_keeps_pathname_newlines() {
        for (output, path) in [
            ("/games/Project Zomboid\n", "/games/Project Zomboid"),
            ("/games/Game\n\n", "/games/Game\n"),
            ("/games/Game\r\n", "/games/Game\r"),
            ("/games/Game", "/games/Game"),
        ] {
            let result = Output {
                status: ExitStatus::from_raw(0),
                stdout: output.as_bytes().to_vec(),
                stderr: Vec::new(),
            };
            assert_eq!(dialog_selection(result).unwrap().as_deref(), Some(path));
        }
    }

    #[test]
    fn selection_errors_are_not_message_box_cancellation() {
        let result = Output {
            status: ExitStatus::from_raw(2 << 8),
            stdout: Vec::new(),
            stderr: b"invalid selection invocation".to_vec(),
        };
        assert!(dialog_selection(result).is_err());
    }
}
