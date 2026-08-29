use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use iced::Font;

use crate::comment::CommentStyle;
use crate::indent::Indentation;
use crate::keybindings::KeyResolver;
use crate::word::WordSeparators;
use crate::{font, history, line_numbers, text_editor};

/// Editor settings the app can change after construction: a config reload
/// writes here, and every open [`crate::TextArea`] reads through a shared
/// handle on each `view`, which is what lets a reload reach tabs that already
/// exist. One per app, created alongside `TextArea::factory`.
pub struct SharedEditorConfig {
    /// `f32` bits - an atomic can't hold a float directly.
    background_alpha: AtomicU32,
    /// `f32` bits, same reason. Range-checked by the widget's
    /// `scroll_sensitivity` builder, not here.
    scroll_sensitivity: AtomicU32,
    /// `f32` bits, as above. Range-checked by the widget's `drag_speed`
    /// builder.
    drag_speed: AtomicU32,
    /// Clamped by `History::set_depth`, not here.
    undo_depth: AtomicUsize,
    /// The typeface documents are drawn in. Behind a lock rather than in an
    /// atomic because a [`Font`] is four fields wide; swappable because a
    /// `config.toml` reload has to reach tabs that already exist.
    font: RwLock<Font>,
    /// `f32` bits, as above. Clamped by [`font::clamp_size`] on the way in.
    font_size: AtomicU32,
    /// Whether documents are numbered down the left.
    line_numbers: AtomicBool,
    /// `f32` bits, as above. How far back from the document's text the line
    /// numbers are drawn.
    line_numbers_alpha: AtomicU32,
    /// `f32` bits, as above. The blank left of the line numbers, in
    /// characters. Clamped by [`line_numbers::Padding`], not here.
    line_numbers_padding_left: AtomicU32,
    /// `f32` bits, as above. The blank between the line numbers and the
    /// text, in characters. Clamped in the same place.
    line_numbers_padding_right: AtomicU32,
    /// `Arc` inside the lock so `view` clones a refcount out per redraw,
    /// not the whole resolver. Swappable because a `keybinds.toml` reload
    /// has to reach tabs that already exist.
    resolver: RwLock<Arc<KeyResolver>>,
    /// Extension -> comment style, flattened from the config by the app.
    /// Keys are lowercase; look up lowercased.
    comment_styles: RwLock<Arc<HashMap<String, CommentStyle>>>,
    /// What Tab inserts and how wide a tab draws. Behind a lock rather than
    /// in an atomic because it is two fields, like [`Self::font`]; read once
    /// per view and once per Tab press, neither of them hot.
    indentation: RwLock<Indentation>,
    /// What ends a word, for the word motions and the double click. Behind a
    /// lock for the same reason [`Self::font`] is - it is a string, not a
    /// number - and read once per word action, which is one keystroke or one
    /// click.
    word_separators: RwLock<WordSeparators>,
}

impl SharedEditorConfig {
    pub fn new(background_alpha: f32, resolver: Arc<KeyResolver>) -> Arc<Self> {
        Arc::new(Self {
            background_alpha: AtomicU32::new(
                background_alpha.clamp(0.0, 1.0).to_bits(),
            ),
            scroll_sensitivity: AtomicU32::new(1.0f32.to_bits()),
            drag_speed: AtomicU32::new(1.0f32.to_bits()),
            undo_depth: AtomicUsize::new(history::DEFAULT_DEPTH),
            font: RwLock::new(Font::MONOSPACE),
            font_size: AtomicU32::new(font::DEFAULT_SIZE.to_bits()),
            line_numbers: AtomicBool::new(false),
            line_numbers_alpha: AtomicU32::new(
                text_editor::DEFAULT_LINE_NUMBER_ALPHA.to_bits(),
            ),
            line_numbers_padding_left: AtomicU32::new(
                line_numbers::DEFAULT_PADDING.to_bits(),
            ),
            line_numbers_padding_right: AtomicU32::new(
                line_numbers::DEFAULT_PADDING.to_bits(),
            ),
            resolver: RwLock::new(resolver),
            comment_styles: RwLock::new(Arc::new(HashMap::new())),
            indentation: RwLock::new(Indentation::default()),
            word_separators: RwLock::new(WordSeparators::default()),
        })
    }

    pub fn background_alpha(&self) -> f32 {
        f32::from_bits(self.background_alpha.load(Ordering::Relaxed))
    }

    pub fn resolver(&self) -> Arc<KeyResolver> {
        self.resolver.read().unwrap().clone()
    }

    pub fn set_background_alpha(&self, alpha: f32) {
        self.background_alpha
            .store(alpha.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// The multiplier on wheel and trackpad scroll distance. `1.0` is the
    /// shipped speed; the widget clamps whatever lands here to a usable
    /// range, so a nonsense `config.toml` value can't disable scrolling.
    pub fn scroll_sensitivity(&self) -> f32 {
        f32::from_bits(self.scroll_sensitivity.load(Ordering::Relaxed))
    }

    pub fn set_scroll_sensitivity(&self, sensitivity: f32) {
        self.scroll_sensitivity
            .store(sensitivity.to_bits(), Ordering::Relaxed);
    }

    /// The multiplier on how fast a selection drag held past the top or
    /// bottom edge walks the view. `1.0` is the shipped speed; the widget
    /// clamps whatever lands here to the same usable range as the wheel's.
    pub fn drag_speed(&self) -> f32 {
        f32::from_bits(self.drag_speed.load(Ordering::Relaxed))
    }

    pub fn set_drag_speed(&self, speed: f32) {
        self.drag_speed.store(speed.to_bits(), Ordering::Relaxed);
    }

    /// Undo steps per tab. A step is a burst of typing, not a keystroke.
    pub fn undo_depth(&self) -> usize {
        self.undo_depth.load(Ordering::Relaxed)
    }

    pub fn set_undo_depth(&self, depth: usize) {
        self.undo_depth.store(depth, Ordering::Relaxed);
    }

    /// The typeface documents are drawn in. `Font::MONOSPACE` - the
    /// system's own monospace face - until a config names another.
    pub fn font(&self) -> Font {
        *self.font.read().unwrap()
    }

    pub fn set_font(&self, font: Font) {
        *self.font.write().unwrap() = font;
    }

    /// The height of the editor's text in pixels.
    pub fn font_size(&self) -> f32 {
        f32::from_bits(self.font_size.load(Ordering::Relaxed))
    }

    pub fn set_font_size(&self, size: f32) {
        self.font_size
            .store(font::clamp_size(size).to_bits(), Ordering::Relaxed);
    }

    /// Whether each line is numbered down the left of the document. Off
    /// until a theme asks for it.
    pub fn line_numbers(&self) -> bool {
        self.line_numbers.load(Ordering::Relaxed)
    }

    pub fn set_line_numbers(&self, line_numbers: bool) {
        self.line_numbers.store(line_numbers, Ordering::Relaxed);
    }

    /// How much of the document's text color the line numbers are drawn at.
    /// `1.0` would put them level with the text, which is the one thing a
    /// gutter must not do.
    pub fn line_numbers_alpha(&self) -> f32 {
        f32::from_bits(self.line_numbers_alpha.load(Ordering::Relaxed))
    }

    pub fn set_line_numbers_alpha(&self, alpha: f32) {
        self.line_numbers_alpha
            .store(alpha.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// The blank either side of the line numbers, in characters. `None`
    /// while the document isn't numbered, which is the same answer the
    /// widget's own builder takes.
    pub fn line_numbers_padding(&self) -> Option<line_numbers::Padding> {
        self.line_numbers().then(|| {
            line_numbers::Padding::new(
                f32::from_bits(
                    self.line_numbers_padding_left.load(Ordering::Relaxed),
                ),
                f32::from_bits(
                    self.line_numbers_padding_right.load(Ordering::Relaxed),
                ),
            )
        })
    }

    pub fn set_line_numbers_padding_left(&self, characters: f32) {
        self.line_numbers_padding_left
            .store(characters.to_bits(), Ordering::Relaxed);
    }

    pub fn set_line_numbers_padding_right(&self, characters: f32) {
        self.line_numbers_padding_right
            .store(characters.to_bits(), Ordering::Relaxed);
    }

    /// Routed through here so settings have one mutation API, but stored in
    /// `FOREGROUND_ALPHA` - see that static for why it's global.
    pub fn set_foreground_alpha(&self, alpha: f32) {
        FOREGROUND_ALPHA
            .store(alpha.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_resolver(&self, resolver: Arc<KeyResolver>) {
        *self.resolver.write().unwrap() = resolver;
    }

    pub fn comment_styles(&self) -> Arc<HashMap<String, CommentStyle>> {
        self.comment_styles.read().unwrap().clone()
    }

    pub fn set_comment_styles(&self, styles: HashMap<String, CommentStyle>) {
        *self.comment_styles.write().unwrap() = Arc::new(styles);
    }

    /// What one press of Tab inserts, and the width every tab in every open
    /// document is drawn at. Tabs at four columns until a config says
    /// otherwise.
    pub fn indentation(&self) -> Indentation {
        *self.indentation.read().unwrap()
    }

    /// Range-checked by [`Indentation::new`], the way a font size is by
    /// [`font::clamp_size`] - so a nonsense `config.toml` width can't reach
    /// the buffer that draws the tabs.
    pub fn set_indentation(&self, indentation: Indentation) {
        *self.indentation.write().unwrap() = indentation;
    }

    /// The characters a word ends at, for the word motions and the double
    /// click. VS Code's list until a config names another.
    pub fn word_separators(&self) -> WordSeparators {
        self.word_separators.read().unwrap().clone()
    }

    pub fn set_word_separators(&self, separators: WordSeparators) {
        *self.word_separators.write().unwrap() = separators;
    }
}

/// Scales every syntax-highlighted color `color_for` produces. Global
/// rather than a field on [`SharedEditorConfig`] because
/// `text_editor::highlight_with`'s `to_format` callback must be a bare `fn`
/// pointer - it can't capture state, so `color_for` has nothing else to
/// read this from. `f32` bits; written via
/// [`SharedEditorConfig::set_foreground_alpha`], including on config reload.
static FOREGROUND_ALPHA: AtomicU32 = AtomicU32::new(f32::to_bits(1.0));

pub(crate) fn foreground_alpha() -> f32 {
    f32::from_bits(FOREGROUND_ALPHA.load(Ordering::Relaxed))
}
