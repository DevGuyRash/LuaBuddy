//! The two scrolling surfaces: a list of rows and a page of text.

use super::{
    kit::{self, Color, Font, Painter, Theme},
    model::{Block, Tone},
};
use crate::lua::{Context, Result, Value};

const PAD: f64 = 8.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Face {
    Header {
        folded: bool,
        count: usize,
    },
    /// A quiet row's second line only describes it; otherwise it states a condition.
    Item {
        status: String,
        tone: Tone,
        quiet: bool,
    },
    /// A plain sentence that can't be selected, such as an empty result.
    Note,
    /// Asks for more results when selected.
    More,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row<K> {
    pub key: K,
    pub text: String,
    pub face: Face,
}

impl<K> Row<K> {
    fn selectable(&self) -> bool {
        matches!(
            self.face,
            Face::Item { .. } | Face::More | Face::Header { .. }
        )
    }
}

fn row_height<K>(row: &Row<K>) -> f64 {
    match row.face {
        Face::Header { .. } => kit::line_height(Font::Small) + 12.0,
        Face::Item { .. } => kit::line_height(Font::Medium) + kit::line_height(Font::Small) + 10.0,
        Face::Note | Face::More => kit::line_height(Font::Small) + 16.0,
    }
}

pub struct List<K> {
    pub panel: Value,
    pub rows: Vec<Row<K>>,
    pub selected: Option<K>,
    hover: Option<usize>,
    content: f64,
    /// Where each row starts, so drawing can begin at the first visible one.
    tops: Vec<f64>,
}

impl<K: Clone + PartialEq> List<K> {
    pub fn new(panel: Value) -> Self {
        Self {
            panel,
            rows: Vec::new(),
            selected: None,
            hover: None,
            content: 0.0,
            tops: Vec::new(),
        }
    }

    /// Replaces the rows, keeping the selection and scroll position when possible.
    pub fn show(&mut self, context: &mut Context<'_, '_>, rows: Vec<Row<K>>) -> Result<()> {
        if self.rows == rows {
            return Ok(());
        }
        self.rows = rows;
        self.hover = None;
        self.tops = self
            .rows
            .iter()
            .scan(0.0, |top, row| {
                let start = *top;
                *top += row_height(row);
                Some(start)
            })
            .collect();
        self.content = self.rows.iter().map(row_height).sum();
        set_scroll_height(context, &self.panel, self.content)?;
        let scroll = kit::ask_number(context, &self.panel, "getYScroll")?;
        scroll_to(context, &self.panel, self.content, scroll)
    }

    pub fn index_of(&self, key: &K) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.selectable() && &row.key == key)
    }

    pub fn row_at(&self, y: f64) -> Option<usize> {
        let mut top = 0.0;
        for (index, row) in self.rows.iter().enumerate() {
            let bottom = top + row_height(row);
            if (top..bottom).contains(&y) {
                return Some(index);
            }
            top = bottom;
        }
        None
    }

    /// The neighboring selectable row, wrapping at both ends.
    pub fn neighbor(&self, forward: bool) -> Option<usize> {
        let count = self.rows.len();
        let start = self
            .selected
            .as_ref()
            .and_then(|key| self.index_of(key))
            .unwrap_or(if forward { count.saturating_sub(1) } else { 0 });
        (1..=count)
            .map(|step| {
                if forward {
                    (start + step) % count
                } else {
                    (start + count - step % count) % count
                }
            })
            .find(|index| self.rows[*index].selectable())
    }

    pub fn reveal(&self, context: &mut Context<'_, '_>, index: usize) -> Result<()> {
        let top: f64 = self.rows[..index].iter().map(row_height).sum();
        let bottom = top + row_height(&self.rows[index]);
        let height = kit::ask_number(context, &self.panel, "getHeight")?;
        let scroll = -kit::ask_number(context, &self.panel, "getYScroll")?;
        if top < scroll {
            scroll_to(context, &self.panel, self.content, -top)
        } else if bottom > scroll + height {
            scroll_to(context, &self.panel, self.content, height - bottom)
        } else {
            Ok(())
        }
    }

    /// Whether the last row is in view, so further results can be requested.
    pub fn at_end(&self, context: &mut Context<'_, '_>) -> Result<bool> {
        let height = kit::ask_number(context, &self.panel, "getHeight")?;
        let scroll = -kit::ask_number(context, &self.panel, "getYScroll")?;
        Ok(scroll + height >= self.content - 1.0)
    }

    pub fn point(&mut self, y: Option<f64>) {
        self.hover = y.and_then(|y| self.row_at(y));
    }

    pub fn wheel(&self, context: &mut Context<'_, '_>, steps: f64) -> Result<()> {
        wheel(context, &self.panel, self.content, steps)
    }

    /// Draws visible rows only. Cost doesn't depend on the list's length.
    pub fn draw(&self, context: &mut Context<'_, '_>, theme: &Theme) -> Result<()> {
        let Frame {
            width,
            height,
            scroll,
            inner,
        } = frame(context, &self.panel, self.content)?;
        let mut painter = Painter::new(context, &self.panel)?;
        painter.rect(0.0, -scroll, width, height, kit::PANEL)?;
        painter.border(0.0, -scroll, width, height, kit::BORDER)?;
        painter.clip(1.0, 1.0, inner - 2.0, height - 2.0)?;
        let drawn = self.draw_rows(&mut painter, theme, inner, -scroll, height);
        painter.unclip()?;
        drawn
    }

    fn draw_rows(
        &self,
        painter: &mut Painter<'_, '_, '_>,
        theme: &Theme,
        width: f64,
        top: f64,
        height: f64,
    ) -> Result<()> {
        let first = self
            .tops
            .partition_point(|start| *start <= top)
            .saturating_sub(1);
        for (index, row) in self.rows.iter().enumerate().skip(first) {
            let y = self.tops[index];
            let tall = row_height(row);
            if y > top + height {
                break;
            }
            if y + tall >= top {
                let selected = row.selectable() && self.selected.as_ref() == Some(&row.key);
                let hovered = self.hover == Some(index) && row.face != Face::Note;
                draw_row(
                    painter,
                    theme,
                    row,
                    [0.0, y, width, tall],
                    selected,
                    hovered,
                )?;
            }
        }
        Ok(())
    }
}

fn draw_row<K>(
    painter: &mut Painter<'_, '_, '_>,
    theme: &Theme,
    row: &Row<K>,
    [x, y, w, h]: [f64; 4],
    selected: bool,
    hovered: bool,
) -> Result<()> {
    if selected {
        painter.rect(x, y, w, h, kit::SELECTED)?;
    } else if hovered {
        painter.rect(x, y, w, h, kit::HOVER)?;
    }
    let room = w - 2.0 * PAD;
    match &row.face {
        Face::Header { folded, count } => {
            painter.rect(x, y, w, h, [1.0, 1.0, 1.0, 0.05])?;
            let label = format!(
                "{} {}  ({count})",
                if *folded { "+" } else { "-" },
                row.text
            );
            painter.text(Font::Small, x + PAD, y + 6.0, &label, kit::MUTED)?;
        }
        Face::Item {
            status,
            tone,
            quiet,
        } => {
            let color = theme.tone(*tone);
            painter.rect(x + 1.0, y + 1.0, 3.0, h - 2.0, color)?;
            let left = x + PAD + 5.0;
            let title = kit::fit(painter.context, Font::Medium, &row.text, room - 5.0)?;
            let title_color = if *tone == Tone::Muted {
                kit::MUTED
            } else {
                kit::TEXT
            };
            painter.text(Font::Medium, left, y + 4.0, &title, title_color)?;
            let status = kit::fit(painter.context, Font::Small, status, room - 5.0)?;
            painter.text(
                Font::Small,
                left,
                y + 5.0 + kit::line_height(Font::Medium),
                &status,
                if *quiet { kit::MUTED } else { color },
            )?;
        }
        Face::Note | Face::More => {
            let color = if row.face == Face::More {
                kit::WAITING
            } else {
                kit::MUTED
            };
            let text = kit::fit(painter.context, Font::Small, &row.text, room)?;
            painter.text(Font::Small, x + PAD, y + 8.0, &text, color)?;
        }
    }
    Ok(())
}

struct Line {
    x: f64,
    y: f64,
    font: Font,
    color: Color,
    text: String,
}

pub struct Page {
    pub panel: Value,
    blocks: Vec<Block>,
    lines: Vec<Line>,
    picture: Option<Value>,
    laid_out: f64,
    content: f64,
}

const PICTURE: f64 = 112.0;

impl Page {
    pub fn new(panel: Value) -> Self {
        Self {
            panel,
            blocks: Vec::new(),
            lines: Vec::new(),
            picture: None,
            laid_out: 0.0,
            content: 0.0,
        }
    }

    /// Shows new text. The scroll position resets only for a different subject.
    pub fn show(
        &mut self,
        context: &mut Context<'_, '_>,
        theme: &Theme,
        blocks: Vec<Block>,
        picture: Option<Value>,
    ) -> Result<()> {
        if self.blocks == blocks && self.picture.is_some() == picture.is_some() {
            return Ok(());
        }
        let same_subject = self.blocks.first() == blocks.first();
        self.blocks = blocks;
        self.picture = picture;
        self.lay_out(context, theme)?;
        let scroll = if same_subject {
            kit::ask_number(context, &self.panel, "getYScroll")?
        } else {
            0.0
        };
        scroll_to(context, &self.panel, self.content, scroll)
    }

    /// Moves the page, laying its text out again only when its width changes.
    pub fn resize(
        &mut self,
        context: &mut Context<'_, '_>,
        theme: &Theme,
        bounds: [f64; 4],
    ) -> Result<()> {
        place(context, &self.panel, bounds)?;
        if self.laid_out != bounds[2] {
            self.lay_out(context, theme)?;
        }
        let scroll = kit::ask_number(context, &self.panel, "getYScroll")?;
        scroll_to(context, &self.panel, self.content, scroll)
    }

    fn lay_out(&mut self, context: &mut Context<'_, '_>, theme: &Theme) -> Result<()> {
        let width = kit::ask_number(context, &self.panel, "getWidth")?;
        self.laid_out = width;
        let room = width - 2.0 * PAD - 18.0;
        self.lines.clear();
        let mut y = PAD;
        let beside_picture = |y: f64| {
            if self.picture.is_some() && y < PICTURE + PAD {
                room - PICTURE - PAD
            } else {
                room
            }
        };
        let mut facts = 0.0_f64;
        for block in &self.blocks {
            if let Block::Fact(name, ..) = block {
                facts = facts.max(kit::width(context, Font::Small, name)?);
            }
        }
        for block in &self.blocks {
            let (font, color, indent, text, before) = match block {
                Block::Title(text) => (Font::Large, [1.0; 4], 0.0, text, 0.0),
                Block::Status(tone, text) => (Font::Medium, theme.tone(*tone), 0.0, text, 2.0),
                Block::Text(tone, text) => (Font::Small, theme.tone(*tone), 0.0, text, 4.0),
                Block::Heading(text) => {
                    if self.picture.is_some() && y < PICTURE + PAD {
                        y = PICTURE + PAD;
                    }
                    (Font::Medium, kit::WARNING, 0.0, text, 12.0)
                }
                Block::Fact(name, tone, value) => {
                    self.lines.push(Line {
                        x: PAD,
                        y: y + 2.0,
                        font: Font::Small,
                        color: kit::MUTED,
                        text: name.clone(),
                    });
                    (
                        Font::Small,
                        theme.tone(*tone),
                        (facts + 16.0).min(room / 2.0),
                        value,
                        2.0,
                    )
                }
            };
            y += before;
            let limit = (beside_picture(y) - indent).max(60.0);
            for text in kit::wrap(context, font, text, limit)? {
                self.lines.push(Line {
                    x: PAD + indent,
                    y,
                    font,
                    color,
                    text,
                });
                y += kit::line_height(font) + 1.0;
            }
        }
        self.content = y + PAD;
        set_scroll_height(context, &self.panel, self.content)
    }

    pub fn wheel(&self, context: &mut Context<'_, '_>, steps: f64) -> Result<()> {
        wheel(context, &self.panel, self.content, steps)
    }

    pub fn draw(&self, context: &mut Context<'_, '_>) -> Result<()> {
        let Frame {
            width,
            height,
            scroll,
            inner,
        } = frame(context, &self.panel, self.content)?;
        let mut painter = Painter::new(context, &self.panel)?;
        painter.rect(0.0, -scroll, width, height, kit::PANEL)?;
        painter.border(0.0, -scroll, width, height, kit::BORDER)?;
        painter.clip(1.0, 1.0, inner - 2.0, height - 2.0)?;
        let drawn = (|| -> Result<()> {
            if let Some(picture) = &self.picture {
                painter.picture(
                    picture,
                    width - PICTURE - PAD - 18.0,
                    PAD,
                    PICTURE,
                    PICTURE,
                    1.0,
                )?;
            }
            for line in &self.lines {
                if line.y + kit::line_height(line.font) < -scroll || line.text.is_empty() {
                    continue;
                }
                if line.y > height - scroll {
                    break;
                }
                painter.text(line.font, line.x, line.y, &line.text, line.color)?;
            }
            Ok(())
        })();
        painter.unclip()?;
        drawn
    }
}

/// Moves a scrolling panel and its scrollbar together. The scrollbar is created
/// for the panel's first size and does not reliably follow later ones.
pub fn place(context: &mut Context<'_, '_>, panel: &Value, bounds: [f64; 4]) -> Result<()> {
    kit::set_bounds(context, panel, bounds)?;
    let bar = context.get(panel, "vscroll")?;
    if bar.is_nil() {
        return Ok(());
    }
    for method in ["setAnchorRight", "setAnchorBottom"] {
        kit::call_flag(context, &bar, method, false)?;
    }
    let width = kit::ask_number(context, &bar, "getWidth")?;
    kit::set_bounds(context, &bar, [bounds[2] - width, 0.0, width, bounds[3]])
}

struct Frame {
    width: f64,
    height: f64,
    scroll: f64,
    /// Width left of a visible scrollbar.
    inner: f64,
}

fn frame(context: &mut Context<'_, '_>, panel: &Value, content: f64) -> Result<Frame> {
    let width = kit::ask_number(context, panel, "getWidth")?;
    let height = kit::ask_number(context, panel, "getHeight")?;
    let scroll = kit::ask_number(context, panel, "getYScroll")?;
    Ok(Frame {
        width,
        height,
        scroll,
        inner: if content > height {
            width - 14.0
        } else {
            width
        },
    })
}

fn set_scroll_height(context: &mut Context<'_, '_>, panel: &Value, content: f64) -> Result<()> {
    let height = context.number(content)?;
    context.invoke(panel, "setScrollHeight", &[height])?;
    Ok(())
}

fn scroll_to(
    context: &mut Context<'_, '_>,
    panel: &Value,
    content: f64,
    scroll: f64,
) -> Result<()> {
    let height = kit::ask_number(context, panel, "getHeight")?;
    let scroll = scroll.clamp((height - content).min(0.0), 0.0);
    let scroll = context.number(scroll.round())?;
    context.invoke(panel, "setYScroll", &[scroll])?;
    Ok(())
}

fn wheel(context: &mut Context<'_, '_>, panel: &Value, content: f64, steps: f64) -> Result<()> {
    let scroll = kit::ask_number(context, panel, "getYScroll")?;
    scroll_to(context, panel, content, scroll - steps * 48.0)
}
