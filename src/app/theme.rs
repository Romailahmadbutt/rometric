//! Light neumorphic design system for Signal Bench.
//!
//! Everything here is pure painting plus small custom widgets. No widget in
//! this module owns application state; callers pass in the same `f32`/`bool`
//! fields the standard egui widgets used before, so the audio engine and
//! settings persistence stay untouched.
//!
//! Visual refresh notes (public API is unchanged; everything new is additive):
//!
//! * Real pressed states: raised surfaces sink into an inset when pressed.
//! * Smooth gradients on blue surfaces (primary buttons, slider and fader fills).
//! * Better contrast: darker `MUTED` / `FAINT`, plus `WARN_TEXT` for orange text.
//! * Keyboard focus rings, hover/drag feedback, and pointer cursors.
//! * Audio-specific polish: fader ticks and zero notch, bipolar fills, a zoned
//!   meter with peak hold and glow, and a `DANGER` colour.
//! * Elevation tokens (`Elevation`) and shadows that scale with element size.
//! * Animations honour `reduced_motion` (via `Style::animation_time`).
//! * Optional font install (`install_fonts`) for a proper semibold heading face.
//! * Gear icon that looks like a gear, round-capped icon strokes.

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, FontFamily, FontId, Id, Layout, Margin, Mesh, Pos2,
    Rect, Response, RichText, Rounding, Sense, Shape, Stroke, TextStyle, Ui, Vec2,
};
use rometric::analysis::{peak_dbfs, SPECTRUM_BANDS, SPECTRUM_FLOOR_DB};

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

pub const BG: Color32 = Color32::from_rgb(0xE8, 0xED, 0xF8);
pub const CARD: Color32 = Color32::from_rgb(0xED, 0xF1, 0xFA);
pub const CARD_HOVER: Color32 = Color32::from_rgb(0xF4, 0xF7, 0xFE);
pub const INSET: Color32 = Color32::from_rgb(0xE1, 0xE7, 0xF4);
pub const GRAPH: Color32 = Color32::from_rgb(0xE9, 0xEE, 0xF9);
pub const BLUE: Color32 = Color32::from_rgb(0x34, 0x78, 0xF6);
pub const BLUE_DEEP: Color32 = Color32::from_rgb(0x1E, 0x5E, 0xDB);
pub const BLUE_LIGHT: Color32 = Color32::from_rgb(0x65, 0xA5, 0xFF);
pub const BLUE_SOFT: Color32 = Color32::from_rgb(0xD9, 0xE6, 0xFF);
pub const INK: Color32 = Color32::from_rgb(0x17, 0x2B, 0x55);
/// Secondary text. About 4.5:1 against `CARD`.
pub const MUTED: Color32 = Color32::from_rgb(0x5F, 0x6E, 0x8C);
/// Hints and placeholders. About 3:1 against `CARD`; do not use for essential text.
pub const FAINT: Color32 = Color32::from_rgb(0x7F, 0x8D, 0xA8);
/// Bright orange for icons and meter segments. Too light for small text.
pub const WARN: Color32 = Color32::from_rgb(0xFF, 0x94, 0x1F);
/// Orange that is readable as text on `CARD` (about 4.7:1).
pub const WARN_TEXT: Color32 = Color32::from_rgb(0xA8, 0x54, 0x00);
/// Clipping / error red.
pub const DANGER: Color32 = Color32::from_rgb(0xE5, 0x48, 0x4D);
pub const OK: Color32 = Color32::from_rgb(0x22, 0xB5, 0x73);
pub const WHITE: Color32 = Color32::WHITE;

pub const BORDER: Color32 = Color32::from_rgba_premultiplied(178, 178, 178, 178);
pub const HAIRLINE: Color32 = Color32::from_rgba_premultiplied(2, 4, 7, 22);
pub const GRID: Color32 = Color32::from_rgba_premultiplied(2, 3, 7, 20);
pub const GRID_STRONG: Color32 = Color32::from_rgba_premultiplied(4, 8, 15, 46);
pub const INNER_DARK: Color32 = Color32::from_rgba_premultiplied(45, 51, 63, 96);
pub const INNER_LIGHT: Color32 = Color32::from_rgba_premultiplied(190, 190, 190, 190);
/// Keyboard focus ring (BLUE at ~43% alpha).
pub const FOCUS: Color32 = Color32::from_rgba_premultiplied(22, 52, 106, 110);

pub const CARD_RADIUS: f32 = 18.0;
pub const BUTTON_RADIUS: f32 = 11.0;

/// Tick colour used on faders and bipolar sliders.
const TICK: Color32 = Color32::from_rgba_premultiplied(2, 4, 8, 60);
const TICK_STRONG: Color32 = Color32::from_rgba_premultiplied(5, 9, 19, 120);

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Linear mix of two colours (works on premultiplied components).
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

/// Animate a bool using the global animation time, so `reduced_motion`
/// (which sets `animation_time` to 0) is honoured by every custom widget.
fn animate(ctx: &egui::Context, id: Id, target: bool) -> f32 {
    let time = ctx.style().animation_time;
    ctx.animate_bool_with_time_and_easing(id, target, time, egui::emath::easing::cubic_out)
}

/// True once the user has navigated with Tab and not clicked since. Used so
/// focus rings appear for keyboard users but not after every mouse click.
fn keyboard_nav(ctx: &egui::Context) -> bool {
    let id = Id::new("signal_bench_theme_keyboard_nav");
    let mut on = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    ctx.input(|i| {
        if i.key_pressed(egui::Key::Tab) {
            on = true;
        }
        if i.pointer.any_pressed() {
            on = false;
        }
    });
    ctx.data_mut(|d| d.insert_temp(id, on));
    on
}

/// Soft blue ring just outside `rect`.
pub fn focus_ring(painter: &egui::Painter, rect: Rect, radius: f32) {
    painter.rect_stroke(rect.expand(3.0), radius + 3.0, Stroke::new(2.0_f32, FOCUS));
}

fn paint_focus(ui: &Ui, response: &Response, radius: f32) {
    if response.has_focus() && keyboard_nav(ui.ctx()) {
        focus_ring(ui.painter(), response.rect, radius);
    }
}

fn field_focus(painter: &egui::Painter, rect: Rect, radius: f32) {
    painter.rect_stroke(
        rect.expand(1.0),
        radius + 1.0,
        Stroke::new(1.6_f32, Color32::from_rgba_premultiplied(30, 70, 148, 150)),
    );
}

// ---------------------------------------------------------------------------
// Fonts (optional)
// ---------------------------------------------------------------------------

static SEMIBOLD_READY: AtomicBool = AtomicBool::new(false);
const SEMIBOLD_NAME: &str = "semibold";

/// Font files to embed. Example (files placed in `assets/fonts/`):
///
/// ```ignore
/// theme::install_fonts(&cc.egui_ctx, theme::FontSet {
///     regular: include_bytes!("../assets/fonts/Inter-Regular.ttf"),
///     semibold: include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
///     mono: None,
/// });
/// ```
#[allow(dead_code)] // Optional embedding API; preserve existing font support.
pub struct FontSet {
    pub regular: &'static [u8],
    pub semibold: &'static [u8],
    pub mono: Option<&'static [u8]>,
}

/// Install the UI fonts. Call once at startup (for example in the eframe
/// creation closure) before the first frame is drawn. Optional: without it
/// the theme falls back to egui's built-in fonts.
#[allow(dead_code)]
pub fn install_fonts(ctx: &egui::Context, fonts: FontSet) {
    let mut defs = egui::FontDefinitions::default();
    defs.font_data.insert(
        "theme-regular".to_owned(),
        egui::FontData::from_static(fonts.regular),
    );
    defs.font_data.insert(
        "theme-semibold".to_owned(),
        egui::FontData::from_static(fonts.semibold),
    );
    if let Some(mono) = fonts.mono {
        defs.font_data
            .insert("theme-mono".to_owned(), egui::FontData::from_static(mono));
        defs.families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, "theme-mono".to_owned());
    }
    let proportional = defs.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "theme-regular".to_owned());
    let fallback = proportional.clone();
    let mut semibold = vec!["theme-semibold".to_owned()];
    semibold.extend(fallback);
    defs.families
        .insert(FontFamily::Name(SEMIBOLD_NAME.into()), semibold);
    ctx.set_fonts(defs);
    SEMIBOLD_READY.store(true, Ordering::Relaxed);
}

/// Heading face: semibold if `install_fonts` ran, otherwise the default.
pub fn semibold() -> FontFamily {
    if SEMIBOLD_READY.load(Ordering::Relaxed) {
        FontFamily::Name(SEMIBOLD_NAME.into())
    } else {
        FontFamily::Proportional
    }
}

// ---------------------------------------------------------------------------
// Style installation
// ---------------------------------------------------------------------------

pub fn install(ctx: &egui::Context, reduced_motion: bool) {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = BG;
    visuals.window_fill = CARD;
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.window_rounding = Rounding::same(18.0);
    visuals.extreme_bg_color = INSET;
    visuals.faint_bg_color = CARD;
    visuals.code_bg_color = INSET;
    visuals.override_text_color = None;
    visuals.hyperlink_color = BLUE;
    visuals.selection.bg_fill = BLUE;
    visuals.selection.stroke = Stroke::new(1.0_f32, WHITE);
    visuals.text_cursor.stroke = Stroke::new(2.0_f32, BLUE);
    visuals.interact_cursor = Some(CursorIcon::PointingHand);
    visuals.window_shadow = egui::epaint::Shadow {
        offset: Vec2::new(0.0, 10.0),
        blur: 28.0,
        spread: 2.0,
        color: Color32::from_rgba_unmultiplied(80, 96, 126, 34),
    };
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: Vec2::new(0.0, 8.0),
        blur: 22.0,
        spread: 1.0,
        color: Color32::from_rgba_unmultiplied(80, 96, 126, 30),
    };
    visuals.menu_rounding = Rounding::same(14.0);

    visuals.widgets.noninteractive.bg_fill = CARD;
    visuals.widgets.noninteractive.weak_bg_fill = CARD;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, HAIRLINE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, INK);
    visuals.widgets.noninteractive.rounding = Rounding::same(12.0);

    visuals.widgets.inactive.bg_fill = CARD;
    visuals.widgets.inactive.weak_bg_fill = CARD;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, INK);
    visuals.widgets.inactive.rounding = Rounding::same(BUTTON_RADIUS);
    visuals.widgets.inactive.expansion = 0.0;

    visuals.widgets.hovered.bg_fill = CARD_HOVER;
    visuals.widgets.hovered.weak_bg_fill = CARD_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, BLUE_LIGHT);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, BLUE_DEEP);
    visuals.widgets.hovered.rounding = Rounding::same(BUTTON_RADIUS);
    visuals.widgets.hovered.expansion = 0.0;

    visuals.widgets.active.bg_fill = BLUE;
    visuals.widgets.active.weak_bg_fill = BLUE_SOFT;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, BLUE);
    visuals.widgets.active.fg_stroke = Stroke::new(2.0_f32, WHITE);
    visuals.widgets.active.rounding = Rounding::same(BUTTON_RADIUS);
    visuals.widgets.active.expansion = 0.0;

    visuals.widgets.open.bg_fill = CARD_HOVER;
    visuals.widgets.open.weak_bg_fill = CARD_HOVER;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, INK);
    visuals.widgets.open.rounding = Rounding::same(BUTTON_RADIUS);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 5.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.interact_size = Vec2::new(36.0, 28.0);
    style.spacing.slider_width = 100.0;
    style.spacing.combo_width = 160.0;
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.scroll.floating = true;
    style.animation_time = if reduced_motion { 0.0 } else { 0.12 };
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(22.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(14.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(11.5, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(12.5, FontFamily::Monospace),
    );
    ctx.set_style(style);
}

// ---------------------------------------------------------------------------
// Elevation
// ---------------------------------------------------------------------------

/// How far a surface sits above the background. Nested surfaces should sit
/// lower than the surface that contains them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Elevation {
    /// List rows and other quiet surfaces.
    Flat,
    /// Buttons and chips at rest.
    Low,
    /// Sections nested inside a card.
    Mid,
    /// Top-level cards, popups and tooltips.
    High,
}

impl Elevation {
    pub fn lift(self) -> f32 {
        match self {
            Elevation::Flat => 0.3,
            Elevation::Low => 0.55,
            Elevation::Mid => 0.78,
            Elevation::High => 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Surfaces
// ---------------------------------------------------------------------------

fn blur_rect(rect: Rect, rounding: f32, offset: Vec2, blur: f32, color: Color32) -> Shape {
    Shape::Rect(
        egui::epaint::RectShape::filled(rect.translate(offset), rounding, color)
            .with_blur_width(blur),
    )
}

/// Raised neumorphic surface. Shadow size scales with the element so small
/// controls get tight shadows and large cards get soft, wide ones.
/// `fade` (0..=1) fades the whole surface out, for cross-fades.
fn raised_shape_fade(rect: Rect, radius: f32, fill: Color32, lift: f32, fade: f32) -> Shape {
    let fade = fade.clamp(0.0, 1.0);
    let scale = (rect.width().min(rect.height()) / 56.0).clamp(0.7, 1.0);
    let soft = (0.5 + lift * 0.5) * scale;
    let t = ((lift - 0.55) / 0.45).clamp(0.0, 1.0);
    let light_alpha = ((40.0 + 32.0 * t) * fade) as u8;
    let dark_alpha = ((42.0 + 20.0 * t) * fade) as u8;
    let blur_light = (12.0 * scale).max(8.0);
    let blur_dark = (15.0 * scale).max(10.0);
    Shape::Vec(vec![
        blur_rect(
            rect,
            radius,
            Vec2::new(-4.0 * soft, -4.0 * soft),
            blur_light,
            Color32::from_rgba_unmultiplied(255, 255, 255, light_alpha),
        ),
        blur_rect(
            rect,
            radius,
            Vec2::new(6.0 * soft, 6.0 * soft),
            blur_dark,
            Color32::from_rgba_unmultiplied(156, 174, 207, dark_alpha),
        ),
        Shape::Rect(egui::epaint::RectShape::new(
            rect,
            radius,
            fill.gamma_multiply(fade),
            Stroke::new(1.0_f32, BORDER.gamma_multiply(fade)),
        )),
    ])
}

fn raised_shape(rect: Rect, radius: f32, fill: Color32, lift: f32) -> Shape {
    raised_shape_fade(rect, radius, fill, lift, 1.0)
}

pub fn paint_raised(painter: &egui::Painter, rect: Rect, radius: f32, fill: Color32) {
    painter.add(raised_shape(rect, radius, fill, Elevation::High.lift()));
}

#[allow(dead_code)] // Retained design-system primitive for additional surfaces.
pub fn paint_elevated(
    painter: &egui::Painter,
    rect: Rect,
    radius: f32,
    fill: Color32,
    elevation: Elevation,
) {
    painter.add(raised_shape(rect, radius, fill, elevation.lift()));
}

fn paint_button_surface(
    painter: &egui::Painter,
    rect: Rect,
    radius: f32,
    fill: Color32,
    lift: f32,
) {
    painter.add(raised_shape(rect, radius, fill, lift));
}

pub fn paint_inset(painter: &egui::Painter, rect: Rect, radius: f32) {
    paint_inset_with(painter, rect, radius, INSET);
}

pub fn paint_inset_with(painter: &egui::Painter, rect: Rect, radius: f32, fill: Color32) {
    painter.rect_filled(rect, radius, fill);
    let clipped = painter.with_clip_rect(rect);
    // Soft dark edge, biased to the top-left.
    let dark = egui::epaint::RectShape {
        rect: rect.translate(Vec2::new(2.0, 2.5)).shrink(0.5),
        rounding: Rounding::same(radius),
        fill: Color32::TRANSPARENT,
        stroke: Stroke::new(1.6_f32, INNER_DARK),
        blur_width: 7.0,
        fill_texture_id: Default::default(),
        uv: Rect::ZERO,
    };
    clipped.add(Shape::Rect(dark));
    // Soft light edge, biased to the bottom-right.
    let light = egui::epaint::RectShape {
        rect: rect.translate(Vec2::new(-2.0, -2.5)).shrink(0.5),
        rounding: Rounding::same(radius),
        fill: Color32::TRANSPARENT,
        stroke: Stroke::new(1.4_f32, INNER_LIGHT),
        blur_width: 6.0,
        fill_texture_id: Default::default(),
        uv: Rect::ZERO,
    };
    clipped.add(Shape::Rect(light));
    painter.rect_stroke(
        rect.shrink(0.5),
        radius,
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 150)),
    );
}

// ---------------------------------------------------------------------------
// Gradients
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    Vertical,
    Horizontal,
}

/// Outline of a rounded rect as a clockwise point list.
fn rounded_points(rect: Rect, radius: f32, steps: usize) -> Vec<Pos2> {
    let r = radius
        .min(rect.width() * 0.5)
        .min(rect.height() * 0.5)
        .max(0.0);
    let corners = [
        (Pos2::new(rect.right() - r, rect.top() + r), -FRAC_PI_2),
        (Pos2::new(rect.right() - r, rect.bottom() - r), 0.0),
        (Pos2::new(rect.left() + r, rect.bottom() - r), FRAC_PI_2),
        (Pos2::new(rect.left() + r, rect.top() + r), PI),
    ];
    let mut points = Vec::with_capacity(4 * (steps + 1));
    for (c, start) in corners {
        for i in 0..=steps {
            let a = start + FRAC_PI_2 * i as f32 / steps as f32;
            points.push(Pos2::new(c.x + r * a.cos(), c.y + r * a.sin()));
        }
    }
    points
}

/// A rounded rectangle filled with a two-colour linear gradient.
///
/// Built as a triangle fan from the centre with per-vertex colours, which is
/// exact for linear gradients. Meshes are not anti-aliased, so a flat rounded
/// rect underneath supplies a smooth edge and the mesh sits 0.6px inside it.
fn gradient_shape(rect: Rect, radius: f32, from: Color32, to: Color32, axis: Axis) -> Shape {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return Shape::Noop;
    }
    let base = Shape::Rect(egui::epaint::RectShape::filled(
        rect,
        radius,
        mix(from, to, 0.5),
    ));
    let inner = rect.shrink(0.6);
    if inner.width() < 1.0 || inner.height() < 1.0 {
        return base;
    }
    let points = rounded_points(inner, (radius - 0.6).max(0.0), 8);
    let color_at = |p: Pos2| {
        let t = match axis {
            Axis::Vertical => (p.y - inner.top()) / inner.height(),
            Axis::Horizontal => (p.x - inner.left()) / inner.width(),
        };
        mix(from, to, t)
    };
    let mut mesh = Mesh::default();
    let center = inner.center();
    mesh.colored_vertex(center, color_at(center));
    for p in &points {
        mesh.colored_vertex(*p, color_at(*p));
    }
    let n = points.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    Shape::Vec(vec![base, Shape::mesh(mesh)])
}

// ---------------------------------------------------------------------------
// Primary (blue) surfaces
// ---------------------------------------------------------------------------

pub fn paint_primary(painter: &egui::Painter, rect: Rect, radius: f32, glow: bool, hover: f32) {
    paint_primary_state(painter, rect, radius, glow, hover, false, 1.0);
}

/// Primary surface with press feedback. `opacity` cross-fades it in over
/// whatever is already painted (used by chips and toggle pills).
fn paint_primary_state(
    painter: &egui::Painter,
    rect: Rect,
    radius: f32,
    glow: bool,
    hover: f32,
    pressed: bool,
    opacity: f32,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }
    if glow {
        let (offset, blur, alpha) = if pressed {
            (Vec2::new(0.0, 1.0), 8.0, 38.0)
        } else {
            (Vec2::new(0.0, 4.0), 16.0, 62.0 + hover * 26.0)
        };
        painter.add(blur_rect(
            rect,
            radius,
            offset,
            blur,
            Color32::from_rgba_unmultiplied(52, 120, 246, (alpha * opacity) as u8),
        ));
    }
    painter.add(primary_shape_state(rect, radius, pressed, opacity));
    if hover > 0.0 && !pressed {
        painter.rect_filled(
            rect,
            radius,
            Color32::from_white_alpha((hover * 16.0 * opacity) as u8),
        );
    }
}

fn primary_shape_state(rect: Rect, radius: f32, pressed: bool, opacity: f32) -> Shape {
    let (top, bottom) = if pressed {
        (mix(BLUE, BLUE_DEEP, 0.35), mix(BLUE, BLUE_DEEP, 0.8))
    } else {
        (mix(BLUE, BLUE_LIGHT, 0.28), mix(BLUE, BLUE_DEEP, 0.6))
    };
    Shape::Vec(vec![
        gradient_shape(
            rect,
            radius,
            top.gamma_multiply(opacity),
            bottom.gamma_multiply(opacity),
            Axis::Vertical,
        ),
        Shape::Rect(egui::epaint::RectShape::stroke(
            rect.shrink(0.5),
            radius,
            Stroke::new(1.0_f32, Color32::from_white_alpha((58.0 * opacity) as u8)),
        )),
    ])
}

pub fn primary_shape(rect: Rect, radius: f32) -> Shape {
    primary_shape_state(rect, radius, false, 1.0)
}

pub fn row_shape(rect: Rect, radius: f32, fill: Color32) -> Shape {
    raised_shape(rect, radius, fill, Elevation::Flat.lift())
}

pub fn paint_value_chip(painter: &egui::Painter, rect: Rect, text: &str, color: Color32) {
    paint_inset(painter, rect, 9.0);
    // Monospace keeps digits from jittering while a value is dragged; fall back
    // to proportional if the text would not fit.
    let mono = painter.layout_no_wrap(text.to_owned(), FontId::monospace(11.5), color);
    let galley = if mono.size().x <= rect.width() - 8.0 {
        mono
    } else {
        painter.layout_no_wrap(text.to_owned(), FontId::proportional(12.0), color)
    };
    painter.galley(rect.center() - galley.size() * 0.5, galley, color);
}

// ---------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------

/// A raised card. Content is laid out first, then the card surface is slotted
/// in behind it so soft shadows are not clipped by the content.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    card_with(ui, CARD_RADIUS, 12.0, add)
}

pub fn card_with<R>(ui: &mut Ui, radius: f32, padding: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    card_padded(ui, radius, Margin::same(padding), add)
}

pub fn card_padded<R>(
    ui: &mut Ui,
    radius: f32,
    padding: Margin,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    card_at(ui, Elevation::High, radius, padding, add)
}

/// A card at an explicit elevation.
pub fn card_at<R>(
    ui: &mut Ui,
    elevation: Elevation,
    radius: f32,
    padding: Margin,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let slot = ui.painter().add(Shape::Noop);
    let inner = egui::Frame::none().inner_margin(padding).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
    let rect = inner.response.rect;
    let painter = ui.painter().clone();
    painter.set(slot, raised_shape(rect, radius, CARD, elevation.lift()));
    inner.inner
}

/// A titled section. Sits one step below a top-level card.
pub fn section_card<R>(ui: &mut Ui, icon: Icon, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    card_at(ui, Elevation::Mid, 16.0, Margin::same(10.0), |ui| {
        ui.spacing_mut().interact_size.y = 24.0;
        section_title(ui, icon, title);
        ui.add_space(6.0);
        add(ui)
    })
}

pub fn section_title(ui: &mut Ui, icon: Icon, title: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = 20.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
        paint_icon(ui.painter(), rect.shrink(2.0), icon, BLUE);
        ui.label(
            RichText::new(title)
                .size(15.0)
                .strong()
                .family(semibold())
                .color(INK),
        );
    });
}

pub fn heading(ui: &mut Ui, icon: Icon, title: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
        paint_icon(ui.painter(), rect.shrink(2.5), icon, BLUE);
        ui.label(
            RichText::new(title)
                .size(18.0)
                .strong()
                .family(semibold())
                .color(INK),
        );
    });
}

pub fn divider(ui: &mut Ui) {
    ui.add_space(2.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.5, HAIRLINE);
    ui.add_space(2.0);
}

pub fn status_dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, 9.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.6, color);
}

/// Segmented level meter with zones (normal -> amber -> red), a soft glow on
/// lit segments, and an automatic peak-hold marker that decays.
pub fn meter(ui: &mut Ui, label: &str, value: f32, color: Color32) {
    let id = ui.id().with(("signal-bench-meter-peak", label));
    let now = ui.input(|i| i.time);
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    let value = if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    };
    let (mut peak, mut hold_until) = ui
        .ctx()
        .data(|d| d.get_temp::<(f32, f64)>(id))
        .unwrap_or((value, now));
    if value >= peak {
        peak = value;
        hold_until = now + 0.9;
    } else if now > hold_until {
        peak = (peak - dt * 0.6).max(value);
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, (peak, hold_until)));
    if peak > value + 0.001 {
        ui.ctx().request_repaint();
    }
    meter_with_peak(ui, label, value, Some(peak), color);
    let db = peak_dbfs(value);
    let text = if db.is_finite() && db > -100.0 {
        format!("{db:.1} dBFS")
    } else {
        "−∞ dBFS".into()
    };
    ui.add_sized(
        [76.0, 18.0],
        egui::Label::new(
            RichText::new(text)
                .monospace()
                .size(10.5)
                .color(if db >= 0.0 { DANGER } else { MUTED }),
        )
        .truncate(),
    )
    .on_hover_text("Sample peak relative to full scale. 0 dBFS is full scale; silence is −∞.");
}

/// Same as [`meter`] but the caller supplies the peak (or `None` for no marker).
pub fn meter_with_peak(ui: &mut Ui, label: &str, value: f32, peak: Option<f32>, color: Color32) {
    ui.label(RichText::new(label).size(10.5).color(FAINT));
    let (rect, _) = ui.allocate_exact_size(Vec2::new(70.0, 9.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect.expand2(Vec2::new(3.0, 2.5)), 5.0, INSET);
    painter.rect_stroke(
        rect.expand2(Vec2::new(3.0, 2.5)),
        5.0,
        Stroke::new(1.0_f32, HAIRLINE),
    );
    let segments = 12usize;
    let gap = 2.0;
    let width = (rect.width() - gap * (segments as f32 - 1.0)) / segments as f32;
    let lit = (value.clamp(0.0, 1.0) * segments as f32).round() as usize;
    let zone = |index: usize| {
        if index + 1 >= segments {
            DANGER
        } else if index + 3 >= segments {
            WARN
        } else {
            color
        }
    };
    let segment_rect = |index: usize| {
        let x = rect.left() + index as f32 * (width + gap);
        Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(width, rect.height()))
    };
    // Glow first, so every segment is drawn crisp on top of it.
    for index in 0..lit {
        let c = zone(index);
        painter.add(blur_rect(
            segment_rect(index).expand(1.0),
            2.0,
            Vec2::ZERO,
            6.0,
            Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 55),
        ));
    }
    for index in 0..segments {
        let fill = if index < lit {
            zone(index)
        } else {
            Color32::from_rgba_unmultiplied(23, 43, 85, 20)
        };
        painter.rect_filled(segment_rect(index), 1.5, fill);
    }
    if let Some(peak) = peak {
        let peak = peak.clamp(0.0, 1.0);
        if peak > 0.0 {
            let index = ((peak * segments as f32).ceil() as usize)
                .saturating_sub(1)
                .min(segments - 1);
            if index >= lit {
                let c = zone(index);
                painter.rect_filled(
                    segment_rect(index),
                    1.5,
                    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 170),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Buttons and chips
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Headphones,
    Sliders,
    Speaker,
    SpeakerOff,
    Compare,
    Preset,
    Settings,
    Search,
    Play,
    Stop,
    Refresh,
    Undo,
    Redo,
    Restore,
    Bars,
    Layers,
    Warning,
    Diagnostics,
    Help,
}

fn icon_galley(ui: &Ui, text: &str, font: FontId) -> std::sync::Arc<egui::Galley> {
    ui.painter().layout_no_wrap(text.to_owned(), font, INK)
}

fn button_body(
    ui: &mut Ui,
    label: &str,
    icon: Option<Icon>,
    enabled: bool,
    kind: ButtonKind,
) -> Response {
    let enabled = enabled && ui.is_enabled();
    let font = FontId::proportional(match kind {
        ButtonKind::Small => 11.5,
        ButtonKind::Medium => 12.0,
        ButtonKind::Primary | ButtonKind::Neutral => 13.0,
    });
    let galley = icon_galley(ui, label, font.clone());
    let height = match kind {
        ButtonKind::Small => 26.0,
        ButtonKind::Medium => 30.0,
        ButtonKind::Primary | ButtonKind::Neutral => 32.0,
    };
    let pad_x = match kind {
        ButtonKind::Small | ButtonKind::Medium => 9.0,
        ButtonKind::Primary | ButtonKind::Neutral => 12.0,
    };
    let icon_w = if icon.is_some() {
        if kind == ButtonKind::Small || kind == ButtonKind::Medium {
            16.0
        } else {
            18.0
        }
    } else {
        0.0
    };
    let width = (galley.size().x + icon_w + pad_x * 2.0).max(height + 4.0);
    let (rect, mut response) = ui.allocate_exact_size(
        Vec2::new(width, height),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let hover = animate(
        ui.ctx(),
        response.id.with("hover"),
        enabled && response.hovered(),
    );
    let pressed = enabled && response.is_pointer_button_down_on();
    let radius = match kind {
        ButtonKind::Small | ButtonKind::Medium => 11.0,
        ButtonKind::Primary | ButtonKind::Neutral => BUTTON_RADIUS,
    };
    let fill = mix(CARD, CARD_HOVER, hover);

    {
        let painter = ui.painter();
        if !enabled {
            painter.rect_filled(rect, radius, Color32::from_rgb(0xE7, 0xEB, 0xF6));
            painter.rect_stroke(rect, radius, Stroke::new(1.0_f32, BORDER));
        } else if kind == ButtonKind::Primary {
            paint_primary_state(painter, rect, radius, true, hover, pressed, 1.0);
        } else if pressed {
            // Sink into the surface instead of just changing colour.
            paint_inset_with(painter, rect, radius, CARD);
        } else {
            // Hovering lifts the button slightly.
            paint_button_surface(
                painter,
                rect,
                radius,
                fill,
                Elevation::Low.lift() + 0.2 * hover,
            );
        }
    }

    let text_color = if !enabled {
        FAINT
    } else if kind == ButtonKind::Primary {
        WHITE
    } else {
        mix(INK, BLUE_DEEP, hover)
    };
    let nudge = if pressed {
        Vec2::new(0.0, 1.0)
    } else {
        Vec2::ZERO
    };
    let painter = ui.painter();
    let content_w = galley.size().x + icon_w;
    let start_x = rect.center().x - content_w * 0.5;
    if let Some(icon) = icon {
        let icon_rect = Rect::from_center_size(
            Pos2::new(start_x + icon_w * 0.4, rect.center().y) + nudge,
            Vec2::splat(icon_w * 0.75),
        );
        paint_icon(
            painter,
            icon_rect,
            icon,
            if kind == ButtonKind::Primary {
                WHITE
            } else {
                text_color
            },
        );
    }
    painter.galley(
        Pos2::new(start_x + icon_w, rect.center().y - galley.size().y * 0.5) + nudge,
        galley,
        text_color,
    );
    if enabled {
        paint_focus(ui, &response, radius);
        response = response.on_hover_cursor(CursorIcon::PointingHand);
    } else {
        response = response.on_hover_text("Not available right now");
    }
    response
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    Primary,
    Neutral,
    Medium,
    Small,
}

pub fn button_icon(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    button_body(ui, label, Some(icon), true, ButtonKind::Neutral)
}

#[allow(dead_code)] // Standard-height variant remains available to the theme.
pub fn button_enabled(ui: &mut Ui, icon: Icon, label: &str, enabled: bool) -> Response {
    button_body(ui, label, Some(icon), enabled, ButtonKind::Neutral)
}

pub fn primary_button(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    button_body(ui, label, Some(icon), true, ButtonKind::Primary)
}

pub fn primary_button_enabled(ui: &mut Ui, icon: Icon, label: &str, enabled: bool) -> Response {
    button_body(ui, label, Some(icon), enabled, ButtonKind::Primary)
}

pub fn small_button(ui: &mut Ui, label: &str) -> Response {
    button_body(ui, label, None, true, ButtonKind::Small)
}

pub fn medium_button_icon(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    button_body(ui, label, Some(icon), true, ButtonKind::Medium)
}

pub fn compact_button_icon(ui: &mut Ui, icon: Icon, label: &str, enabled: bool) -> Response {
    button_body(ui, label, Some(icon), enabled, ButtonKind::Small)
}

/// A neutral button that spans the available width.
#[allow(dead_code)]
pub fn wide_button(ui: &mut Ui, label: &str) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hover = animate(ui.ctx(), response.id.with("wide"), response.hovered());
    let pressed = response.is_pointer_button_down_on();
    {
        let painter = ui.painter();
        if pressed {
            paint_inset_with(painter, rect, 12.0, CARD);
        } else {
            paint_button_surface(
                painter,
                rect,
                12.0,
                mix(CARD, CARD_HOVER, hover),
                0.55 + 0.2 * hover,
            );
        }
        painter.text(
            rect.center()
                + if pressed {
                    Vec2::new(0.0, 1.0)
                } else {
                    Vec2::ZERO
                },
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(13.0),
            mix(INK, BLUE_DEEP, hover),
        );
    }
    paint_focus(ui, &response, 12.0);
    response.on_hover_cursor(CursorIcon::PointingHand)
}

pub fn link_button(ui: &mut Ui, label: &str) -> Response {
    let galley = icon_galley(ui, label, FontId::proportional(12.0));
    let (rect, response) =
        ui.allocate_exact_size(galley.size() + Vec2::new(4.0, 4.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let hovered = response.hovered();
    let color = if hovered { BLUE } else { MUTED };
    let painter = ui.painter();
    painter.galley(
        Pos2::new(rect.left() + 2.0, rect.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
    if hovered {
        painter.line_segment(
            [
                Pos2::new(rect.left() + 2.0, rect.bottom() - 1.0),
                Pos2::new(rect.right() - 2.0, rect.bottom() - 1.0),
            ],
            Stroke::new(1.0_f32, color),
        );
    }
    paint_focus(ui, &response, 4.0);
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// A pill-shaped selector. `active` renders the blue gradient; inactive
/// renders a raised, soft surface. The two cross-fade.
pub fn chip(ui: &mut Ui, label: &str, active: bool, width: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            active,
            label,
        )
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hover = animate(ui.ctx(), response.id.with("chip"), response.hovered());
    let on = animate(ui.ctx(), response.id.with("chip-on"), active);
    let pressed = response.is_pointer_button_down_on();
    let nudge = if pressed {
        Vec2::new(0.0, 1.0)
    } else {
        Vec2::ZERO
    };
    {
        let painter = ui.painter();
        if on < 1.0 {
            if pressed && !active {
                paint_inset_with(painter, rect, 13.0, CARD);
            } else {
                painter.add(raised_shape_fade(
                    rect,
                    13.0,
                    mix(CARD, CARD_HOVER, hover),
                    0.55 + 0.2 * hover,
                    1.0 - on,
                ));
            }
        }
        paint_primary_state(painter, rect, 13.0, true, hover, pressed, on);
        painter.text(
            rect.center() + nudge,
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(13.5),
            mix(INK, WHITE, on),
        );
    }
    paint_focus(ui, &response, 13.0);
    response.on_hover_cursor(CursorIcon::PointingHand)
}

pub fn toggle_pill(ui: &mut Ui, label: &str, on: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(140.0, 32.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), on, label)
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hover = animate(ui.ctx(), response.id.with("toggle"), response.hovered());
    let t = animate(ui.ctx(), response.id.with("toggle-on"), on);
    let pressed = response.is_pointer_button_down_on();
    {
        let painter = ui.painter();
        if t < 1.0 {
            if pressed && !on {
                paint_inset_with(painter, rect, 15.0, CARD);
            } else {
                painter.add(raised_shape_fade(
                    rect,
                    15.0,
                    mix(CARD, CARD_HOVER, hover),
                    0.55 + 0.2 * hover,
                    1.0 - t,
                ));
            }
        }
        paint_primary_state(painter, rect, 15.0, true, hover, pressed, t);
        let dot = Pos2::new(rect.left() + 19.0, rect.center().y);
        painter.circle_filled(dot, 4.2, mix(FAINT, WHITE, t));
        if t > 0.0 {
            painter.circle_stroke(
                dot,
                4.2 + 2.2 * t,
                Stroke::new(1.0_f32, Color32::from_white_alpha((110.0 * t) as u8)),
            );
        }
        painter.text(
            Pos2::new(dot.x + 12.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(13.5),
            mix(INK, WHITE, t),
        );
    }
    paint_focus(ui, &response, 15.0);
    response.on_hover_cursor(CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------------
// Checkboxes, sliders, fields
// ---------------------------------------------------------------------------

pub fn checkbox(ui: &mut Ui, checked: &mut bool, label: &str) -> Response {
    let font = FontId::proportional(12.5);
    let available = ui.available_width().max(30.0);
    let galley = ui
        .painter()
        .layout(label.to_owned(), font, INK, (available - 30.0).max(1.0));
    let height = (galley.size().y + 5.0).max(24.0);
    let (rect, mut response) = ui.allocate_exact_size(
        Vec2::new((galley.size().x + 30.0).min(available), height),
        Sense::click(),
    );
    if !ui.is_rect_visible(rect) {
        return response;
    }
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *checked, label)
    });
    let hover = animate(
        ui.ctx(),
        response.id.with("check-hover"),
        response.hovered(),
    );
    let t = animate(ui.ctx(), response.id.with("check-on"), *checked);
    let box_rect = Rect::from_center_size(
        Pos2::new(rect.left() + 10.0, rect.center().y),
        Vec2::splat(19.0),
    );
    {
        let painter = ui.painter();
        if t < 1.0 {
            paint_inset(painter, box_rect, 6.0);
            if hover > 0.0 {
                painter.rect_stroke(
                    box_rect,
                    6.0,
                    Stroke::new(
                        1.3_f32,
                        Color32::from_white_alpha((190.0 * hover * (1.0 - t)) as u8),
                    ),
                );
            }
        }
        if t > 0.0 {
            paint_primary_state(painter, box_rect, 6.0, false, 0.0, false, t);
            // The tick draws itself in along its path.
            let p0 = Pos2::new(box_rect.left() + 4.8, box_rect.center().y);
            let p1 = Pos2::new(box_rect.center().x - 0.8, box_rect.bottom() - 5.4);
            let p2 = Pos2::new(box_rect.right() - 4.4, box_rect.top() + 5.2);
            let l1 = p0.distance(p1);
            let l2 = p1.distance(p2);
            let drawn = (l1 + l2) * t;
            let mut points = vec![p0];
            if drawn <= l1 {
                points.push(p0 + (p1 - p0) * (drawn / l1.max(f32::EPSILON)));
            } else {
                points.push(p1);
                points.push(p1 + (p2 - p1) * ((drawn - l1) / l2.max(f32::EPSILON)));
            }
            painter.add(Shape::line(points, Stroke::new(2.1_f32, WHITE)));
        }
        painter.galley(
            Pos2::new(rect.left() + 30.0, rect.center().y - galley.size().y * 0.5),
            galley,
            mix(INK, BLUE_DEEP, hover),
        );
    }
    if response.has_focus() && keyboard_nav(ui.ctx()) {
        focus_ring(ui.painter(), box_rect, 6.0);
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// A labelled parameter row: name, recessed track, and a value chip.
///
/// If `min < 0 < max` the fill grows from zero (bipolar), with a notch at zero.
pub fn param_slider(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    label: &str,
    value: &mut f32,
    min: f32,
    max: f32,
    text: String,
) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
    let painter = ui.painter();
    painter.text(
        Pos2::new(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        MUTED,
    );
    let chip = Rect::from_min_size(
        Pos2::new(rect.right() - 62.0, rect.center().y - 11.0),
        Vec2::new(62.0, 22.0),
    );
    paint_value_chip(painter, chip, &text, INK);

    let track_left = rect.left() + 86.0;
    let track_right = chip.left() - 10.0;
    let track = Rect::from_min_max(
        Pos2::new(track_left, rect.center().y - 4.0),
        Pos2::new(track_right.max(track_left + 20.0), rect.center().y + 4.0),
    );
    let hit = Rect::from_min_max(
        Pos2::new(track.left() - 6.0, rect.top()),
        Pos2::new(track.right() + 6.0, rect.bottom()),
    );
    let mut response = ui.interact(hit, ui.make_persistent_id(id), Sense::click_and_drag());
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    let mut changed = false;
    if let Some(pointer) = response.interact_pointer_pos() {
        if response.clicked() || response.dragged() {
            let t = ((pointer.x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0);
            *value = min + t * (max - min);
            changed = true;
        }
    }
    if response.has_focus() {
        let fine = if ui.input(|input| input.modifiers.shift) {
            0.2
        } else {
            1.0
        };
        let step = (max - min) / 100.0 * fine;
        if ui.input(|input| input.key_pressed(egui::Key::ArrowLeft)) {
            *value -= step;
            changed = true;
        }
        if ui.input(|input| input.key_pressed(egui::Key::ArrowRight)) {
            *value += step;
            changed = true;
        }
    }
    *value = value.clamp(min, max);

    let hover = animate(
        ui.ctx(),
        response.id.with("slider-hover"),
        response.hovered() || response.dragged(),
    );
    if response.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    } else if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }

    let range = (max - min).max(f32::EPSILON);
    let t = ((*value - min) / range).clamp(0.0, 1.0);
    let knob_x = track.left() + t * track.width();
    let bipolar = min < 0.0 && max > 0.0;
    let zero_x = if bipolar {
        track.left() + ((0.0 - min) / range) * track.width()
    } else {
        track.left()
    };

    paint_inset(painter, track, 4.0);
    if bipolar {
        painter.line_segment(
            [
                Pos2::new(zero_x, track.top() - 3.0),
                Pos2::new(zero_x, track.bottom() + 3.0),
            ],
            Stroke::new(1.0_f32, TICK_STRONG),
        );
    }
    let (x0, x1) = if knob_x >= zero_x {
        (zero_x, knob_x)
    } else {
        (knob_x, zero_x)
    };
    if x1 - x0 > 1.0 {
        let (from, to) = if knob_x >= zero_x {
            (BLUE_LIGHT, BLUE)
        } else {
            (BLUE, BLUE_LIGHT)
        };
        painter.add(gradient_shape(
            Rect::from_min_max(Pos2::new(x0, track.top()), Pos2::new(x1, track.bottom())),
            4.0,
            from,
            to,
            Axis::Horizontal,
        ));
    }
    let knob = Pos2::new(knob_x, track.center().y);
    let radius = 6.8 + 1.4 * hover;
    if hover > 0.0 {
        painter.circle_filled(
            knob,
            radius + 4.5,
            Color32::from_rgba_unmultiplied(52, 120, 246, (34.0 * hover) as u8),
        );
    }
    painter.circle_filled(
        knob + Vec2::new(1.5, 1.8),
        radius,
        Color32::from_rgba_unmultiplied(120, 136, 168, 48),
    );
    painter.circle_filled(knob, radius, WHITE);
    painter.circle_stroke(knob, radius, Stroke::new(2.0_f32, BLUE));
    if response.has_focus() && keyboard_nav(ui.ctx()) {
        painter.circle_stroke(knob, radius + 4.0, Stroke::new(2.0_f32, FOCUS));
    }
    if changed {
        response.mark_changed();
        ui.ctx().request_repaint();
    }
    response.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), f64::from(*value), label));
    response
}

/// Vertical fader with a tick scale and a zero notch. Double-click resets to 0
/// (clamped into range). If `min < 0 < max` the fill grows from the zero line.
pub fn vertical_fader(ui: &mut Ui, value: &mut f32, min: f32, max: f32, height: f32) -> Response {
    let width = ui.available_width().clamp(24.0, 34.0);
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), Vec2::new(8.0, (height - 26.0).max(36.0)));
    let mut changed = false;
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    if response.double_clicked() {
        // Handle the reset on its own, so the pointer mapping below cannot
        // immediately overwrite it with the click position.
        let reset = 0.0_f32.clamp(min, max);
        if (*value - reset).abs() > f32::EPSILON {
            *value = reset;
            changed = true;
        }
    } else if let Some(pointer) = response.interact_pointer_pos() {
        if response.clicked() || response.dragged() {
            let t = ((track.bottom() - pointer.y) / track.height().max(1.0)).clamp(0.0, 1.0);
            let next = min + t * (max - min);
            if (*value - next).abs() > f32::EPSILON {
                *value = next;
                changed = true;
            }
        }
    }
    if response.has_focus() {
        let step = 0.5;
        if ui.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
            *value += step;
            changed = true;
        }
        if ui.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
            *value -= step;
            changed = true;
        }
    }
    *value = value.clamp(min, max);

    let hover = animate(
        ui.ctx(),
        response.id.with("fader-hover"),
        response.hovered() || response.dragged(),
    );
    if response.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    } else if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }
    let focused = response.has_focus() && keyboard_nav(ui.ctx());

    let painter = ui.painter();
    let range = (max - min).max(f32::EPSILON);
    let y_for = |v: f32| track.bottom() - ((v - min) / range).clamp(0.0, 1.0) * track.height();
    let bipolar = min < 0.0 && max > 0.0;
    let zero_y = if bipolar { y_for(0.0) } else { track.bottom() };

    // Tick scale on both sides of the track.
    for i in 0..=4 {
        let t = i as f32 / 4.0;
        if bipolar && (t - (0.0 - min) / range).abs() < 0.02 {
            continue;
        }
        let y = track.bottom() - t * track.height();
        for (x, dir) in [
            (track.left() - 3.0, -1.0_f32),
            (track.right() + 3.0, 1.0_f32),
        ] {
            painter.line_segment(
                [Pos2::new(x, y), Pos2::new(x + dir * 3.0, y)],
                Stroke::new(1.0_f32, TICK),
            );
        }
    }
    if bipolar {
        // Zero notch: a longer, darker tick on both sides.
        for (x, dir) in [
            (track.left() - 3.0, -1.0_f32),
            (track.right() + 3.0, 1.0_f32),
        ] {
            painter.line_segment(
                [Pos2::new(x, zero_y), Pos2::new(x + dir * 5.0, zero_y)],
                Stroke::new(1.5_f32, TICK_STRONG),
            );
        }
    }

    paint_inset(painter, track, 5.0);
    let knob_y = y_for(*value);
    let (y0, y1) = if knob_y <= zero_y {
        (knob_y, zero_y)
    } else {
        (zero_y, knob_y)
    };
    if y1 - y0 > 1.0 {
        painter.add(gradient_shape(
            Rect::from_min_max(Pos2::new(track.left(), y0), Pos2::new(track.right(), y1)),
            5.0,
            BLUE_LIGHT,
            BLUE,
            Axis::Vertical,
        ));
    }
    if bipolar {
        painter.line_segment(
            [
                Pos2::new(track.left() - 1.0, zero_y),
                Pos2::new(track.right() + 1.0, zero_y),
            ],
            Stroke::new(1.0_f32, TICK_STRONG),
        );
    }

    // Hardware-style cap with a blue index line and grip marks.
    let cap_w = (width - 2.0).clamp(20.0, 28.0);
    let cap = Rect::from_center_size(
        Pos2::new(track.center().x, knob_y),
        Vec2::new(cap_w, 15.0 + 1.5 * hover),
    );
    if hover > 0.0 {
        painter.rect_filled(
            cap.expand(3.5),
            9.0,
            Color32::from_rgba_unmultiplied(52, 120, 246, (34.0 * hover) as u8),
        );
    }
    painter.add(blur_rect(
        cap,
        6.0,
        Vec2::new(0.0, 2.5),
        8.0,
        Color32::from_rgba_unmultiplied(120, 136, 168, 70),
    ));
    painter.add(gradient_shape(
        cap,
        6.0,
        WHITE,
        mix(CARD, INSET, 0.5),
        Axis::Vertical,
    ));
    painter.rect_stroke(
        cap,
        6.0,
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(23, 43, 85, 46)),
    );
    for dy in [-4.0_f32, 4.0] {
        painter.line_segment(
            [
                Pos2::new(cap.left() + 5.0, cap.center().y + dy),
                Pos2::new(cap.right() - 5.0, cap.center().y + dy),
            ],
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(23, 43, 85, 34)),
        );
    }
    painter.line_segment(
        [
            Pos2::new(cap.left() + 4.0, cap.center().y),
            Pos2::new(cap.right() - 4.0, cap.center().y),
        ],
        Stroke::new(2.2_f32, BLUE),
    );
    if focused {
        focus_ring(painter, cap, 6.0);
    }
    if changed {
        response.mark_changed();
    }
    response.on_hover_text("Drag to adjust · Up/Down: 0.5 dB · double-click to reset")
}

/// Inset search box with a magnifier icon.
#[allow(deprecated)]
pub fn search_field(ui: &mut Ui, text: &mut String, hint: &str) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::hover());
    // The well is painted first so the text edit sits on top of it.
    paint_inset(ui.painter(), rect, 14.0);
    let inner = ui.allocate_new_ui(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                Pos2::new(rect.left() + 36.0, rect.top() + 5.0),
                Pos2::new(rect.right() - 12.0, rect.bottom() - 5.0),
            ))
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.style_mut().visuals.extreme_bg_color = Color32::TRANSPARENT;
            ui.style_mut().visuals.override_text_color = Some(INK);
            ui.add(
                egui::TextEdit::singleline(text)
                    .id(Id::new("profile_search"))
                    .frame(false)
                    .hint_text(RichText::new(hint).color(FAINT))
                    .desired_width(f32::INFINITY),
            )
        },
    );
    let icon_rect = Rect::from_center_size(
        Pos2::new(rect.left() + 19.0, rect.center().y),
        Vec2::splat(16.0),
    );
    let focused = inner.inner.has_focus();
    paint_icon(
        ui.painter(),
        icon_rect,
        Icon::Search,
        if focused { BLUE } else { MUTED },
    );
    if focused {
        field_focus(ui.painter(), rect, 14.0);
    }
    inner.inner
}

#[allow(deprecated)]
pub fn text_field(ui: &mut Ui, text: &mut String, hint: &str) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::hover());
    paint_inset(ui.painter(), rect, 12.0);
    let inner = ui.allocate_new_ui(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(12.0, 5.0)))
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.style_mut().visuals.extreme_bg_color = Color32::TRANSPARENT;
            ui.style_mut().visuals.override_text_color = Some(INK);
            ui.add(
                egui::TextEdit::singleline(text)
                    .id(Id::new("preset_name"))
                    .frame(false)
                    .hint_text(RichText::new(hint).color(FAINT))
                    .desired_width(f32::INFINITY),
            )
        },
    );
    if inner.inner.has_focus() {
        field_focus(ui.painter(), rect, 12.0);
    }
    inner.inner
}

// ---------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------

fn arc_points(center: Pos2, radius: f32, start: f32, end: f32, segments: usize) -> Vec<Pos2> {
    (0..=segments)
        .map(|index| {
            let t = index as f32 / segments as f32;
            let angle = start + (end - start) * t;
            Pos2::new(
                center.x + radius * angle.cos(),
                center.y + radius * angle.sin(),
            )
        })
        .collect()
}

pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let center = rect.center();
    let r = rect.width().min(rect.height()) * 0.5;
    let stroke = Stroke::new((r * 0.16).clamp(1.2, 2.0), color);
    let thin = Stroke::new((r * 0.12).clamp(1.0, 1.6), color);

    // Line helpers with round caps, so strokes end softly instead of squared off.
    let seg = |a: Pos2, b: Pos2, s: Stroke| {
        painter.line_segment([a, b], s);
        painter.circle_filled(a, s.width * 0.5, s.color);
        painter.circle_filled(b, s.width * 0.5, s.color);
    };
    let path = |points: Vec<Pos2>, s: Stroke| {
        if let (Some(first), Some(last)) = (points.first().copied(), points.last().copied()) {
            painter.circle_filled(first, s.width * 0.5, s.color);
            painter.circle_filled(last, s.width * 0.5, s.color);
        }
        painter.add(Shape::line(points, s));
    };

    match icon {
        Icon::Headphones => {
            path(
                arc_points(center + Vec2::new(0.0, 1.0), r * 0.72, PI, 2.0 * PI, 20),
                stroke,
            );
            let pad = Vec2::new(r * 0.22, r * 0.38);
            for side in [-1.0, 1.0] {
                let pad_rect = Rect::from_center_size(
                    Pos2::new(center.x + side * r * 0.72, center.y + r * 0.18),
                    pad * 2.0,
                );
                painter.rect_filled(pad_rect, pad.x, color);
            }
        }
        Icon::Sliders => {
            for (index, x) in [-0.55f32, 0.0, 0.55].iter().enumerate() {
                let x = center.x + x * r;
                seg(
                    Pos2::new(x, center.y - r * 0.75),
                    Pos2::new(x, center.y + r * 0.75),
                    thin,
                );
                let y = center.y + [-0.4, 0.3, -0.15][index] * r;
                painter.circle_filled(Pos2::new(x, y), r * 0.18, color);
            }
        }
        Icon::Speaker => {
            let body = vec![
                Pos2::new(center.x - r * 0.8, center.y - r * 0.3),
                Pos2::new(center.x - r * 0.4, center.y - r * 0.3),
                Pos2::new(center.x + r * 0.02, center.y - r * 0.72),
                Pos2::new(center.x + r * 0.02, center.y + r * 0.72),
                Pos2::new(center.x - r * 0.4, center.y + r * 0.3),
                Pos2::new(center.x - r * 0.8, center.y + r * 0.3),
            ];
            painter.add(Shape::convex_polygon(body, color, Stroke::NONE));
            for radius in [0.42f32, 0.66] {
                path(
                    arc_points(center + Vec2::new(r * 0.1, 0.0), r * radius, -0.8, 0.8, 10),
                    thin,
                );
            }
        }
        Icon::SpeakerOff => {
            paint_icon(
                painter,
                rect.translate(Vec2::new(-r * 0.18, 0.0)),
                Icon::Speaker,
                color,
            );
            seg(
                Pos2::new(center.x + r * 0.45, center.y - r * 0.45),
                Pos2::new(center.x + r * 0.95, center.y + r * 0.45),
                stroke,
            );
        }
        Icon::Compare => {
            let small = Rect::from_center_size(
                center + Vec2::new(-r * 0.22, -r * 0.22),
                Vec2::splat(r * 0.9),
            );
            let big = Rect::from_center_size(
                center + Vec2::new(r * 0.24, r * 0.24),
                Vec2::splat(r * 0.9),
            );
            painter.rect_stroke(small, r * 0.2, thin);
            painter.rect_stroke(big, r * 0.2, thin);
        }
        Icon::Preset => {
            let doc = Rect::from_center_size(center, Vec2::new(r * 1.2, r * 1.5));
            painter.rect_stroke(doc, r * 0.18, thin);
            for y in [-0.24f32, 0.06, 0.36].iter() {
                seg(
                    Pos2::new(doc.left() + r * 0.22, center.y + y * r),
                    Pos2::new(doc.right() - r * 0.22, center.y + y * r),
                    thin,
                );
            }
        }
        Icon::Settings => {
            // A toothed gear outline with a hub, so it no longer reads as a sun.
            let teeth = 8;
            let step = TAU / teeth as f32;
            let mut points = Vec::with_capacity(teeth * 4);
            for i in 0..teeth {
                let a = i as f32 * step;
                for (da, radius) in [
                    (-step * 0.30, 0.62),
                    (-step * 0.17, 0.88),
                    (step * 0.17, 0.88),
                    (step * 0.30, 0.62),
                ] {
                    let angle = a + da;
                    points.push(Pos2::new(
                        center.x + r * radius * angle.cos(),
                        center.y + r * radius * angle.sin(),
                    ));
                }
            }
            painter.add(Shape::closed_line(points, thin));
            painter.circle_stroke(center, r * 0.27, thin);
        }
        Icon::Search => {
            painter.circle_stroke(center + Vec2::new(-r * 0.16, -r * 0.16), r * 0.5, stroke);
            seg(
                center + Vec2::new(r * 0.22, r * 0.22),
                center + Vec2::new(r * 0.72, r * 0.72),
                stroke,
            );
        }
        Icon::Play => {
            painter.add(Shape::convex_polygon(
                vec![
                    center + Vec2::new(-r * 0.42, -r * 0.62),
                    center + Vec2::new(r * 0.64, 0.0),
                    center + Vec2::new(-r * 0.42, r * 0.62),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Stop => {
            painter.rect_filled(
                Rect::from_center_size(center, Vec2::splat(r * 1.1)),
                r * 0.16,
                color,
            );
        }
        Icon::Refresh | Icon::Restore => {
            path(arc_points(center, r * 0.68, -0.4, 5.2, 22), stroke);
            let tip = Pos2::new(
                center.x + r * 0.68 * (-0.4f32).cos(),
                center.y + r * 0.68 * (-0.4f32).sin(),
            );
            painter.add(Shape::convex_polygon(
                vec![
                    tip + Vec2::new(-r * 0.2, -r * 0.02),
                    tip + Vec2::new(r * 0.13, -r * 0.3),
                    tip + Vec2::new(r * 0.22, r * 0.12),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Undo | Icon::Redo => {
            let flip = if icon == Icon::Redo { -1.0 } else { 1.0 };
            path(arc_points(center, r * 0.58, 3.4, 6.0, 14), stroke);
            let tip = Pos2::new(center.x + flip * r * 0.52, center.y - r * 0.22);
            painter.add(Shape::convex_polygon(
                vec![
                    tip + Vec2::new(0.0, -r * 0.25),
                    tip + Vec2::new(flip * r * 0.3, 0.0),
                    tip + Vec2::new(0.0, r * 0.25),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Bars => {
            for (index, height) in [0.45f32, 0.8, 0.6, 0.95].iter().enumerate() {
                let x = center.x + (index as f32 - 1.5) * r * 0.42;
                painter.rect_filled(
                    Rect::from_center_size(
                        Pos2::new(x, center.y + r * (0.85 - height * 0.85)),
                        Vec2::new(r * 0.24, r * height * 1.5),
                    ),
                    r * 0.1,
                    color,
                );
            }
        }
        Icon::Layers => {
            for (index, y) in [-0.38f32, 0.0, 0.38].iter().enumerate() {
                painter.rect_stroke(
                    Rect::from_center_size(
                        center + Vec2::new(0.0, y * r),
                        Vec2::new(r * 1.35, r * 0.42),
                    ),
                    r * 0.12,
                    if index == 1 { thin } else { stroke },
                );
            }
        }
        Icon::Warning => {
            painter.add(Shape::convex_polygon(
                vec![
                    center + Vec2::new(0.0, -r * 0.85),
                    center + Vec2::new(r * 0.85, r * 0.62),
                    center + Vec2::new(-r * 0.85, r * 0.62),
                ],
                Color32::TRANSPARENT,
                stroke,
            ));
            seg(
                center + Vec2::new(0.0, -r * 0.3),
                center + Vec2::new(0.0, r * 0.14),
                stroke,
            );
            painter.circle_filled(center + Vec2::new(0.0, r * 0.4), r * 0.1, color);
        }
        Icon::Diagnostics => {
            let points: Vec<Pos2> = (0..=12)
                .map(|index| {
                    let t = index as f32 / 12.0;
                    let y = (t * TAU * 2.0).sin() * r * 0.38;
                    Pos2::new(center.x - r * 0.85 + t * r * 1.7, center.y + y)
                })
                .collect();
            path(points, stroke);
        }
        Icon::Help => {
            painter.circle_stroke(center, r * 0.84, thin);
            path(
                arc_points(
                    center + Vec2::new(0.0, -r * 0.22),
                    r * 0.28,
                    PI,
                    TAU + FRAC_PI_2,
                    15,
                ),
                thin,
            );
            seg(
                center + Vec2::new(0.0, r * 0.06),
                center + Vec2::new(0.0, r * 0.22),
                thin,
            );
            painter.circle_filled(center + Vec2::new(0.0, r * 0.49), r * 0.08, color);
        }
    }
}

pub fn star(ui: &mut Ui, filled: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            ui.is_enabled(),
            filled,
            "Favorite profile",
        )
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hover = animate(ui.ctx(), response.id.with("star"), response.hovered());
    if hover > 0.0 {
        ui.painter().circle_filled(
            rect.center(),
            13.0,
            Color32::from_rgba_unmultiplied(255, 255, 255, (190.0 * hover) as u8),
        );
    }
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        if filled { "★" } else { "☆" },
        FontId::proportional(18.0),
        if filled {
            WARN
        } else {
            mix(FAINT, MUTED, hover)
        },
    );
    paint_focus(ui, &response, 14.0);
    response.on_hover_cursor(CursorIcon::PointingHand)
}

pub fn icon_badge(ui: &mut Ui, icon: Icon) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
    paint_primary(ui.painter(), rect, 10.0, false, 0.0);
    paint_icon(ui.painter(), rect.shrink(7.0), icon, WHITE);
    rect
}

pub fn warning(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(17.0), Sense::hover());
        paint_icon(ui.painter(), rect.shrink(1.0), Icon::Warning, WARN);
        // WARN_TEXT is dark enough to read at this size; WARN itself is not.
        ui.label(RichText::new(text).size(12.5).color(WARN_TEXT));
    });
}

pub fn tooltip_chip(painter: &egui::Painter, anchor: Pos2, text: &str, bounds: Rect) {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(12.0), INK);
    let size = galley.size() + Vec2::new(18.0, 12.0);
    let mut min = anchor + Vec2::new(12.0, -size.y - 8.0);
    min.x = min.x.clamp(
        bounds.left() + 4.0,
        (bounds.right() - size.x - 4.0).max(bounds.left() + 4.0),
    );
    min.y = min.y.clamp(
        bounds.top() + 4.0,
        (bounds.bottom() - size.y - 4.0).max(bounds.top() + 4.0),
    );
    let rect = Rect::from_min_size(min, size);
    paint_raised(painter, rect, 9.0, WHITE);
    painter.galley(
        Pos2::new(rect.left() + 9.0, rect.center().y - galley.size().y * 0.5),
        galley,
        INK,
    );
}

/// The existing spectrum, expressed as a compact mirrored rainbow-dot display.
/// Geometry is proportional to the current centre card; bars are actual FFT
/// magnitudes. The mirror is visual only, not a second channel/analysis panel.
pub fn spectrum(
    ui: &mut Ui,
    levels: &[f32; SPECTRUM_BANDS],
    height: f32,
    hint: Option<&str>,
) -> Rect {
    let (response, painter) =
        ui.allocate_painter(Vec2::new(ui.available_width(), height), Sense::hover());
    let rect = response.rect;
    paint_inset_with(&painter, rect, 11.0, Color32::from_rgb(0x23, 0x1F, 0x20));
    let plot = Rect::from_min_max(
        rect.min + Vec2::new(12.0, 8.0),
        rect.max - Vec2::new(12.0, 20.0),
    );
    let center_y = plot.center().y;
    painter.line_segment(
        [
            Pos2::new(plot.left(), center_y),
            Pos2::new(plot.right(), center_y),
        ],
        Stroke::new(0.5_f32, Color32::from_white_alpha(22)),
    );
    let palette = [
        Color32::from_rgb(30, 172, 246),
        Color32::from_rgb(59, 83, 255),
        Color32::from_rgb(231, 34, 186),
        Color32::from_rgb(249, 46, 80),
        Color32::from_rgb(255, 133, 35),
        Color32::from_rgb(222, 225, 40),
        Color32::from_rgb(74, 207, 101),
    ];
    let columns = ((plot.width() / 4.0) as usize).clamp(2, SPECTRUM_BANDS);
    let step_x = plot.width() / columns as f32;
    let step_y = 3.0;
    let rows = (plot.height() * 0.5 / step_y).floor().max(1.0) as usize;
    let radius = (step_x * 0.20).clamp(0.8, 1.35);
    // Auto-range only the picture, not the audio or the absolute hover readout.
    // Quiet streams remain visible; a true floor/silent input still draws no bars.
    let visual_ceiling = levels.iter().copied().fold(-40.0_f32, f32::max).min(6.0);
    for column in 0..columns {
        let t = column as f32 / (columns - 1) as f32;
        let band = (t * (SPECTRUM_BANDS - 1) as f32).round() as usize;
        let magnitude = ((levels[band] - SPECTRUM_FLOOR_DB) / (visual_ceiling - SPECTRUM_FLOOR_DB))
            .clamp(0.0, 1.0);
        let lit = (magnitude * rows as f32).round() as usize;
        let palette_position = t * (palette.len() - 1) as f32;
        let left = (palette_position.floor() as usize).min(palette.len() - 2);
        let color = mix(
            palette[left],
            palette[left + 1],
            palette_position - left as f32,
        );
        let x = plot.left() + (column as f32 + 0.5) * step_x;
        for row in 0..lit {
            let fade = 1.0 - row as f32 / rows as f32 * 0.55;
            let c = color.gamma_multiply(fade);
            let dy = (row as f32 + 0.5) * step_y;
            painter.circle_filled(Pos2::new(x, center_y - dy), radius, c);
            painter.circle_filled(Pos2::new(x, center_y + dy), radius, c);
        }
    }
    for (frequency, label) in [
        (20.0_f32, "20 Hz"),
        (100.0, "100"),
        (1000.0, "1k"),
        (10000.0, "10k"),
        (20000.0, "20k"),
    ] {
        let t = (frequency / 20.0).ln() / 1000f32.ln();
        painter.text(
            Pos2::new(plot.left() + t * plot.width(), rect.bottom() - 10.0),
            if frequency == 20.0 {
                Align2::LEFT_CENTER
            } else if frequency == 20000.0 {
                Align2::RIGHT_CENTER
            } else {
                Align2::CENTER_CENTER
            },
            label,
            FontId::proportional(10.0),
            Color32::from_rgb(177, 183, 199),
        );
    }
    if let Some(hint) = hint {
        painter.text(
            plot.center(),
            Align2::CENTER_CENTER,
            hint,
            FontId::proportional(12.0),
            Color32::from_rgb(190, 197, 214),
        );
    } else if let Some(pointer) = response.hover_pos() {
        let t = ((pointer.x - plot.left()) / plot.width()).clamp(0.0, 1.0);
        let band = (t * (SPECTRUM_BANDS - 1) as f32).round() as usize;
        let frequency = 20.0 * 1000f32.powf(band as f32 / (SPECTRUM_BANDS - 1) as f32);
        let text = format!("{frequency:.0} Hz · {:.1} dBFS", levels[band]);
        tooltip_chip(&painter, pointer, &text, rect);
    }
    rect
}
