//! Turns upstream descriptions into plain readable paragraphs.

/// Removes the game's rich-text commands and Workshop BBCode, keeping every
/// word, list item, table cell and link target. Unknown tags stay literal.
pub fn readable(text: &str) -> String {
    let text = separate(&bbcode(&native(text)));
    let mut result = String::new();
    let mut blank = true;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !blank {
                result.push('\n');
            }
            blank = true;
            continue;
        }
        result.push_str(line);
        result.push('\n');
        blank = false;
    }
    result.trim_end().to_owned()
}

/// The game joins a mod's description lines without anything between them, so
/// one sentence ends where the next begins: "done.Next". Breaks them apart.
fn separate(text: &str) -> String {
    let characters = text.chars().collect::<Vec<_>>();
    let mut result = String::with_capacity(text.len());
    for (index, ch) in characters.iter().enumerate() {
        result.push(*ch);
        let before = index.checked_sub(1).and_then(|index| characters.get(index));
        if !matches!(ch, '.' | '!' | '?')
            || !before.is_some_and(|ch| ch.is_lowercase() || *ch == ')')
        {
            continue;
        }
        // A capitalized word of three letters or more; "Node.Js" stays whole.
        let mut word = characters[index + 1..]
            .iter()
            .take_while(|ch| ch.is_alphabetic());
        let joined = word.next().is_some_and(|ch| ch.is_uppercase()) && {
            let mut rest = 0;
            word.all(|ch| {
                rest += 1;
                ch.is_lowercase()
            }) && rest >= 2
        };
        if joined {
            result.push('\n');
        }
    }
    result
}

fn native(text: &str) -> String {
    const COMMANDS: &[&str] = &[
        "LINE",
        "BR",
        "H1",
        "H2",
        "TEXT",
        "CENTRE",
        "LEFT",
        "RIGHT",
        "RGB",
        "PUSHRGB",
        "POPRGB",
        "RED",
        "GREEN",
        "ORANGE",
        "SIZE",
        "IMAGE",
        "IMAGECENTRE",
        "VIDEOCENTRE",
        "SETX",
        "INDENT",
        "SPACE",
        "JOYPAD",
        "KEY",
        "GHC",
        "BHC",
        "FONT",
    ];
    let mut result = String::new();
    let mut remaining = text;
    while let Some(start) = remaining.find('<') {
        result.push_str(&remaining[..start]);
        remaining = &remaining[start..];
        let Some(end) = remaining.find('>') else {
            break;
        };
        let name = remaining[1..end].split(':').next().unwrap_or("").trim();
        if COMMANDS.contains(&name) {
            match name {
                "LINE" | "BR" | "H1" | "H2" | "CENTRE" => result.push('\n'),
                "SPACE" => result.push(' '),
                _ => (),
            }
        } else {
            result.push_str(&remaining[..=end]);
        }
        remaining = &remaining[end + 1..];
    }
    result.push_str(remaining);
    result
}

/// Starts a new line unless one has just started. Blank lines stay where the
/// author wrote them, not wherever two tags meet.
fn soft_break(text: &mut String) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
}

fn bbcode(text: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;
    let mut links = Vec::<(String, usize)>::new();
    let mut cell_closed = false;
    while let Some(start) = remaining.find('[') {
        result.push_str(&remaining[..start]);
        remaining = &remaining[start..];
        let Some(end) = remaining.find(']') else {
            break;
        };
        let raw = &remaining[1..end];
        let tag = raw.to_ascii_lowercase();
        let mut skip_to = None;
        let recognized = match tag.split('=').next().unwrap_or("") {
            // The picture itself can't be shown; its address is noise.
            "img" => {
                skip_to = remaining.find("[/img]").map(|close| close + "[/img]".len());
                true
            }
            "url" => {
                let target = raw
                    .split_once('=')
                    .map(|(_, target)| target.trim_matches([' ', '"', '\'']))
                    .unwrap_or("");
                links.push((target.to_owned(), result.len()));
                true
            }
            "/url" => {
                if let Some((target, start)) = links.pop()
                    && !target.is_empty()
                    && result.get(start..).map(str::trim) != Some(target.as_str())
                {
                    result.push_str(&format!(" ({target})"));
                }
                true
            }
            "h1" | "h2" | "h3" | "/h1" | "/h2" | "/h3" | "list" | "/list" | "olist" | "/olist"
            | "quote" | "/quote" | "code" | "/code" | "table" | "/table" | "hr" | "/hr"
            | "/img" => {
                soft_break(&mut result);
                true
            }
            "tr" | "/tr" => {
                cell_closed = false;
                soft_break(&mut result);
                true
            }
            "td" | "th" => {
                if cell_closed {
                    result.push_str(" | ");
                }
                cell_closed = false;
                true
            }
            "/td" | "/th" => {
                cell_closed = true;
                true
            }
            "*" => {
                soft_break(&mut result);
                result.push_str("- ");
                true
            }
            "b" | "/b" | "i" | "/i" | "u" | "/u" | "s" | "/s" | "strike" | "/strike"
            | "spoiler" | "/spoiler" | "noparse" | "/noparse" | "previewyoutube"
            | "/previewyoutube" => true,
            _ => false,
        };
        if !recognized {
            result.push_str(&remaining[..=end]);
        }
        remaining = &remaining[skip_to.unwrap_or(end + 1)..];
    }
    result.push_str(remaining);
    for (target, _) in links {
        if !target.is_empty() {
            result.push_str(&format!(" ({target})"));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::readable;

    #[test]
    fn workshop_markup_keeps_text_lists_tables_and_link_targets() {
        assert_eq!(
            readable(
                "[h1]Guide[/h1][b]Use[/b] [url=https://example.test]the original[/url][list][*]First[*]Second[/list][table][tr][td]Speed[/td][td]42[/td][/tr][/table][img]https://example.test/banner.png[/img][B42]中文"
            ),
            "Guide\nUse the original (https://example.test)\n- First\n- Second\nSpeed | 42\n[B42]中文"
        );
    }

    #[test]
    fn game_rich_text_becomes_lines_and_unknown_tags_stay_literal() {
        assert_eq!(
            readable("One<LINE>Two <RGB:1,0,0>red <SIZE:large>big <notatag> [unfinished"),
            "One\nTwo red big <notatag> [unfinished"
        );
    }

    #[test]
    fn sentences_the_game_joined_are_separated_but_names_with_dots_are_not() {
        assert_eq!(
            readable("Cooks a meal.Auto Read: reads books. Uses Node.Js and B42.Fixes"),
            "Cooks a meal.\nAuto Read: reads books. Uses Node.Js and B42.Fixes"
        );
    }
}
