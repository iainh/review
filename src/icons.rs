//! Lucide icons used by the compact document toolbar.

use std::sync::Arc;

use egui::{
    FontData, FontDefinitions, FontFamily, FontId, Response, RichText, WidgetInfo, WidgetType,
};
pub use lucide_icons::Icon;

const FAMILY: &str = "lucide";
const INSTALLED: &str = "lucide_font_installed";

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    if let Some(data) = crate::fonts::system_ui() {
        fonts.font_data.insert("system-ui".into(), data);
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "system-ui".into());
    }
    fonts.font_data.insert(
        FAMILY.into(),
        Arc::new(FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES)),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .push(FAMILY.into());
    ctx.set_fonts(fonts);
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(INSTALLED), true));
}

pub fn ensure_installed(ctx: &egui::Context) {
    if !ctx.data(|data| {
        data.get_temp::<bool>(egui::Id::new(INSTALLED))
            .unwrap_or(false)
    }) {
        install(ctx);
    }
}

fn text(icon: Icon) -> RichText {
    RichText::new(char::from(icon).to_string()).font(FontId::new(18.0, FontFamily::Proportional))
}

pub fn button(ui: &mut egui::Ui, icon: Icon, label: &'static str) -> Response {
    selectable_button(ui, icon, label, false)
}

pub fn selectable_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &'static str,
    selected: bool,
) -> Response {
    let response = crate::native_ui::studio_button(
        ui,
        egui::Button::new(text(icon))
            .selected(selected)
            .min_size(egui::vec2(30.0, 30.0)),
        selected,
    )
    .on_hover_text(label);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_text_uses_system_font_and_icons_remain_available() {
        let reference = egui::Context::default();
        let mut fonts = FontDefinitions::default();
        if let Some(native) = crate::fonts::system_ui() {
            fonts.font_data.insert("native-only".into(), native);
            fonts
                .families
                .insert(FontFamily::Proportional, vec!["native-only".into()]);
        }
        reference.set_fonts(fonts);
        let _ = reference.run_ui(Default::default(), |_| {});

        let ctx = egui::Context::default();
        install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let font = FontId::proportional(13.0);
        for character in "Review Wi1…".chars() {
            let expected = reference.fonts_mut(|fonts| fonts.glyph_width(&font, character));
            let actual = ctx.fonts_mut(|fonts| fonts.glyph_width(&font, character));
            assert_eq!(actual, expected, "system font advance for {character}");
        }
        ctx.fonts_mut(|fonts| {
            let icon_font = FontId::proportional(18.0);
            for icon in [Icon::Search, Icon::ArrowRight, Icon::ZoomIn] {
                assert!(fonts.has_glyph(&icon_font, char::from(icon)));
            }
            assert!(fonts.has_glyph(&FontId::monospace(13.0), 'W'));
        });
    }
}
