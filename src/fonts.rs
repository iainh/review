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
    use font_kit::{file_type::FileType, font::Font};

    // Keep Core Text's resolved file. Looking up its private PostScript name
    // again can silently substitute Times New Roman for San Francisco.
    let font = new_ui_font_for_language(kCTFontSystemFontType, 13.0, None);
    let path = font.url()?.to_path()?;
    let font_index = match Font::analyze_path(&path).ok()? {
        FileType::Single => 0,
        FileType::Collection(count) => (0..count).find(|&index| {
            Font::from_path(&path, index)
                .ok()
                .and_then(|face| face.postscript_name())
                .is_some_and(|name| name == font.postscript_name())
        })?,
    };
    Some(Handle::Path { path, font_index })
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn loaded_ui_font_matches_core_text_system_face() {
        use core_text::font::{kCTFontSystemFontType, new_ui_font_for_language};

        let expected = new_ui_font_for_language(kCTFontSystemFontType, 13.0, None);
        let data = super::system_ui().expect("system UI font should be available");
        let loaded =
            font_kit::font::Font::from_bytes(std::sync::Arc::new(data.font.to_vec()), data.index)
                .unwrap();
        assert_eq!(
            loaded.postscript_name().unwrap(),
            expected.postscript_name()
        );
    }
}
