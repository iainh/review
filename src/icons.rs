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
