//! An append-only record of what LuaBuddy intended, decided, did and observed.
//!
//! `diagnostics.json` answers "what is the state now". The journal answers
//! "how did it get that way": one JSON object per line, oldest first.

use crate::ModId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const FILE: &str = "journal.jsonl";
pub const PREVIOUS_FILE: &str = "journal.previous.jsonl";
const ROTATE_BYTES: u64 = 8 * 1024 * 1024;
const RECENT: usize = 800;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Detail that explains a step. Kept in the journal, not echoed to the game log.
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub time: String,
    pub seq: u64,
    pub session: String,
    /// Every entry written while one intent is being carried out shares its number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<u64>,
    pub level: Level,
    pub event: String,
    #[serde(default, rename = "mod", skip_serializing_if = "Option::is_none")]
    pub module: Option<ModId>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

impl Entry {
    /// One readable line, for a terminal or the game's own log.
    pub fn line(&self) -> String {
        let mut line = format!("{}: ", self.event);
        if let Some(module) = &self.module {
            line.push_str(&format!("[{module}] "));
        }
        line.push_str(&self.message);
        if let Some(ms) = self.duration_ms {
            line.push_str(&format!(" ({ms:.0} ms)"));
        }
        line
    }
}

#[derive(Debug, Default)]
pub struct Journal {
    session: String,
    next: u64,
    recent: VecDeque<Entry>,
    unwritten: Vec<Entry>,
    target: Option<Target>,
}

#[derive(Debug)]
struct Target {
    directory: PathBuf,
    file: File,
    bytes: u64,
}

impl Journal {
    pub fn set_session(&mut self, session: impl Into<String>) {
        self.session = session.into();
    }

    /// Starts writing to `directory`. Entries recorded earlier are written first.
    pub fn attach(&mut self, directory: &Path) -> io::Result<()> {
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.directory == directory)
        {
            return Ok(());
        }
        fs::create_dir_all(directory)?;
        self.target = Some(open(directory)?);
        for entry in std::mem::take(&mut self.unwritten) {
            self.write(&entry)?;
        }
        Ok(())
    }

    /// Records the entry. The entry is retained in memory even when writing fails.
    pub fn append(&mut self, mut entry: Entry) -> (Entry, io::Result<()>) {
        self.next += 1;
        entry.seq = self.next;
        entry.session.clone_from(&self.session);
        if entry.time.is_empty() {
            entry.time = timestamp(SystemTime::now());
        }
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(entry.clone());
        let result = if self.target.is_some() {
            self.write(&entry)
        } else {
            if self.unwritten.len() < RECENT {
                self.unwritten.push(entry.clone());
            }
            Ok(())
        };
        (entry, result)
    }

    fn write(&mut self, entry: &Entry) -> io::Result<()> {
        let mut line = serde_json::to_vec(entry).map_err(io::Error::other)?;
        line.push(b'\n');
        let Some(target) = &mut self.target else {
            return Ok(());
        };
        if target.bytes > 0 && target.bytes + line.len() as u64 > ROTATE_BYTES {
            let directory = target.directory.clone();
            fs::rename(directory.join(FILE), directory.join(PREVIOUS_FILE))?;
            *target = open(&directory)?;
        }
        target.file.write_all(&line)?;
        target.bytes += line.len() as u64;
        Ok(())
    }

    pub fn recent(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.recent.iter()
    }

    /// Changes whenever an entry is recorded.
    pub fn revision(&self) -> u64 {
        self.next
    }

    pub fn directory(&self) -> Option<&Path> {
        self.target
            .as_ref()
            .map(|target| target.directory.as_path())
    }
}

fn open(directory: &Path) -> io::Result<Target> {
    let path = directory.join(FILE);
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let bytes = file.metadata()?.len();
    Ok(Target {
        directory: directory.to_owned(),
        file,
        bytes,
    })
}

/// Reads journal lines oldest first, including the rotated file. A torn final
/// line from a crash is skipped rather than hiding everything before it.
pub fn read(directory: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for name in [PREVIOUS_FILE, FILE] {
        match fs::read_to_string(directory.join(name)) {
            Ok(text) => entries.extend(
                text.lines()
                    .filter_map(|line| serde_json::from_str::<Entry>(line).ok()),
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    Ok(entries)
}

/// UTC in RFC 3339 form with millisecond precision.
pub fn timestamp(time: SystemTime) -> String {
    let elapsed = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = elapsed.as_secs();
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Civil-from-days, using a year that starts in March so leap days fall last.
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        elapsed.subsec_millis()
    )
}

/// Reads a time written by [`timestamp`].
pub fn parse_timestamp(text: &str) -> Option<SystemTime> {
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<u64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let seconds = number(11..13)? * 3600 + number(14..16)? * 60 + number(17..19)?;
    let millis = number(20..23)?;
    // Days from the civil date, again with years that start in March.
    let year = year.checked_sub(u64::from(month <= 2))?;
    let era = year / 400;
    let year_of_era = year % 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day.checked_sub(1)?;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = (era * 146_097 + day_of_era).checked_sub(719_468)?;
    UNIX_EPOCH.checked_add(std::time::Duration::from_millis(
        (days * 86_400 + seconds) * 1000 + millis,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn entry(message: &str) -> Entry {
        Entry {
            time: String::new(),
            seq: 0,
            session: String::new(),
            operation: Some(7),
            level: Level::Info,
            event: "test.event".into(),
            module: Some(ModId::new("Mod").unwrap()),
            message: message.into(),
            duration_ms: Some(1.5),
            data: serde_json::json!({ "key": "value" }),
        }
    }

    #[test]
    fn timestamps_are_utc_rfc3339_and_read_back() {
        let time = UNIX_EPOCH + Duration::from_millis(1_795_640_000_123);
        let text = timestamp(time);
        assert_eq!(text, "2026-11-25T20:53:20.123Z");
        assert_eq!(parse_timestamp(&text), Some(time));
        assert_eq!(
            timestamp(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00.000Z"
        );
        assert_eq!(
            parse_timestamp("2000-02-29T00:00:00.000Z"),
            Some(UNIX_EPOCH + Duration::from_secs(951_782_400))
        );
        assert_eq!(parse_timestamp("not a time"), None);
    }

    #[test]
    fn entries_recorded_before_a_directory_are_written_to_it_in_order() {
        let directory =
            std::env::temp_dir().join(format!("luabuddy-journal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let mut journal = Journal::default();
        journal.set_session("one");
        journal.append(entry("early")).1.unwrap();
        journal.attach(&directory).unwrap();
        journal.append(entry("later")).1.unwrap();
        let written = read(&directory).unwrap();
        assert_eq!(
            written
                .iter()
                .map(|entry| entry.message.as_str())
                .collect::<Vec<_>>(),
            ["early", "later"]
        );
        assert_eq!(written[1].seq, 2);
        assert_eq!(written[1].session, "one");
        assert_eq!(written[1].data["key"], "value");
        // A line cut off by a crash hides only itself.
        let mut file = OpenOptions::new()
            .append(true)
            .open(directory.join(FILE))
            .unwrap();
        file.write_all(b"{\"time\":\"cut").unwrap();
        assert_eq!(read(&directory).unwrap().len(), 2);
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_line_says_what_happened_to_which_mod() {
        assert_eq!(entry("Loaded").line(), "test.event: [Mod] Loaded (2 ms)");
    }
}
