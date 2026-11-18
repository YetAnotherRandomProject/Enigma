//! Apparence de l'application : angles droits, contours fins, boutons aux
//! marges généreuses, panneaux gris neutres et accent bleu (dans l'esprit
//! des logiciels de création type Photoshop). Thèmes sombre et clair.

use eframe::egui::{
    self, Color32, CornerRadius, Margin, Shadow, Stroke, Style, Theme, Vec2, Visuals,
    style::{HandleShape, WidgetVisuals},
};

struct Palette {
    dark: bool,
    /// Fond des panneaux.
    panel: Color32,
    /// Champs de saisie, zones de texte, canevas.
    field: Color32,
    /// Boutons au repos.
    raised: Color32,
    hover: Color32,
    pressed: Color32,
    /// Contours.
    border: Color32,
    border_hover: Color32,
    text: Color32,
    strong: Color32,
    weak: Color32,
    accent: Color32,
    code: Color32,
    window: Color32,
}

const DARK: Palette = Palette {
    dark: true,
    panel: Color32::from_rgb(0x32, 0x32, 0x32),
    field: Color32::from_rgb(0x1f, 0x1f, 0x1f),
    raised: Color32::from_rgb(0x45, 0x45, 0x45),
    hover: Color32::from_rgb(0x52, 0x52, 0x52),
    pressed: Color32::from_rgb(0x2a, 0x2a, 0x2a),
    border: Color32::from_rgb(0x1a, 0x1a, 0x1a),
    border_hover: Color32::from_rgb(0x6e, 0x6e, 0x6e),
    text: Color32::from_rgb(0xd4, 0xd4, 0xd4),
    strong: Color32::from_rgb(0xff, 0xff, 0xff),
    weak: Color32::from_rgb(0x8f, 0x8f, 0x8f),
    accent: Color32::from_rgb(0x2d, 0x8c, 0xeb),
    code: Color32::from_rgb(0x26, 0x26, 0x26),
    window: Color32::from_rgb(0x3a, 0x3a, 0x3a),
};

const LIGHT: Palette = Palette {
    dark: false,
    panel: Color32::from_rgb(0xec, 0xec, 0xec),
    field: Color32::from_rgb(0xff, 0xff, 0xff),
    raised: Color32::from_rgb(0xdc, 0xdc, 0xdc),
    hover: Color32::from_rgb(0xcf, 0xcf, 0xcf),
    pressed: Color32::from_rgb(0xbf, 0xbf, 0xbf),
    border: Color32::from_rgb(0xa8, 0xa8, 0xa8),
    border_hover: Color32::from_rgb(0x70, 0x70, 0x70),
    text: Color32::from_rgb(0x26, 0x26, 0x26),
    strong: Color32::from_rgb(0x00, 0x00, 0x00),
    weak: Color32::from_rgb(0x70, 0x70, 0x70),
    accent: Color32::from_rgb(0x14, 0x73, 0xe6),
    code: Color32::from_rgb(0xf4, 0xf4, 0xf4),
    window: Color32::from_rgb(0xf4, 0xf4, 0xf4),
};

/// Couleur d'accent du thème (poignées, sélection, focus).
pub fn accent(v: &Visuals) -> Color32 {
    if v.dark_mode {
        DARK.accent
    } else {
        LIGHT.accent
    }
}

/// Installe les deux thèmes.
pub fn install(ctx: &egui::Context) {
    ctx.style_mut_of(Theme::Dark, |s| apply(s, &DARK));
    ctx.style_mut_of(Theme::Light, |s| apply(s, &LIGHT));
}

fn widget(bg: Color32, border: Color32, fg: Color32) -> WidgetVisuals {
    WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: bg,
        bg_stroke: Stroke::new(1.0, border),
        corner_radius: CornerRadius::ZERO,
        fg_stroke: Stroke::new(1.0, fg),
        expansion: 0.0,
    }
}

fn apply(style: &mut Style, p: &Palette) {
    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(8.0, 6.0);
    s.button_padding = Vec2::new(12.0, 5.0);
    s.menu_margin = Margin::same(6);
    s.window_margin = Margin::same(14);
    s.interact_size.y = 24.0;
    s.indent = 18.0;
    s.icon_width = 15.0;
    s.icon_width_inner = 9.0;
    s.combo_height = 300.0;

    let v = &mut style.visuals;
    *v = if p.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.widgets.noninteractive = widget(p.panel, p.border, p.text);
    v.widgets.inactive = widget(p.raised, p.border, p.text);
    v.widgets.hovered = widget(p.hover, p.border_hover, p.strong);
    v.widgets.active = widget(p.pressed, p.accent, p.strong);
    v.widgets.open = widget(p.pressed, p.border_hover, p.strong);
    // Séparateurs plus discrets que les contours des boutons.
    v.widgets.noninteractive.bg_stroke = Stroke::new(
        1.0,
        if p.dark {
            Color32::from_rgb(0x22, 0x22, 0x22)
        } else {
            Color32::from_rgb(0xc4, 0xc4, 0xc4)
        },
    );
    v.selection.bg_fill = p.accent.gamma_multiply(if p.dark { 0.7 } else { 0.35 });
    v.selection.stroke = Stroke::new(1.0, p.strong);
    v.hyperlink_color = p.accent;
    v.weak_text_color = Some(p.weak);
    v.faint_bg_color = p.panel;
    v.extreme_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.code_bg_color = p.code;
    v.panel_fill = p.panel;
    v.window_fill = p.window;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    let shadow = Shadow {
        offset: [0, 3],
        blur: 8,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 110 } else { 50 }),
    };
    v.window_shadow = shadow;
    v.popup_shadow = shadow;
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    v.handle_shape = HandleShape::Rect { aspect_ratio: 0.5 };
    v.striped = false;
    v.indent_has_left_vline = true;
    v.collapsing_header_frame = true;
}
