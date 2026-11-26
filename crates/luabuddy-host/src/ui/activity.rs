//! The journal as the window shows it.

use super::{
    model::{Block, Tone},
    panes::{Face, Row},
    screen::Key,
};
use luabuddy_core::journal::{self, Entry, Level};
use serde_json::Value as Json;
use std::{collections::BTreeSet, time::SystemTime};

fn tone(entry: &Entry) -> Tone {
    match entry.level {
        Level::Debug => Tone::Muted,
        Level::Info => Tone::Normal,
        Level::Warn => Tone::Warning,
        Level::Error => Tone::Bad,
    }
}

/// How long ago, in the words a reader would use.
fn age(entry: &Entry, now: SystemTime) -> String {
    let Some(elapsed) =
        journal::parse_timestamp(&entry.time).and_then(|then| now.duration_since(then).ok())
    else {
        return entry.time.clone();
    };
    match elapsed.as_secs() {
        0..=4 => "just now".into(),
        seconds @ 5..=59 => format!("{seconds} s ago"),
        seconds @ 60..=3599 => format!("{} min ago", seconds / 60),
        seconds => format!("{} h ago", seconds / 3600),
    }
}

/// Whether the entry ends an action, however it went.
fn ends(entry: &Entry) -> bool {
    entry.event.ends_with(".outcome") || entry.event.ends_with(".failed")
}

/// Newest first. One action is one row: its outcome, and anything that went
/// wrong on the way. Searching also finds every step in between.
pub fn rows(entries: &[Entry], filter: &str) -> Vec<Row<Key>> {
    let now = SystemTime::now();
    let finished: BTreeSet<_> = entries
        .iter()
        .filter(|entry| ends(entry))
        .filter_map(|entry| entry.operation)
        .collect();
    entries
        .iter()
        .filter(|entry| {
            if !filter.is_empty() {
                return entry.line().to_lowercase().contains(filter);
            }
            let step = entry
                .operation
                .is_some_and(|operation| finished.contains(&operation))
                && !ends(entry);
            entry.level > if step { Level::Info } else { Level::Debug }
        })
        .map(|entry| Row {
            key: Key::Entry(entry.seq),
            text: first_line(&entry.message).to_owned(),
            face: Face::Item {
                status: match &entry.module {
                    Some(id) => format!("{}   {id}", age(entry, now)),
                    None => age(entry, now),
                },
                tone: tone(entry),
                quiet: true,
            },
        })
        .collect()
}

use super::kit::first_line;

pub fn document(entry: &Entry, entries: &[Entry]) -> Vec<Block> {
    let mut blocks = vec![
        Block::Title(first_line(&entry.message).to_owned()),
        Block::Status(tone(entry), entry.event.clone()),
    ];
    let mut rest = entry.message.lines().skip(1).peekable();
    if rest.peek().is_some() {
        for line in rest {
            blocks.push(Block::Text(Tone::Normal, line.to_owned()));
        }
    }
    blocks.push(Block::Heading("Details".into()));
    let fact = |name: &str, value: String| Block::Fact(name.into(), Tone::Normal, value);
    blocks.push(fact("Time", entry.time.clone()));
    if let Some(module) = &entry.module {
        blocks.push(fact("Mod", module.to_string()));
    }
    if let Some(ms) = entry.duration_ms {
        blocks.push(fact("Took", format!("{ms:.1} ms")));
    }
    let mut facts = Vec::new();
    flatten("", &entry.data, &mut facts);
    for (name, value) in facts {
        blocks.push(fact(&name, value));
    }
    if let Some(operation) = entry.operation {
        let related = entries
            .iter()
            .rev()
            .filter(|other| other.operation == Some(operation) && other.session == entry.session)
            .collect::<Vec<_>>();
        if related.len() > 1 {
            blocks.push(Block::Heading("Everything in this action".into()));
            for other in related {
                blocks.push(Block::Text(
                    if other.seq == entry.seq {
                        Tone::Normal
                    } else {
                        match tone(other) {
                            Tone::Normal => Tone::Muted,
                            tone => tone,
                        }
                    },
                    first_line(&other.line()).to_owned(),
                ));
            }
        }
    }
    blocks
}

fn flatten(prefix: &str, value: &Json, facts: &mut Vec<(String, String)>) {
    let name = |key: &str| {
        if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}.{key}")
        }
    };
    match value {
        Json::Null => (),
        Json::Object(map) => {
            for (key, value) in map {
                flatten(&name(key), value, facts);
            }
        }
        Json::Array(items) if items.iter().any(|item| item.is_object() || item.is_array()) => {
            for (index, item) in items.iter().enumerate() {
                flatten(&name(&index.to_string()), item, facts);
            }
        }
        Json::Array(items) => facts.push((
            prefix.to_owned(),
            items.iter().map(scalar).collect::<Vec<_>>().join(", "),
        )),
        value => facts.push((prefix.to_owned(), scalar(value))),
    }
}

fn scalar(value: &Json) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

pub fn copy_text(entry: &Entry) -> String {
    serde_json::to_string_pretty(entry).unwrap_or_else(|_| entry.line())
}

#[cfg(test)]
mod tests {
    use super::flatten;
    use serde_json::json;

    #[test]
    fn nested_data_becomes_one_fact_per_value() {
        let mut facts = Vec::new();
        flatten(
            "",
            &json!({"world": {"place": "server", "players": [{"at": [1, 2, 0]}]}, "empty": null}),
            &mut facts,
        );
        assert_eq!(
            facts,
            [
                ("world.place".to_owned(), "server".to_owned()),
                ("world.players.0.at".to_owned(), "1, 2, 0".to_owned()),
            ]
        );
    }
}
