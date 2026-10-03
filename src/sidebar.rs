use std::collections::HashMap;

use egui::{Color32, TextureHandle, Vec2};
use mupdf::Outline;

use crate::{
    document::PdfDocument,
    render_worker::{Priority, RenderKey, RenderWorker},
    zoom::Zoom,
};

const PREVIEW_HEIGHT: f32 = 210.0;

pub struct Sidebar {
    pub open: bool,
    pages: bool,
    outline: Result<Vec<Outline>, String>,
    thumbnails: HashMap<usize, Result<TextureHandle, String>>,
    thumbnail_dpi: f32,
    visible_page: Option<usize>,
}

impl Sidebar {
    pub fn new(document: &PdfDocument) -> Self {
        Self {
            open: true,
            pages: false,
            outline: document
                .outlines()
                .map_err(|error| format!("Could not load outline: {error:#}")),
            thumbnails: HashMap::new(),
            thumbnail_dpi: 0.0,
            visible_page: None,
        }
    }

    pub fn ui(
        &mut self,
        root: &mut egui::Ui,
        document: &PdfDocument,
        worker: &mut RenderWorker,
    ) -> Option<usize> {
        if !self.open {
            return None;
        }
        let mut destination = None;
        egui::Panel::left("sidebar")
            .default_size(240.0)
            .size_range(200.0..=400.0)
            .resizable(true)
            .show_inside(root, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.pages, false, "Outline");
                    ui.selectable_value(&mut self.pages, true, "Pages");
                });
                ui.separator();
                if self.pages {
                    destination = self.page_previews(ui, document, worker);
                } else {
                    match &self.outline {
                        Ok(outline) if outline.is_empty() => {
                            ui.label("This document has no outline.");
                        }
                        Ok(outline) => {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                outline_rows(
                                    ui,
                                    outline,
                                    document.current_page(),
                                    document.page_count(),
                                    &mut destination,
                                );
                            });
                        }
                        Err(error) => {
                            ui.colored_label(Color32::LIGHT_RED, error);
                        }
                    }
                }
            });
        destination
    }

    fn page_previews(
        &mut self,
        ui: &mut egui::Ui,
        document: &PdfDocument,
        worker: &mut RenderWorker,
    ) -> Option<usize> {
        let dpi = ui.ctx().pixels_per_point();
        if self.thumbnail_dpi != dpi {
            self.thumbnails.clear();
            self.thumbnail_dpi = dpi;
        }
        let row_height = PREVIEW_HEIGHT + ui.spacing().item_spacing.y;
        let current = document.current_page();
        let mut scroll = egui::ScrollArea::vertical().id_salt("thumbnails");
        // Follow page changes from search, outline, or go-to-page. Subsequent
        // manual scrolling remains free until the current page changes again.
        if self.visible_page != Some(current) {
            scroll = scroll.vertical_scroll_offset(current as f32 * row_height);
            self.visible_page = Some(current);
        }
        let mut destination = None;
        scroll.show_rows(ui, PREVIEW_HEIGHT, document.page_count(), |ui, visible| {
            // Keep only visible textures: memory and rasterization do not grow
            // with the document's page count.
            self.thumbnails.retain(|page, _| visible.contains(page));
            for page in visible {
                if let std::collections::hash_map::Entry::Vacant(e) = self.thumbnails.entry(page) {
                    let viewport = ((244.0 * dpi) as u32, (244.0 * dpi) as u32);
                    match document.page_size(page) {
                        Ok(size) => {
                            let scale = Zoom::FitPage.scale(viewport, size, dpi);
                            if let Some(image) =
                                worker.image(RenderKey::new(page, scale), Priority::Thumbnail)
                            {
                                let texture = image
                                    .map(|image| {
                                        ui.ctx().load_texture(
                                            format!("preview {page}"),
                                            egui::ColorImage::from_rgba_unmultiplied(
                                                [image.width as usize, image.height as usize],
                                                &image.rgba,
                                            ),
                                            egui::TextureOptions::LINEAR,
                                        )
                                    })
                                    .map_err(|error| format!("Preview unavailable: {error}"));
                                e.insert(texture);
                            }
                        }
                        Err(error) => {
                            e.insert(Err(format!("Preview unavailable: {error:#}")));
                        }
                    }
                }
                let (rect, response) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), PREVIEW_HEIGHT),
                    egui::Sense::click(),
                );
                let fill = if current == page {
                    ui.visuals().selection.bg_fill
                } else if response.hovered() {
                    ui.visuals().widgets.hovered.bg_fill
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, 4.0, fill);
                match self.thumbnails.get(&page) {
                    Some(Ok(texture)) => {
                        let original = texture.size_vec2();
                        let scale = ((rect.width() - 20.0) / original.x).min(180.0 / original.y);
                        let image = egui::Rect::from_center_size(
                            rect.center() - Vec2::new(0.0, 10.0),
                            original * scale,
                        );
                        ui.painter().image(
                            texture.id(),
                            image,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    Some(Err(error)) => {
                        response.clone().on_hover_text(error.as_str());
                    }
                    None => {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "Loading…",
                            egui::FontId::proportional(13.0),
                            ui.visuals().weak_text_color(),
                        );
                    }
                }
                ui.painter().text(
                    egui::pos2(rect.center().x, rect.bottom() - 12.0),
                    egui::Align2::CENTER_CENTER,
                    format!("Page {}", page + 1),
                    egui::FontId::proportional(13.0),
                    ui.visuals().text_color(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::Button,
                        true,
                        current == page,
                        format!("Page {}", page + 1),
                    )
                });
                if response.clicked() {
                    destination = Some(page);
                }
            }
        });
        destination
    }
}

fn outline_rows(
    ui: &mut egui::Ui,
    entries: &[Outline],
    current: usize,
    count: usize,
    destination: &mut Option<usize>,
) {
    for (index, entry) in entries.iter().enumerate() {
        ui.push_id(index, |ui| {
            let page = entry
                .dest
                .map(|dest| dest.loc.page_number as usize)
                .filter(|&page| page < count);
            let mut label = |ui: &mut egui::Ui| {
                let text = match page {
                    Some(page) => format!("{}  ·  {}", entry.title, page + 1),
                    None => entry.title.clone(),
                };
                if ui
                    .add_enabled(
                        page.is_some(),
                        egui::Button::selectable(page == Some(current), text).wrap(),
                    )
                    .clicked()
                {
                    *destination = page;
                }
            };
            if entry.down.is_empty() {
                label(ui);
            } else {
                let id = ui.make_persistent_id("outline_entry");
                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    true,
                )
                .show_header(ui, label)
                .body(|ui| {
                    outline_rows(ui, &entry.down, current, count, destination);
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::Sidebar;
    use crate::document::PdfDocument;

    #[test]
    #[ignore = "requires REVIEW_TEST_PDF pointing to the OpenID Connect handbook"]
    fn handbook_previews_are_lazy_and_follow_navigation() {
        let mut document = PdfDocument::open(std::env::var("REVIEW_TEST_PDF").unwrap()).unwrap();
        let mut worker = crate::render_worker::RenderWorker::new(document.worker_source());
        let mut sidebar = Sidebar::new(&document);
        sidebar.pages = true;
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 900.0),
            )),
            ..Default::default()
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !sidebar.thumbnails.contains_key(&0) {
            worker.begin_frame();
            let _ = ctx.run_ui(input.clone(), |ui| {
                sidebar.ui(ui, &document, &mut worker);
            });
            worker.end_frame(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "preview worker did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(sidebar.thumbnails.contains_key(&0));
        assert!(sidebar.thumbnails.len() <= 6);
        assert!(sidebar.thumbnails.values().all(|image| image.is_ok()));
        document.go_to_page(44);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !sidebar.thumbnails.contains_key(&44) {
            worker.begin_frame();
            let _ = ctx.run_ui(input.clone(), |ui| {
                sidebar.ui(ui, &document, &mut worker);
            });
            worker.end_frame(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "preview worker did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(sidebar.thumbnails.contains_key(&44));
        assert!(!sidebar.thumbnails.contains_key(&0));
        assert!(sidebar.thumbnails.len() <= 6);
        assert!(sidebar.thumbnails.values().all(|image| image.is_ok()));
    }
}
