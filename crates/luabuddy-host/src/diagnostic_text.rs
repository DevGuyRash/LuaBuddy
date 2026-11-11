use serde_json::Value;
use std::fmt::Write;

#[derive(Clone, Copy)]
enum Token {
    Key,
    String,
    Number,
    Literal,
    Null,
    Punctuation,
}

impl Token {
    fn color(self) -> &'static str {
        match self {
            Self::Key => "0.55,0.78,1",
            Self::String => "0.65,0.88,0.62",
            Self::Number => "1,0.8,0.5",
            Self::Literal => "0.82,0.65,1",
            Self::Null => "0.95,0.62,0.62",
            Self::Punctuation => "0.85,0.85,0.85",
        }
    }
}

pub(crate) fn rich(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(value @ (Value::Object(_) | Value::Array(_))) => structured(&value),
        _ => {
            let mut result = String::new();
            for (index, line) in text.split('\n').enumerate() {
                if index != 0 {
                    result.push_str(" <LINE> ");
                }
                let columns: usize = line
                    .chars()
                    .take_while(|character| matches!(character, ' ' | '\t'))
                    .map(|character| if character == '\t' { 4 } else { 1 })
                    .sum();
                let pixels = columns * 6;
                write!(result, " <INDENT:{pixels}> <SETX:{pixels}> ")
                    .expect("writing to a String cannot fail");
                result.push_str(
                    &line
                        .trim_start_matches([' ', '\t'])
                        .replace('<', "&lt;")
                        .replace('>', "&gt;"),
                );
            }
            result
        }
    }
}

fn structured(value: &Value) -> String {
    let pretty = serde_json::to_string_pretty(value)
        .expect("a serde_json::Value always serializes as valid JSON");
    let mut result = String::new();
    for (index, line) in pretty.lines().enumerate() {
        if index != 0 {
            result.push_str(" <LINE> ");
        }
        let indentation = line.len() - line.trim_start().len();
        let pixels = indentation * 6;
        write!(result, " <INDENT:{pixels}> <SETX:{pixels}> ")
            .expect("writing to a String cannot fail");
        let mut rest = line.trim_start();
        while !rest.is_empty() {
            let bytes = rest.as_bytes();
            if bytes[0].is_ascii_whitespace() {
                result.push_str(" <SPACE> ");
                rest = &rest[1..];
                continue;
            }
            let (length, kind) = if bytes[0] == b'"' {
                let mut end = 1;
                while end < bytes.len() {
                    match bytes[end] {
                        b'\\' => end += 2,
                        b'"' => {
                            end += 1;
                            break;
                        }
                        _ => end += 1,
                    }
                }
                let kind = if rest[end..].trim_start().starts_with(':') {
                    Token::Key
                } else {
                    Token::String
                };
                (end, kind)
            } else if b"{}[],:".contains(&bytes[0]) {
                (1, Token::Punctuation)
            } else {
                let end = bytes
                    .iter()
                    .position(|byte| byte.is_ascii_whitespace() || b"{}[],:".contains(byte))
                    .unwrap_or(bytes.len());
                let kind = match &rest[..end] {
                    "true" | "false" => Token::Literal,
                    "null" => Token::Null,
                    _ => Token::Number,
                };
                (end, kind)
            };
            write!(result, "<RGB:{}>", kind.color()).expect("writing to a String cannot fail");
            result.push_str(
                &rest[..length]
                    .replace('&', "\\u0026")
                    .replace('<', "\\u003c")
                    .replace('>', "\\u003e")
                    .replace("  ", "\\u0020\\u0020"),
            );
            rest = &rest[length..];
        }
    }
    result.push_str(" <RGB:1,1,1> <INDENT:0> ");
    result
}

#[cfg(test)]
mod tests {
    use super::rich;
    use serde_json::{Value, json};
    use std::collections::BTreeSet;

    fn visible_json(rendered: &str) -> Value {
        let mut text = String::new();
        let mut rest = rendered;
        while let Some(start) = rest.find('<') {
            text.push_str(&rest[..start]);
            let end = rest[start..]
                .find('>')
                .expect("generated command is closed")
                + start;
            let command = &rest[start + 1..end];
            if matches!(command, "LINE" | "SPACE") {
                text.push(' ');
            } else {
                assert!(
                    command.starts_with("RGB:")
                        || command.starts_with("INDENT:")
                        || command.starts_with("SETX:"),
                    "unexpected rich-text command {command}"
                );
            }
            rest = &rest[end + 1..];
        }
        text.push_str(rest);
        serde_json::from_str(&text).expect("rendered text retains its JSON meaning")
    }

    #[test]
    fn nested_diagnostics_keep_data_and_distinguish_syntax_roles() {
        let value = json!({"target":{"id":"example", "enabled":true, "error":null, "durationMs":-12.5}, "hooks":["before", "after"]});
        let rendered = rich(&value.to_string());
        assert_eq!(visible_json(&rendered), value);
        assert!(rendered.contains("<INDENT:12>") && rendered.contains("<INDENT:24>"));
        let color = |token: &str| {
            let prefix = &rendered[..rendered.find(token).expect("diagnostic token is present")];
            let start = prefix.rfind("<RGB:").expect("syntax token has a color") + 5;
            prefix[start..].split('>').next().unwrap().to_owned()
        };
        let colors = ["\"id\"", "\"example\"", "true", "null", "-12.5", "{"]
            .into_iter()
            .map(color)
            .collect::<BTreeSet<_>>();
        assert_eq!(colors.len(), 6);
    }

    #[test]
    fn diagnostic_content_cannot_inject_markup_or_lose_escaped_characters() {
        let value = json!({"<IMAGE:secret>":"<LINE> <RGB:1,0,0> &lt;  \"quoted\"\nλ🎮\\path", "nested":[false, {}, []]});
        let rendered = rich(&value.to_string());
        assert_eq!(visible_json(&rendered), value);
        assert!(!rendered.contains("<IMAGE:") && !rendered.contains("<RGB:1,0,0>"));
        assert!(rendered.contains("\\u003c") && rendered.contains("\\u0026lt;"));
    }

    #[test]
    fn prose_and_traceback_indentation_remain_readable_without_syntax_colors() {
        let rendered = rich("Load failed <module>\n    Lua traceback:\n\tframe.lua:7\n");
        assert!(rendered.contains("Load failed &lt;module&gt;"));
        assert_eq!(rendered.matches("<INDENT:24>").count(), 2);
        assert_eq!(rendered.matches("<LINE>").count(), 3);
        assert!(!rendered.contains("<RGB:"));
        assert!(!rich("{broken json").contains("<RGB:"));
    }
}
