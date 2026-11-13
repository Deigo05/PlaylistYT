//! Tema visual inspirado en las apps de ArtCraft: oscuro y sobrio, esquinas rectas,
//! rótulos en mayúsculas con fuente monoespaciada, secciones numeradas ("01 / …"),
//! botón principal blanco y barra de estado con indicadores en verde.

use crate::history::SourceKind;
use eframe::egui::{
    self, Button, Color32, CornerRadius, FontId, Margin, RichText, Shadow, Stroke, TextStyle, Ui, vec2,
};

pub const BG: Color32 = Color32::from_rgb(18, 19, 22);
pub const INSET: Color32 = Color32::from_rgb(12, 13, 15);
pub const PANEL: Color32 = Color32::from_rgb(23, 24, 28);
pub const SURFACE: Color32 = Color32::from_rgb(30, 32, 37);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(40, 42, 48);
pub const LINE: Color32 = Color32::from_rgb(44, 46, 52);
pub const TEXT: Color32 = Color32::from_rgb(242, 241, 238);
pub const MUTED: Color32 = Color32::from_rgb(138, 140, 147);
pub const ACCENT: Color32 = Color32::from_rgb(110, 224, 122);
pub const SELECTION: Color32 = Color32::from_rgb(52, 86, 150);
pub const WARN: Color32 = Color32::from_rgb(232, 160, 64);
pub const DANGER: Color32 = Color32::from_rgb(226, 84, 84);
pub const LINK: Color32 = Color32::from_rgb(140, 178, 255);

/// Aplica el tema a los modos claro y oscuro y fuerza el oscuro.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::monospace(10.5)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Monospace, FontId::monospace(12.5)),
            (TextStyle::Button, FontId::monospace(12.0)),
            (TextStyle::Heading, FontId::proportional(21.0)),
        ]
        .into();

        let s = &mut style.spacing;
        s.item_spacing = vec2(8.0, 7.0);
        s.button_padding = vec2(12.0, 5.0);
        s.interact_size.y = 26.0;
        s.window_margin = Margin::same(14);

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::new(1.0, LINE);
        v.window_shadow = Shadow::NONE;
        v.popup_shadow = Shadow::NONE;
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.extreme_bg_color = INSET;
        v.faint_bg_color = Color32::from_rgb(24, 25, 29);
        v.hyperlink_color = LINK;
        v.warn_fg_color = WARN;
        v.error_fg_color = DANGER;
        v.selection.bg_fill = SELECTION;
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.override_text_color = None;

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = PANEL;
        w.noninteractive.weak_bg_fill = PANEL;
        w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        for (state, fill, stroke) in [
            (&mut w.inactive, SURFACE, Stroke::new(1.0, LINE)),
            (&mut w.hovered, SURFACE_HOVER, Stroke::new(1.0, MUTED)),
            (&mut w.active, SURFACE_HOVER, Stroke::new(1.0, TEXT)),
            (&mut w.open, SURFACE, Stroke::new(1.0, MUTED)),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = stroke;
            state.fg_stroke = Stroke::new(1.0, TEXT);
            state.expansion = 0.0;
        }
        for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            state.corner_radius = CornerRadius::ZERO;
        }
    });
}

/// Rótulo de sección: "01 / NUEVA PLAYLIST" seguido de una línea fina.
pub fn section(ui: &mut Ui, number: &str, title: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{number} /")).monospace().size(11.0).color(MUTED));
        ui.label(RichText::new(title.to_uppercase()).monospace().size(11.0).color(TEXT));
    });
    ui.separator();
}

/// Cabecera de panel lateral en mayúsculas ("HISTORIAL").
pub fn panel_header(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title.to_uppercase()).monospace().size(11.0).color(MUTED));
    ui.separator();
}

/// Botón principal: blanco con texto oscuro, en mayúsculas.
pub fn primary(text: &str) -> Button<'static> {
    Button::new(RichText::new(text.to_uppercase()).monospace().size(12.0).color(BG).strong())
        .fill(TEXT)
        .stroke(Stroke::NONE)
        .min_size(vec2(0.0, 30.0))
}

/// Botón secundario: contorno fino, en mayúsculas.
pub fn secondary(text: &str) -> Button<'static> {
    Button::new(RichText::new(text.to_uppercase()).monospace().size(11.5).color(TEXT))
        .fill(BG)
        .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(242, 241, 238, 90)))
}

/// Botón para acciones destructivas: contorno rojo.
pub fn danger(text: &str) -> Button<'static> {
    Button::new(RichText::new(text.to_uppercase()).monospace().size(11.5).color(DANGER))
        .fill(BG)
        .stroke(Stroke::new(1.0, DANGER.gamma_multiply(0.6)))
}

/// Texto en mayúsculas pequeño y apagado, para metadatos.
pub fn meta(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).monospace().size(11.0).color(MUTED)
}

/// Lectura tipo "pantalla LCD" en verde (cuota, versiones).
pub fn readout(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).monospace().size(11.5).color(ACCENT)
}

/// Marco hundido y oscuro, como la consola de una app de producción.
pub fn inset() -> egui::Frame {
    egui::Frame::new()
        .fill(INSET)
        .stroke(Stroke::new(1.0, LINE))
        .inner_margin(Margin::symmetric(10, 8))
}

/// Indicador de estado de la barra inferior: nombre apagado y valor en verde,
/// o en naranja si falla (con el motivo al pasar el ratón).
pub fn status(ui: &mut Ui, name: &str, busy: bool, state: Option<&Result<String, String>>, missing: &str) {
    ui.label(meta(name.to_uppercase()));
    match state {
        _ if busy => {
            ui.add(egui::Spinner::new().size(11.0));
        }
        Some(Ok(v)) => {
            ui.label(readout(v.as_str()));
        }
        Some(Err(e)) => {
            ui.label(RichText::new(missing.to_uppercase()).monospace().size(11.5).color(WARN))
                .on_hover_text(e);
        }
        None => {
            ui.label(meta(missing.to_uppercase()));
        }
    }
}

/// Etiqueta de color para el tipo de fuente de cada canción.
pub fn kind_badge(ui: &mut Ui, kind: SourceKind) {
    let color = match kind {
        SourceKind::AudioOriginal => ACCENT,
        SourceKind::AudioOficial => Color32::from_rgb(96, 200, 220),
        SourceKind::VideoOficial => WARN,
        SourceKind::Otro => MUTED,
    };
    egui::Frame::new()
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.7)))
        .inner_margin(Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(kind.label().to_uppercase()).monospace().size(10.0).color(color));
        });
}
