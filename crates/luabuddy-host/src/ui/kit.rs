//! Drawing and native widgets, shared by every part of the window.

use super::model::Tone;
use crate::{
    game,
    lua::{Context, Result, Value},
};
use jni::{
    JNIEnv,
    objects::{GlobalRef, JMethodID, JObject, JValue},
    signature::{Primitive, ReturnType},
    sys::jvalue,
};
use std::{borrow::Cow, cell::RefCell, collections::HashMap};

pub type Color = [f64; 4];

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Font {
    Small,
    Medium,
    Large,
}

const FONTS: [(Font, &str); 3] = [
    (Font::Small, "Small"),
    (Font::Medium, "Medium"),
    (Font::Large, "Large"),
];

/// Colors that mean something follow the player's own accessibility settings.
#[derive(Clone, Copy)]
pub struct Theme {
    pub good: Color,
    pub bad: Color,
}

pub const TEXT: Color = [0.90, 0.90, 0.90, 1.0];
pub const MUTED: Color = [0.62, 0.62, 0.62, 1.0];
pub const WAITING: Color = [0.55, 0.76, 0.96, 1.0];
pub const WARNING: Color = [0.96, 0.76, 0.36, 1.0];
pub const BORDER: Color = [1.0, 1.0, 1.0, 0.18];
pub const PANEL: Color = [0.05, 0.05, 0.05, 0.75];
pub const SELECTED: Color = [0.70, 0.35, 0.15, 0.38];
pub const HOVER: Color = [1.0, 1.0, 1.0, 0.08];

impl Theme {
    pub fn load(context: &mut Context<'_, '_>) -> Result<Self> {
        let core = first(context.call_global("getCore", &[])?);
        let mut colors = [[0.0; 4]; 2];
        for (slot, method) in colors
            .iter_mut()
            .zip(["getGoodHighlitedColor", "getBadHighlitedColor"])
        {
            *slot = context.jni(|env| {
                env.with_local_frame(4, |env| -> jni::errors::Result<Color> {
                    let color = env
                        .call_method(
                            core.object(),
                            method,
                            "()Lzombie/core/textures/ColorInfo;",
                            &[],
                        )?
                        .l()?;
                    let mut channel = |name: &str| -> jni::errors::Result<f64> {
                        Ok(f64::from(env.call_method(&color, name, "()F", &[])?.f()?))
                    };
                    Ok([channel("getR")?, channel("getG")?, channel("getB")?, 1.0])
                })
            })?;
        }
        Ok(Self {
            good: readable(colors[0]),
            bad: readable(colors[1]),
        })
    }

    pub fn tone(&self, tone: Tone) -> Color {
        match tone {
            Tone::Normal => TEXT,
            Tone::Muted => MUTED,
            Tone::Good => self.good,
            Tone::Waiting => WAITING,
            Tone::Warning => WARNING,
            Tone::Bad => self.bad,
        }
    }
}

/// Lightens a saturated highlight just enough to read as text on black.
fn readable([r, g, b, a]: Color) -> Color {
    let mix = |channel: f64| channel * 0.75 + 0.25;
    [mix(r), mix(g), mix(b), a]
}

struct Native {
    _element: GlobalRef,
    _manager: GlobalRef,
    text: JMethodID,
    rectangle: JMethodID,
    texture: JMethodID,
    stencil: JMethodID,
    clear_stencil: JMethodID,
    measure: JMethodID,
    fonts: [GlobalRef; 3],
    heights: [f64; 3],
    manager: GlobalRef,
}

/// Measurements per font, looked up by the text itself so a hit allocates nothing.
#[derive(Default)]
struct Cache {
    native: Option<Native>,
    widths: [HashMap<String, f64>; 3],
    /// The last shortening of each text, and the room it was made for.
    fitted: [HashMap<String, (f64, String)>; 3],
}

thread_local! { static CACHE: RefCell<Cache> = RefCell::default(); }

/// Prepares drawing for the current font settings.
pub fn prepare(context: &mut Context<'_, '_>) -> Result<()> {
    let manager = first(context.call_global("getTextManager", &[])?);
    let native = context.jni(|env| native(env, manager.object()))?;
    CACHE.with_borrow_mut(|cache| {
        *cache = Cache {
            native: Some(native),
            widths: Default::default(),
            fitted: Default::default(),
        };
    });
    Ok(())
}

fn native(env: &mut JNIEnv<'_>, manager: &JObject<'_>) -> jni::errors::Result<Native> {
    let element = env.find_class("zombie/ui/UIElement")?;
    let manager_class = env.find_class("zombie/ui/TextManager")?;
    let mut fonts = Vec::new();
    let mut heights = [0.0; 3];
    for (index, (_, name)) in FONTS.into_iter().enumerate() {
        let font = env
            .get_static_field("zombie/ui/UIFont", name, "Lzombie/ui/UIFont;")?
            .l()?;
        heights[index] = f64::from(
            env.call_method(
                manager,
                "getFontHeight",
                "(Lzombie/ui/UIFont;)I",
                &[JValue::Object(&font)],
            )?
            .i()?,
        );
        fonts.push(env.new_global_ref(font)?);
    }
    let fonts = fonts
        .try_into()
        .map_err(|_| jni::errors::Error::NullPtr("UIFont"))?;
    Ok(Native {
        text: env.get_method_id(
            &element,
            "DrawText",
            "(Lzombie/ui/UIFont;Ljava/lang/String;DDDDDD)V",
        )?,
        rectangle: env.get_method_id(
            &element,
            "DrawTextureScaledCol",
            "(Lzombie/core/textures/Texture;DDDDDDDD)V",
        )?,
        texture: env.get_method_id(
            &element,
            "DrawTextureScaledAspect",
            "(Lzombie/core/textures/Texture;DDDDDDDD)V",
        )?,
        stencil: env.get_method_id(&element, "setStencilRect", "(DDDD)V")?,
        clear_stencil: env.get_method_id(&element, "clearStencilRect", "()V")?,
        measure: env.get_method_id(
            &manager_class,
            "MeasureStringX",
            "(Lzombie/ui/UIFont;Ljava/lang/String;)I",
        )?,
        fonts,
        heights,
        manager: env.new_global_ref(manager)?,
        _element: env.new_global_ref(element)?,
        _manager: env.new_global_ref(manager_class)?,
    })
}

fn with_native<T>(action: impl FnOnce(&Native) -> Result<T>) -> Result<T> {
    CACHE.with_borrow(|cache| {
        action(
            cache
                .native
                .as_ref()
                .ok_or("Drawing was used before it was prepared")?,
        )
    })
}

pub fn line_height(font: Font) -> f64 {
    CACHE.with_borrow(|cache| {
        cache
            .native
            .as_ref()
            .map_or(16.0, |native| native.heights[font as usize])
    })
}

pub fn control_height() -> f64 {
    line_height(Font::Small) + 10.0
}

pub fn width(context: &mut Context<'_, '_>, font: Font, text: &str) -> Result<f64> {
    if text.is_empty() {
        return Ok(0.0);
    }
    if let Some(width) = CACHE.with_borrow(|cache| cache.widths[font as usize].get(text).copied()) {
        return Ok(width);
    }
    let measured = with_native(|native| {
        context.jni(|env| {
            let text = env.new_string(text)?;
            // SAFETY: The method ID was resolved on TextManager with this exact
            // descriptor, its class is held by a global reference, and both
            // arguments are live references of the declared types.
            let width = unsafe {
                env.call_method_unchecked(
                    &native.manager,
                    native.measure,
                    ReturnType::Primitive(Primitive::Int),
                    &[
                        jvalue {
                            l: native.fonts[font as usize].as_raw(),
                        },
                        jvalue { l: text.as_raw() },
                    ],
                )?
            };
            Ok(f64::from(width.i()?))
        })
    })?;
    CACHE.with_borrow_mut(|cache| {
        let widths = &mut cache.widths[font as usize];
        // Workshop text is unbounded; a fresh start is cheaper than eviction.
        if widths.len() > 20_000 {
            widths.clear();
        }
        widths.insert(text.to_owned(), measured);
    });
    Ok(measured)
}

/// Shortens the text to fit, marking the cut. Text that fits is returned as is.
pub fn fit<'t>(
    context: &mut Context<'_, '_>,
    font: Font,
    text: &'t str,
    limit: f64,
) -> Result<Cow<'t, str>> {
    if width(context, font, text)? <= limit {
        return Ok(Cow::Borrowed(text));
    }
    let limit = limit.round();
    let cached = CACHE.with_borrow(|cache| {
        cache.fitted[font as usize]
            .get(text)
            .filter(|(room, _)| *room == limit)
            .map(|(_, fitted)| fitted.clone())
    });
    if let Some(fitted) = cached {
        return Ok(Cow::Owned(fitted));
    }
    let room = limit - width(context, font, "...")?;
    let kept = longest_prefix(text, |part| Ok(width(context, font, part)? <= room))?;
    let fitted = format!("{}...", text[..kept].trim_end());
    CACHE.with_borrow_mut(|cache| {
        let cache = &mut cache.fitted[font as usize];
        if cache.len() > 4_000 {
            cache.clear();
        }
        cache.insert(text.to_owned(), (limit, fitted.clone()));
    });
    Ok(Cow::Owned(fitted))
}

/// Breaks the text into lines no wider than `limit`, splitting only words that
/// are too long for a line of their own.
pub fn wrap(
    context: &mut Context<'_, '_>,
    font: Font,
    text: &str,
    limit: f64,
) -> Result<Vec<String>> {
    wrap_with(text, limit, |part| width(context, font, part))
}

fn wrap_with(
    text: &str,
    limit: f64,
    mut measure: impl FnMut(&str) -> Result<f64>,
) -> Result<Vec<String>> {
    // A lone space measures as nothing, so it is measured between letters.
    let space = measure("n n")? - 2.0 * measure("n")?;
    let mut lines = Vec::new();
    for source in text.split('\n') {
        let mut line = String::new();
        let mut used = 0.0;
        for word in source.split_whitespace() {
            let mut word = word;
            let mut word_width = measure(word)?;
            if used > 0.0 && used + space + word_width > limit {
                lines.push(std::mem::take(&mut line));
                used = 0.0;
            }
            while word_width > limit && word.chars().count() > 1 {
                let fits = longest_prefix(word, |part| Ok(measure(part)? <= limit))?;
                let kept = separator_before(word, fits);
                lines.push(word[..kept].to_owned());
                word = &word[kept..];
                word_width = measure(word)?;
            }
            if used > 0.0 {
                line.push(' ');
                used += space;
            }
            line.push_str(word);
            used += word_width;
        }
        lines.push(line);
    }
    Ok(lines)
}

/// Where to break a long path or link: after a separator when one is near.
fn separator_before(word: &str, fits: usize) -> usize {
    word[..fits]
        .rfind(['/', '\\', '-', '_', '?', '&'])
        .map(|index| index + 1)
        .filter(|index| *index > fits / 2)
        .unwrap_or(fits)
}

/// The byte length of the longest prefix that fits, at least one character.
fn longest_prefix(text: &str, mut fits: impl FnMut(&str) -> Result<bool>) -> Result<usize> {
    let ends = text
        .char_indices()
        .map(|(index, ch)| index + ch.len_utf8())
        .collect::<Vec<_>>();
    let (mut low, mut high) = (0, ends.len());
    while low + 1 < high {
        let middle = (low + high) / 2;
        if fits(&text[..ends[middle]])? {
            low = middle;
        } else {
            high = middle;
        }
    }
    Ok(ends.get(low).copied().unwrap_or(0))
}

/// Draws into one element during its own render callback.
pub struct Painter<'c, 'a, 'vm> {
    pub context: &'c mut Context<'a, 'vm>,
    element: Value,
}

impl<'c, 'a, 'vm> Painter<'c, 'a, 'vm> {
    pub fn new(context: &'c mut Context<'a, 'vm>, panel: &Value) -> Result<Self> {
        let element = context.get(panel, "javaObject")?;
        if element.is_nil() {
            return Err("A native element was drawn before it existed".into());
        }
        Ok(Self { context, element })
    }

    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, [r, g, b, a]: Color) -> Result<()> {
        let null = std::ptr::null_mut();
        self.void(
            |native| native.rectangle,
            &[
                jvalue { l: null },
                jvalue { d: x },
                jvalue { d: y },
                jvalue { d: w },
                jvalue { d: h },
                jvalue { d: r },
                jvalue { d: g },
                jvalue { d: b },
                jvalue { d: a },
            ],
        )
    }

    pub fn border(&mut self, x: f64, y: f64, w: f64, h: f64, color: Color) -> Result<()> {
        self.rect(x, y, w, 1.0, color)?;
        self.rect(x, y + h - 1.0, w, 1.0, color)?;
        self.rect(x, y + 1.0, 1.0, h - 2.0, color)?;
        self.rect(x + w - 1.0, y + 1.0, 1.0, h - 2.0, color)
    }

    pub fn picture(
        &mut self,
        texture: &Value,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        alpha: f64,
    ) -> Result<()> {
        self.void(
            |native| native.texture,
            &[
                jvalue {
                    l: texture.object().as_raw(),
                },
                jvalue { d: x },
                jvalue { d: y },
                jvalue { d: w },
                jvalue { d: h },
                jvalue { d: 1.0 },
                jvalue { d: 1.0 },
                jvalue { d: 1.0 },
                jvalue { d: alpha },
            ],
        )
    }

    pub fn text(
        &mut self,
        font: Font,
        x: f64,
        y: f64,
        text: &str,
        [r, g, b, a]: Color,
    ) -> Result<()> {
        let element = self.element.clone();
        with_native(|native| {
            self.context.jni(|env| {
                let text = env.new_string(text)?;
                // SAFETY: The method ID was resolved on UIElement with this exact
                // descriptor and the element is a live UIElement.
                unsafe {
                    env.call_method_unchecked(
                        element.object(),
                        native.text,
                        ReturnType::Primitive(Primitive::Void),
                        &[
                            jvalue {
                                l: native.fonts[font as usize].as_raw(),
                            },
                            jvalue { l: text.as_raw() },
                            jvalue { d: x.floor() },
                            jvalue { d: y.floor() },
                            jvalue { d: r },
                            jvalue { d: g },
                            jvalue { d: b },
                            jvalue { d: a },
                        ],
                    )?;
                }
                Ok(())
            })
        })
    }

    /// Limits drawing to the rectangle until [`Painter::unclip`].
    pub fn clip(&mut self, x: f64, y: f64, w: f64, h: f64) -> Result<()> {
        self.void(
            |native| native.stencil,
            &[
                jvalue { d: x },
                jvalue { d: y },
                jvalue { d: w },
                jvalue { d: h },
            ],
        )
    }

    pub fn unclip(&mut self) -> Result<()> {
        self.void(|native| native.clear_stencil, &[])
    }

    fn void(&mut self, method: impl FnOnce(&Native) -> JMethodID, args: &[jvalue]) -> Result<()> {
        let element = self.element.clone();
        with_native(|native| {
            let method = method(native);
            self.context.jni(|env| {
                // SAFETY: Each method ID was resolved on UIElement with the argument
                // types its caller passes, and the element is a live UIElement.
                unsafe {
                    env.call_method_unchecked(
                        element.object(),
                        method,
                        ReturnType::Primitive(Primitive::Void),
                        args,
                    )?;
                }
                Ok(())
            })
        })
    }
}

pub use crate::lua::first;

pub fn numbers(context: &mut Context<'_, '_>, values: &[f64]) -> Result<Vec<Value>> {
    values.iter().map(|value| context.number(*value)).collect()
}

pub fn construct(context: &mut Context<'_, '_>, class: &str, args: &[Value]) -> Result<Value> {
    let factory = context.global(class)?;
    if factory.is_nil() {
        return Err(format!("The game's {class} is unavailable"));
    }
    let object = first(context.invoke(&factory, "new", args)?);
    if object.is_nil() {
        return Err(format!("The game's {class} created nothing"));
    }
    Ok(object)
}

pub fn initialise(context: &mut Context<'_, '_>, object: &Value) -> Result<()> {
    context.invoke(object, "initialise", &[])?;
    if context.get(object, "javaObject")?.is_nil() {
        context.invoke(object, "instantiate", &[])?;
    }
    Ok(())
}

pub fn add(context: &mut Context<'_, '_>, parent: &Value, child: &Value) -> Result<()> {
    initialise(context, child)?;
    context.invoke(parent, "addChild", std::slice::from_ref(child))?;
    Ok(())
}

pub fn set_text(
    context: &mut Context<'_, '_>,
    object: &Value,
    field: &str,
    text: &str,
) -> Result<()> {
    let value = context.string(text)?;
    context.set(object, field, &value)
}

pub fn set_number(
    context: &mut Context<'_, '_>,
    object: &Value,
    field: &str,
    number: f64,
) -> Result<()> {
    let value = context.number(number)?;
    context.set(object, field, &value)
}

pub fn set_flag(
    context: &mut Context<'_, '_>,
    object: &Value,
    field: &str,
    flag: bool,
) -> Result<()> {
    let value = context.boolean(flag)?;
    context.set(object, field, &value)
}

pub fn call_flag(
    context: &mut Context<'_, '_>,
    object: &Value,
    method: &str,
    flag: bool,
) -> Result<()> {
    let value = context.boolean(flag)?;
    context.invoke(object, method, &[value])?;
    Ok(())
}

/// Calls a boolean setter on a Java object that has no Lua table.
pub fn call_flag_native(
    context: &mut Context<'_, '_>,
    object: &Value,
    method: &str,
    flag: bool,
) -> Result<()> {
    context.jni(|env| {
        env.call_method(
            object.object(),
            method,
            "(Z)V",
            &[JValue::Bool(flag.into())],
        )?;
        Ok(())
    })
}

pub fn ask_flag(context: &mut Context<'_, '_>, object: &Value, method: &str) -> Result<bool> {
    let value = first(context.invoke(object, method, &[])?);
    context.truth(&value)
}

pub fn ask_number(context: &mut Context<'_, '_>, object: &Value, method: &str) -> Result<f64> {
    let value = first(context.invoke(object, method, &[])?);
    context.numeric(&value)
}

pub fn set_bounds(
    context: &mut Context<'_, '_>,
    object: &Value,
    [x, y, w, h]: [f64; 4],
) -> Result<()> {
    for (method, value) in [("setX", x), ("setY", y), ("setWidth", w), ("setHeight", h)] {
        let value = context.number(value)?;
        context.invoke(object, method, &[value])?;
    }
    Ok(())
}

pub fn screen(context: &mut Context<'_, '_>) -> Result<(f64, f64)> {
    let core = first(context.call_global("getCore", &[])?);
    context.jni(|env| {
        let width = env
            .call_method(core.object(), "getScreenWidth", "()I", &[])?
            .i()?;
        let height = env
            .call_method(core.object(), "getScreenHeight", "()I", &[])?
            .i()?;
        Ok((f64::from(width), f64::from(height)))
    })
}

pub fn set_color(
    context: &mut Context<'_, '_>,
    object: &Value,
    field: &str,
    color: Color,
) -> Result<()> {
    let table = context.table()?;
    for (channel, value) in ["r", "g", "b", "a"].into_iter().zip(color) {
        set_number(context, &table, channel, value)?;
    }
    context.set(object, field, &table)
}

/// Loads a texture by path; a missing picture is not an error.
pub fn texture(context: &mut Context<'_, '_>, path: &str) -> Option<Value> {
    let path = context.string(path).ok()?;
    context
        .call_global("getTexture", &[path])
        .ok()?
        .into_iter()
        .next()
        .filter(|texture| !texture.is_nil())
}

pub fn sound(context: &mut Context<'_, '_>, name: &str) {
    let played = (|| -> Result<()> {
        let manager = first(context.call_global("getSoundManager", &[])?);
        let name = context.string(name)?;
        context.invoke(&manager, "playUISound", &[name])?;
        Ok(())
    })();
    if let Err(error) = played {
        crate::log::debug("ui.sound", error).write();
    }
}

pub fn set_clipboard(context: &mut Context<'_, '_>, text: &str) -> Result<()> {
    context.jni(|env| {
        let value = env.new_string(text)?;
        env.call_static_method(
            "zombie/core/Clipboard",
            "setClipboard",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&value)],
        )?;
        Ok(())
    })
}

pub fn working_directory(context: &mut Context<'_, '_>) -> Result<std::path::PathBuf> {
    game::property(context, "user.dir")?
        .map(Into::into)
        .ok_or("The game's working directory is unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::{longest_prefix, wrap_with};

    fn lines(text: &str, limit: f64) -> Vec<String> {
        wrap_with(text, limit, |part| Ok(part.chars().count() as f64)).unwrap()
    }

    #[test]
    fn lines_break_between_words_and_keep_paragraphs() {
        assert_eq!(
            lines("one two three\nfour", 8.0),
            ["one two", "three", "four"]
        );
        assert_eq!(lines("a\n\nb", 8.0), ["a", "", "b"]);
    }

    #[test]
    fn a_word_longer_than_a_line_is_split_at_separators_without_losing_characters() {
        let split = lines("see /home/λambda/very/long", 10.0);
        assert!(split.iter().all(|line| line.chars().count() <= 10));
        assert_eq!(split.concat(), "see/home/λambda/very/long");
        assert_eq!(split[1], "/home/");
        assert_eq!(lines("abcdefghijkl", 5.0), ["abcde", "fghij", "kl"]);
    }

    #[test]
    fn a_prefix_always_makes_progress() {
        assert_eq!(longest_prefix("λx", |_| Ok(false)).unwrap(), 2);
        assert_eq!(
            longest_prefix("abc", |part| Ok(part.len() <= 2)).unwrap(),
            2
        );
    }
}
