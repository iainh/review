//! System UI font selection; egui's bundled fonts remain available as fallbacks.

use std::sync::{Arc, OnceLock};

use egui::FontData;
use font_kit::handle::Handle;

pub fn system_ui() -> Option<Arc<FontData>> {
    static FONT: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    FONT.get_or_init(|| {
        let (bytes, index) = match system_handle()? {
            Handle::Path { path, font_index } => (std::fs::read(path).ok()?, font_index),
            Handle::Memory { bytes, font_index } => (bytes.as_ref().clone(), font_index),
        };
        // Preserve the face index for fonts stored in a collection.
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        Some(Arc::new(data))
    })
    .clone()
}

#[cfg(not(target_os = "macos"))]
fn system_handle() -> Option<Handle> {
    use font_kit::{family_name::FamilyName, properties::Properties, source::SystemSource};

    #[cfg(target_os = "windows")]
    let family = FamilyName::Title("Segoe UI".into());
    #[cfg(not(target_os = "windows"))]
    let family = FamilyName::SansSerif;

    SystemSource::new()
        .select_best_match(&[family], &Properties::new())
        .ok()
}

#[cfg(target_os = "macos")]
fn system_handle() -> Option<Handle> {
    use core_text::font::{kCTFontSystemFontType, new_ui_font_for_language};
    use font_kit::source::SystemSource;

    // Query Core Text rather than relying on private San Francisco family names.
    let font = new_ui_font_for_language(kCTFontSystemFontType, 13.0, None);
    SystemSource::new()
        .select_by_postscript_name(&font.postscript_name())
        .ok()
}
