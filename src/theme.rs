//! Tema visual inspirado en las apps de ArtCraft: fondo casi negro, tarjetas redondeadas
//! con borde fino, barra lateral, tipografía sans, botón principal blanco, selección en
//! azul, avisos en ámbar y pasos de progreso con círculos.

use crate::history::SourceKind;
use eframe::egui::{
    self, Button, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, Response,
    RichText, Sense, Shadow, Stroke, TextStyle, Ui, pos2, vec2,
};
use std::sync::Arc;

pub const SIDEBAR: Color32 = Color32::from_rgb(16, 16, 20);
pub const BG: Color32 = Color32::from_rgb(17, 19, 21);
pub const CARD: Color32 = Color32::from_rgb(26, 28, 30);
pub const CARD_LINE: Color32 = Color32::from_rgb(44, 46, 48);
pub const DIVIDER: Color32 = Color32::from_rgb(35, 37, 38);
pub const INPUT_LINE: Color32 = Color32::from_rgb(66, 68, 71);
pub const HOVER: Color32 = Color32::from_rgb(44, 46, 47);
pub const TEXT: Color32 = Color32::from_rgb(236, 237, 238);
pub const MUTED: Color32 = Color32::from_rgb(141, 143, 146);
pub const PRIMARY: Color32 = Color32::from_rgb(243, 241, 237);
pub const BLUE: Color32 = Color32::from_rgb(47, 111, 224);
pub const SEL_FILL: Color32 = Color32::from_rgb(30, 44, 69);
pub const SEL_LINE: Color32 = Color32::from_rgb(41, 89, 171);
pub const GREEN: Color32 = Color32::from_rgb(90, 194, 141);
pub const WARN: Color32 = Color32::from_rgb(222, 164, 54);
pub const DANGER: Color32 = Color32::from_rgb(229, 96, 96);
pub const LINK: Color32 = Color32::from_rgb(138, 180, 255);

const SEMIBOLD: &str = "semibold";
const RADIUS: CornerRadius = CornerRadius::same(6);

/// Usa Segoe UI (la fuente de Windows) si está; si no, se queda la de egui.
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let dir = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into()).join("Fonts");
    let mut load = |name: &str, file: &str| {
        let bytes = std::fs::read(dir.join(file)).ok()?;
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
        Some(name.to_owned())
    };
    let regular = load("segoe", "segoeui.ttf");
    let semibold = load("segoe-semibold", "seguisb.ttf");
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    if let Some(name) = regular {
        proportional.insert(0, name);
    }
    let mut family = proportional.clone();
    if let Some(name) = semibold {
        family.insert(0, name);
    }
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), family);
    fonts
}

/// Aplica el tema a los modos claro y oscuro y fuerza el oscuro.
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Body, FontId::proportional(13.5)),
            (TextStyle::Monospace, FontId::monospace(12.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Heading, FontId::new(17.0, FontFamily::Name(SEMIBOLD.into()))),
        ]
        .into();

        let s = &mut style.spacing;
        s.item_spacing = vec2(8.0, 7.0);
        s.button_padding = vec2(12.0, 6.0);
        s.interact_size.y = 28.0;
        s.window_margin = Margin::same(16);

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.window_stroke = Stroke::new(1.0, CARD_LINE);
        v.window_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(120) };
        v.popup_shadow = Shadow { offset: [0, 4], blur: 12, spread: 0, color: Color32::from_black_alpha(100) };
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.extreme_bg_color = CARD;
        v.faint_bg_color = Color32::from_rgb(30, 32, 34);
        v.hyperlink_color = LINK;
        v.warn_fg_color = WARN;
        v.error_fg_color = DANGER;
        v.selection.bg_fill = SEL_FILL;
        v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(110, 160, 245));
        v.override_text_color = None;

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = CARD;
        w.noninteractive.weak_bg_fill = CARD;
        w.noninteractive.bg_stroke = Stroke::new(1.0, DIVIDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(200, 201, 203));
        for (state, fill, line) in [
            (&mut w.inactive, CARD, INPUT_LINE),
            (&mut w.hovered, HOVER, Color32::from_rgb(96, 98, 101)),
            (&mut w.active, HOVER, MUTED),
            (&mut w.open, HOVER, MUTED),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, line);
            state.fg_stroke = Stroke::new(1.0, TEXT);
            state.expansion = 0.0;
        }
        for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            state.corner_radius = RADIUS;
        }
    });
}

/// Texto en seminegrita (títulos, nombres).
pub fn semibold(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text.into()).font(FontId::new(size, FontFamily::Name(SEMIBOLD.into()))).color(TEXT)
}

/// Texto pequeño y apagado: etiquetas y notas.
pub fn meta(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(12.0).color(MUTED)
}

/// Valor destacado en verde (algo que está bien: conectado, versión).
pub fn readout(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(12.0).color(GREEN)
}

/// Tarjeta: el contenedor básico de la interfaz.
pub fn card() -> Frame {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, CARD_LINE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
}

/// Tarjeta con borde de color para lo que pide atención (ámbar) o falló (rojo).
pub fn alert_card(color: Color32) -> Frame {
    card().stroke(Stroke::new(1.0, color.gamma_multiply(0.85)))
}

/// Título de un paso: círculo con el número y el texto en seminegrita.
pub fn section(ui: &mut Ui, number: &str, title: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        ui.painter().circle_stroke(rect.center(), 10.5, Stroke::new(1.0, INPUT_LINE));
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, number, FontId::proportional(11.5), TEXT);
        ui.label(semibold(title, 15.0));
    });
    ui.add_space(4.0);
}

/// Botón principal: claro con texto oscuro.
pub fn primary(text: &str) -> Button<'static> {
    Button::new(semibold(text, 13.0).color(BG))
        .fill(PRIMARY)
        .stroke(Stroke::NONE)
        .corner_radius(RADIUS)
        .min_size(vec2(0.0, 32.0))
}

/// Botón secundario: del color de la tarjeta, con borde fino.
pub fn secondary(text: &str) -> Button<'static> {
    Button::new(RichText::new(text).size(13.0).color(TEXT)).corner_radius(RADIUS).min_size(vec2(0.0, 30.0))
}

/// Botón para acciones destructivas.
pub fn danger(text: &str) -> Button<'static> {
    Button::new(RichText::new(text).size(13.0).color(DANGER))
        .stroke(Stroke::new(1.0, DANGER.gamma_multiply(0.5)))
        .corner_radius(RADIUS)
        .min_size(vec2(0.0, 30.0))
}

/// Opción elegible (como "10 canciones"): azul cuando está elegida.
pub fn chip(selected: bool, text: &str) -> Button<'static> {
    let (fill, line) = if selected { (SEL_FILL, SEL_LINE) } else { (CARD, INPUT_LINE) };
    Button::new(RichText::new(text).size(13.5).color(TEXT))
        .fill(fill)
        .stroke(Stroke::new(1.0, line))
        .corner_radius(RADIUS)
        .min_size(vec2(48.0, 34.0))
}

/// Fila de la barra lateral: resaltada si está elegida, con un punto de color opcional.
pub fn sidebar_item(ui: &mut Ui, selected: bool, text: &str, dot: Option<Color32>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let fill = if selected {
        HOVER
    } else if resp.hovered() {
        Color32::from_rgb(28, 30, 32)
    } else {
        Color32::TRANSPARENT
    };
    let painter = ui.painter();
    painter.rect_filled(rect, RADIUS, fill);
    let color = if selected || resp.hovered() { TEXT } else { Color32::from_rgb(200, 201, 203) };
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(13.5), color);
    let text_right = rect.right() - if dot.is_some() { 24.0 } else { 8.0 };
    let clip = egui::Rect::from_min_max(rect.min, pos2(text_right, rect.max.y));
    let pos = pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0);
    painter.with_clip_rect(clip).galley(pos, galley, color);
    if let Some(dot) = dot {
        painter.circle_filled(pos2(rect.right() - 12.0, rect.center().y), 3.5, dot);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Etiqueta redondeada con fondo teñido ("Lista", "Audio original").
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).color(color));
        });
}

/// Píldora con punto de color y texto (estado en la barra superior).
pub fn pill(ui: &mut Ui, dot: Color32, text: &str) -> Response {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, CARD_LINE))
        .corner_radius(RADIUS)
        .inner_margin(Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, dot);
                ui.label(RichText::new(text).size(12.0).color(TEXT));
            });
        })
        .response
}

/// Fila "clave ........ valor" de una tarjeta de datos.
pub fn row(ui: &mut Ui, key: &str, value: RichText) -> Response {
    ui.horizontal(|ui| {
        ui.label(meta(key));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(value)).inner
    })
    .inner
}

/// Fila de estado de una herramienta: punto verde/ámbar, nombre y versión o problema.
pub fn status(ui: &mut Ui, name: &str, busy: bool, state: Option<&Result<String, String>>, missing: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
        let (dot, value, hover) = match state {
            _ if busy => (MUTED, meta("comprobando…"), None),
            Some(Ok(v)) => (GREEN, meta(v.as_str()), None),
            Some(Err(e)) => (WARN, RichText::new(missing).size(12.0).color(WARN), Some(e.as_str())),
            None => (MUTED, meta(missing), None),
        };
        ui.painter().circle_filled(rect.center(), 3.5, dot);
        ui.label(RichText::new(name).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let resp = ui.label(value);
            if let Some(hover) = hover {
                resp.on_hover_text(hover);
            }
        });
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Done,
    Active,
    Pending,
}

/// Paso de un proceso: círculo claro con marca si está hecho, anillo azul si está en curso.
pub fn step(ui: &mut Ui, state: Step, label: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        let c = rect.center();
        let painter = ui.painter();
        match state {
            Step::Done => {
                painter.circle_filled(c, 8.0, PRIMARY);
                let mark = Stroke::new(1.6, BG);
                painter.line_segment([c + vec2(-3.6, 0.2), c + vec2(-1.1, 2.7)], mark);
                painter.line_segment([c + vec2(-1.1, 2.7), c + vec2(3.7, -2.6)], mark);
            }
            Step::Active => {
                painter.circle_stroke(c, 7.5, Stroke::new(1.5, BLUE));
                painter.circle_filled(c, 3.5, BLUE);
            }
            Step::Pending => {
                painter.circle_stroke(c, 7.5, Stroke::new(1.2, INPUT_LINE));
            }
        }
        let color = if state == Step::Pending { MUTED } else { TEXT };
        ui.label(RichText::new(label).color(color));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let text = match state {
                Step::Done => "Hecho",
                Step::Active => "En curso",
                Step::Pending => "",
            };
            ui.label(meta(text));
        });
    });
}

/// Barra de progreso fina y azul.
pub fn progress_bar(ui: &mut Ui, fraction: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
    let radius = CornerRadius::same(2);
    ui.painter().rect_filled(rect, radius, CARD_LINE);
    let mut filled = rect;
    filled.set_width(rect.width() * fraction.clamp(0.0, 1.0));
    ui.painter().rect_filled(filled, radius, BLUE);
}

/// Etiqueta de color para el tipo de fuente de cada canción.
pub fn kind_badge(ui: &mut Ui, kind: SourceKind) {
    let color = match kind {
        SourceKind::AudioOriginal => GREEN,
        SourceKind::AudioOficial => Color32::from_rgb(96, 200, 220),
        SourceKind::VideoOficial => WARN,
        SourceKind::Otro => MUTED,
    };
    badge(ui, kind.label(), color);
}
