//! The look of the window: Craft Anything's near-black surfaces, soft rounded
//! cards, Figtree text and mono numbers. Colors were read from its window.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Visuals,
};
use std::process::Command;
use std::sync::Arc;

pub const BUTTON_RADIUS: u8 = 8;
pub const CARD_RADIUS: u8 = 12;
const SEMIBOLD: &str = "semibold";

pub const fn rgb(value: u32) -> Color32 {
    Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

pub const PANEL: Color32 = rgb(0x111216);
pub const CARD: Color32 = rgb(0x15161a);
pub const WELL: Color32 = rgb(0x0b0c0f);
pub const RAISED: Color32 = rgb(0x1c1e21);
pub const SELECTION: Color32 = rgb(0x2a2b2f);
pub const LINE: Color32 = rgb(0x2b2c30);
pub const FG: Color32 = rgb(0xf2f1ee);
pub const DIM: Color32 = rgb(0xb4b4b3);
pub const FAINT: Color32 = rgb(0x8d8e8e);
pub const ACCENT: Color32 = rgb(0x75abff);
pub const ACCENT_SOFT: Color32 = rgb(0x18273f);
pub const ACCENT_STRONG: Color32 = rgb(0x1f5fbf);
pub const RED: Color32 = rgb(0xf5544f);
pub const GREEN: Color32 = rgb(0x4ddb7a);
pub const YELLOW: Color32 = rgb(0xe6c07b);

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

pub fn visuals() -> Visuals {
    let control = CornerRadius::same(BUTTON_RADIUS);
    let mut v = Visuals::dark();
    v.override_text_color = Some(FG);
    v.panel_fill = PANEL;
    v.window_fill = RAISED;
    v.extreme_bg_color = WELL;
    v.faint_bg_color = CARD;
    v.window_corner_radius = CornerRadius::same(CARD_RADIUS);
    v.menu_corner_radius = CornerRadius::same(CARD_RADIUS);
    v.window_stroke = Stroke::new(1.0, LINE);
    v.selection.bg_fill = ACCENT_STRONG;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, FG);
    v.widgets.noninteractive.corner_radius = control;
    for (style, fill, stroke) in [
        (&mut v.widgets.inactive, RAISED, LINE),
        (&mut v.widgets.hovered, SELECTION, LINE),
        (&mut v.widgets.active, SELECTION, FAINT),
        (&mut v.widgets.open, SELECTION, LINE),
    ] {
        style.bg_fill = fill;
        style.weak_bg_fill = fill;
        style.bg_stroke = Stroke::new(1.0, stroke);
        style.fg_stroke = Stroke::new(1.0, FG);
        style.corner_radius = control;
        style.expansion = 0.0;
    }
    v
}

/// A monospaced font from this computer, for the numbers.
fn monospace_file() -> Option<Vec<u8>> {
    for name in ["JetBrains Mono", "monospace"] {
        let out = Command::new("fc-match").args(["-f", "%{file}", name]).output().ok()?;
        let path = String::from_utf8(out.stdout).ok()?;
        if (path.ends_with(".ttf") || path.ends_with(".otf"))
            && let Ok(bytes) = std::fs::read(path)
        {
            return Some(bytes);
        }
    }
    None
}

/// Figtree is built in, so the text looks right on any computer. The numbers
/// use JetBrains Mono or the computer's own mono font. On a computer with
/// neither they fall back to Figtree, which keeps the program small.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::empty();
    fonts.font_data.insert(
        "figtree".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/figtree-400.ttf"))),
    );
    fonts.font_data.insert(
        "figtree-semibold".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/figtree-600.ttf"))),
    );
    let mut mono = vec!["figtree".to_string()];
    if let Some(bytes) = monospace_file() {
        fonts.font_data.insert("mono".into(), Arc::new(FontData::from_owned(bytes)));
        mono.insert(0, "mono".to_string());
    }
    fonts.families.insert(FontFamily::Proportional, vec!["figtree".to_string()]);
    fonts.families.insert(FontFamily::Monospace, mono);
    fonts.families.insert(
        FontFamily::Name(SEMIBOLD.into()),
        vec!["figtree-semibold".to_string(), "figtree".to_string()],
    );
    ctx.set_fonts(fonts);
}
