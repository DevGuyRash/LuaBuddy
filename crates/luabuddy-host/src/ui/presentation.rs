use super::*;

pub(super) const TEXT: &str = "0.88,0.90,0.92";
pub(super) const MUTED: &str = "0.65,0.69,0.73";
pub(super) const ACTION: &str = "0.45,0.77,1";
pub(super) const SUCCESS: &str = "0.50,0.89,0.63";
pub(super) const WARNING: &str = "0.98,0.78,0.39";
pub(super) const ERROR: &str = "1,0.49,0.47";

const LABELS: &[(Action, &str)] = &[
    (Action::Open, "Manage mods"),
    (Action::Refresh, "Load new profiles"),
    (Action::Reload, "Reload a profile"),
    (Action::Search, "Workshop search"),
    (Action::Installed, "Installed"),
    (Action::Subscribed, "Subscribed"),
    (Action::Add, "Subscribe + download"),
    (Action::Download, "Download / update"),
    (Action::Enable, "Enable late"),
    (Action::Disable, "Disable late"),
    (Action::Load, "Load enabled"),
    (Action::Cancel, "Cancel query"),
    (Action::Close, "Close (Esc)"),
    (Action::Restart, "Restart game..."),
    (Action::Previous, "< Previous"),
    (Action::Next, "Next >"),
    (Action::StartupEnable, "Startup enable"),
    (Action::StartupDisable, "Startup disable"),
    (Action::PatchDiagnostics, "Patch diagnostics"),
    (Action::ApplyApprovedPatches, "Apply approved patches"),
    (Action::Help, "Help & guides"),
    (Action::OpenWorkshop, "Open Workshop"),
    (Action::CopyWorkshopLink, "Copy link"),
    (Action::GuideScrollUp, "Scroll up"),
    (Action::GuideScrollDown, "Scroll down"),
];

pub(super) fn action_label(action: Action) -> Option<&'static str> {
    LABELS
        .iter()
        .find_map(|(candidate, label)| ((*candidate as u8) == action as u8).then_some(*label))
}

pub(super) fn escape(text: &str) -> String {
    text.replace('<', "&lt;").replace('>', "&gt;")
}

pub(super) fn instructions(text: &str) -> String {
    text.split('\n')
        .map(instruction_line)
        .collect::<Vec<_>>()
        .join(" <LINE> ")
}

fn instruction_line(text: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        let next = LABELS
            .iter()
            .filter_map(|(_, label)| {
                remaining.match_indices(*label).find_map(|(index, _)| {
                    let before = remaining[..index].chars().next_back();
                    let after = remaining[index + label.len()..].chars().next();
                    let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
                    if before.is_some_and(is_word) || after.is_some_and(is_word) {
                        None
                    } else {
                        Some((index, *label))
                    }
                })
            })
            .min_by_key(|(index, label)| (*index, std::cmp::Reverse(label.len())));
        let Some((index, label)) = next else {
            inline(&mut result, remaining);
            break;
        };
        inline(&mut result, &remaining[..index]);
        result.push_str(&format!(" <RGB:{ACTION}> {} <RGB:{TEXT}> ", escape(label)));
        remaining = &remaining[index + label.len()..];
    }
    result
}

fn inline(result: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    if text.starts_with(char::is_whitespace) && !result.is_empty() {
        result.push_str(" <SPACE> ");
    }
    result.push_str(&escape(text.trim()).replace('\n', " <LINE> "));
    if text.ends_with(char::is_whitespace) {
        result.push_str(" <SPACE> ");
    }
}

pub(super) enum Block {
    Title(String, &'static str),
    Section(String),
    Text(String),
    Instruction(String),
    Field(String, String, &'static str),
}

#[derive(Default)]
pub(super) struct Document {
    blocks: Vec<Block>,
}

impl Document {
    pub(super) fn title(&mut self, text: impl Into<String>) {
        self.blocks.push(Block::Title(text.into(), "1,1,1"));
    }
    pub(super) fn heading(&mut self, text: impl Into<String>, color: &'static str) {
        self.blocks.push(Block::Title(text.into(), color));
    }
    pub(super) fn section(&mut self, text: impl Into<String>) {
        self.blocks.push(Block::Section(text.into()));
    }
    pub(super) fn text(&mut self, text: impl Into<String>) {
        self.blocks.push(Block::Text(text.into()));
    }
    pub(super) fn instruction(&mut self, text: impl Into<String>) {
        self.blocks.push(Block::Instruction(text.into()));
    }
    pub(super) fn field(
        &mut self,
        name: impl Into<String>,
        value: impl Into<String>,
        color: &'static str,
    ) {
        self.blocks
            .push(Block::Field(name.into(), value.into(), color));
    }
    pub(super) fn render(&self, context: &mut Context<'_, '_>, panel: &Value) -> Result<String> {
        let width = reading_width(context, panel)?;
        let font = context.get(panel, "defaultFont")?;
        let mut result = String::new();
        for block in &self.blocks {
            match block {
                Block::Title(text, color) => {
                    let heading_font = context.jni(|env| {
                        let value = env
                            .get_static_field("zombie/ui/UIFont", "Medium", "Lzombie/ui/UIFont;")?
                            .l()?;
                        env.new_global_ref(value).map(|value| Value(Some(value)))
                    })?;
                    let text = break_long_tokens(context, text, &heading_font, width)?;
                    result.push_str(&format!(
                        " <INDENT:0> <SETX:0> <H2> <RGB:{color}> {} <LINE> <TEXT> <RGB:{TEXT}> ",
                        escape(&text).replace('\n', " <LINE> ")
                    ));
                }
                Block::Section(text) => {
                    let text = break_long_tokens(context, text, &font, width)?;
                    result.push_str(&format!(
                        " <LINE> <INDENT:0> <SETX:0> <RGB:{ACTION}> {} <LINE> <RGB:{TEXT}> ",
                        escape(&text).replace('\n', " <LINE> ")
                    ));
                }
                Block::Text(text) | Block::Instruction(text) => {
                    let text = break_long_tokens(context, text, &font, width)?;
                    let text = flow_lines(&text, matches!(block, Block::Instruction(_)));
                    result.push_str(&format!(" <RGB:{TEXT}> {text} <LINE> "));
                }
                Block::Field(name, value, color) => {
                    let value = break_long_tokens(context, value, &font, width)?;
                    result.push_str(&format!(" <INDENT:0> <SETX:0> <RGB:{MUTED}> {} <SPACE> <RGB:{color}> {} <RGB:{TEXT}> <LINE> ", escape(name), escape(&value).replace('\n', " <LINE> ")));
                }
            }
        }
        Ok(result)
    }
}

pub(super) fn reading_width(context: &mut Context<'_, '_>, panel: &Value) -> Result<f64> {
    let area = first(context.invoke(panel, "getScrollAreaWidth", &[])?);
    let area = context.numeric(&area)?;
    let left = context.get(panel, "marginLeft")?;
    let right = context.get(panel, "marginRight")?;
    let font = context.get(panel, "defaultFont")?;
    let sample = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let typical = f64::from(measure(context, &font, sample)?) / sample.len() as f64 * 78.0;
    Ok(
        (area - context.numeric(&left)? - context.numeric(&right)? - 12.0)
            .min(typical)
            .max(24.0),
    )
}

fn measure(context: &mut Context<'_, '_>, font: &Value, text: &str) -> Result<i32> {
    let manager = first(context.call_global("getTextManager", &[])?);
    context.jni(|env| {
        let text = env.new_string(text)?;
        env.call_method(
            manager.object(),
            "MeasureStringX",
            "(Lzombie/ui/UIFont;Ljava/lang/String;)I",
            &[JValue::Object(font.object()), JValue::Object(&text)],
        )?
        .i()
    })
}

fn break_long_tokens(
    context: &mut Context<'_, '_>,
    text: &str,
    font: &Value,
    width: f64,
) -> Result<String> {
    let mut result = String::new();
    for part in text.split_inclusive(char::is_whitespace) {
        let token = part.trim_end_matches(char::is_whitespace);
        let whitespace = &part[token.len()..];
        let mut remaining = token;
        while !remaining.is_empty() && f64::from(measure(context, font, remaining)?) > width {
            let boundaries = remaining
                .char_indices()
                .skip(1)
                .map(|(index, _)| index)
                .chain(std::iter::once(remaining.len()))
                .collect::<Vec<_>>();
            let mut low = 0;
            let mut high = boundaries.len();
            while low + 1 < high {
                let middle = (low + high) / 2;
                if f64::from(measure(context, font, &remaining[..boundaries[middle]])?) <= width {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            let maximum = boundaries[low];
            let preferred = remaining[..maximum]
                .char_indices()
                .filter(|(_, ch)| matches!(ch, '/' | '\\' | '-' | '_' | '?' | '&'))
                .map(|(index, ch)| index + ch.len_utf8())
                .rfind(|index| *index >= maximum / 2)
                .unwrap_or(maximum);
            result.push_str(&remaining[..preferred]);
            result.push('\n');
            remaining = &remaining[preferred..];
        }
        result.push_str(remaining);
        result.push_str(whitespace);
    }
    Ok(result)
}

pub(super) fn plain(
    context: &mut Context<'_, '_>,
    panel: &Value,
    text: &str,
    references: bool,
) -> Result<String> {
    let width = reading_width(context, panel)?;
    let font = context.get(panel, "defaultFont")?;
    let text = break_long_tokens(context, text, &font, width)?;
    Ok(flow_lines(&text, references))
}

fn tooltip_metrics(context: &mut Context<'_, '_>) -> Result<(Value, f64, f64)> {
    let class = context.global("ISToolTip")?;
    let get_font = context.get(&class, "GetFont")?;
    let font = first(context.call(&get_font, &[])?);
    let manager = first(context.call_global("getTextManager", &[])?);
    let height = context.jni(|env| {
        env.call_method(
            manager.object(),
            "getFontHeight",
            "(Lzombie/ui/UIFont;)I",
            &[JValue::Object(font.object())],
        )?
        .i()
    })?;
    let width = (geometry(context, 520.0, 320.0)?[2] - 40.0).max(80.0);
    Ok((font, width, f64::from(height.max(1)) / 15.0))
}

pub(super) fn tooltip(
    context: &mut Context<'_, '_>,
    text: &str,
    references: bool,
) -> Result<String> {
    let (font, width, _) = tooltip_metrics(context)?;
    let text = break_long_tokens(context, text, &font, width)?;
    Ok(flow_lines(&text, references))
}

pub(super) fn tooltip_max_line_width(context: &mut Context<'_, '_>) -> Result<f64> {
    let (_, width, scale) = tooltip_metrics(context)?;
    Ok(width / scale)
}

fn flow_lines(text: &str, references: bool) -> String {
    text.split('\n')
        .map(|line| {
            let indent = line
                .chars()
                .take_while(|ch| matches!(ch, ' ' | '\t'))
                .map(|ch| if ch == '\t' { 24 } else { 6 })
                .sum::<usize>();
            let content = line.trim_start_matches([' ', '\t']);
            let content = if references {
                instructions(content)
            } else {
                escape(content)
            };
            format!(" <INDENT:{indent}> <SETX:{indent}> {content}")
        })
        .collect::<Vec<_>>()
        .join(" <LINE> ")
}

pub(super) fn paragraphs(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            result.push(line.trim_end().to_owned());
            continue;
        }
        let mut start = 0;
        let mut last_break = 0;
        for (index, ch) in line.char_indices() {
            if matches!(ch, '.' | '!' | '?')
                && line[index + ch.len_utf8()..].starts_with(char::is_whitespace)
                && index - start >= 300
            {
                result.push(line[start..index + ch.len_utf8()].trim().to_owned());
                start = index + ch.len_utf8();
                last_break = start;
            }
        }
        if last_break < line.len() {
            let remaining = line[start..].trim();
            if !remaining.is_empty() {
                result.push(remaining.to_owned());
            }
        }
    }
    result
}

pub(super) fn set_document(
    context: &mut Context<'_, '_>,
    panel: &Value,
    document: &Document,
) -> Result<()> {
    let text = document.render(context, panel)?;
    let no = context.boolean(false)?;
    context.set(panel, "luabuddyStructured", &no)?;
    field_text(context, panel, "text", &text)?;
    paginate(context, panel)
}

#[cfg(test)]
mod tests {
    use super::{instructions, paragraphs};

    #[test]
    fn action_references_keep_line_boundaries_and_cannot_inject_native_commands() {
        let rendered = instructions(
            "Choose Enable late.\nUse Load enabled, then Reload a profile. <IMAGE:unsafe>",
        );
        assert_eq!(rendered.matches("<LINE>").count(), 1);
        for label in ["Enable late", "Load enabled", "Reload a profile"] {
            assert!(rendered.contains(label));
        }
        assert!(rendered.contains("<RGB:"));
        assert!(rendered.contains("&lt;IMAGE:unsafe&gt;"));
        assert!(!rendered.contains("<IMAGE:unsafe>"));
        assert!(!instructions("Enable later and InstalledTools").contains("<RGB:"));
    }

    #[test]
    fn long_description_paragraphs_preserve_every_word_in_order() {
        let sentence = "All original words and version 1.2.3 stay visible, including 中文 and λ, while complete sentences form readable paragraphs.";
        let original = std::iter::repeat_n(sentence, 12)
            .collect::<Vec<_>>()
            .join(" ");
        let blocks = paragraphs(&original);
        assert!(blocks.len() > 1);
        assert!(blocks.iter().all(|block| !block.is_empty()));
        assert_eq!(blocks.join(" "), original);
    }

    #[test]
    fn paragraph_layout_preserves_traceback_and_code_indentation() {
        assert_eq!(
            paragraphs("Reason\n    first.lua:7\n\tsecond.lua:8\n"),
            vec!["Reason", "    first.lua:7", "\tsecond.lua:8"]
        );
    }
}
