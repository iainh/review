use std::collections::HashMap;

use egui::{Color32, TextureHandle, Vec2};
use mupdf::Outline;

use crate::{
    document::PdfDocument,
    render_worker::{Priority, RenderKey, RenderWorker},
    zoom::Zoom,
};

const PREVIEW_HEIGHT: f32 = 210.0;

pub enum SidebarTarget {
    Page(usize),
    Destination(mupdf::link::LinkDestination),
}

pub struct Sidebar {
    pub open: bool,
    pages: bool,
    width: f32,
    outline: Result<Vec<Outline>, String>,
    thumbnails: HashMap<usize, Result<TextureHandle, String>>,
    thumbnail_dpi: f32,
    visible_page: Option<usize>,
}

impl Sidebar {
    pub fn clear_previews(&mut self) {
        self.thumbnails.clear();
    }

    pub fn new(document: &PdfDocument) -> Self {
        Self {
            open: true,
            pages: false,
            width: 240.0,
            outline: document
                .outlines()
                .map_err(|error| format!("Could not load outline: {error:#}")),
            thumbnails: HashMap::new(),
            thumbnail_dpi: 0.0,
            visible_page: None,
        }
    }

    pub fn state(&self) -> crate::persistence::SidebarState {
        crate::persistence::SidebarState {
            open: self.open,
            width: self.width,
            pages: self.pages,
        }
    }

    pub fn restore(&mut self, state: &crate::persistence::SidebarState) {
        self.open = state.open;
        self.width = state.width;
        self.pages = state.pages;
    }

    pub fn ui(
        &mut self,
        root: &mut egui::Ui,
        document: &PdfDocument,
        worker: &mut RenderWorker,
    ) -> Option<SidebarTarget> {
        if !self.open {
            return None;
        }
        let mut destination = None;
        let panel = egui::Panel::left("sidebar")
            .default_size(self.width)
            .size_range(200.0..=400.0)
            .resizable(true)
            .frame(egui::Frame::side_top_panel(root.style()).fill(root.visuals().faint_bg_color))
            .show_inside(root, |ui| {
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        crate::native_ui::panel_bevel(ui);
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut self.pages, false, "Outline");
                            ui.selectable_value(&mut self.pages, true, "Pages");
                        });
                    });
                ui.separator();
                if self.pages {
                    destination = self
                        .page_previews(ui, document, worker)
                        .map(SidebarTarget::Page);
                } else {
                    match &self.outline {
                        Ok(outline) if outline.is_empty() => {
                            ui.label("This document has no outline.");
                        }
                        Ok(outline) => {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                outline_rows(ui, outline, document, &mut destination);
                            });
                        }
                        Err(error) => {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                    }
                }
            });
        self.width = panel.response.rect.width();
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
                if response.has_focus() {
                    ui.painter().rect_stroke(
                        rect.shrink(1.0),
                        4.0,
                        ui.visuals().selection.stroke,
                        egui::StrokeKind::Inside,
                    );
                }
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
                let physical = (page + 1).to_string();
                let caption = document
                    .page_label(page)
                    .ok()
                    .filter(|label| !label.is_empty() && *label != physical)
                    .map_or_else(
                        || format!("Page {physical}"),
                        |label| format!("Page {physical} · {label}"),
                    );
                // Elide long labels after the physical number, never across it.
                let mut job = egui::text::LayoutJob::simple_singleline(
                    caption,
                    egui::FontId::proportional(13.0),
                    ui.visuals().text_color(),
                );
                job.wrap.max_width = rect.width() - 20.0;
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                let galley = ui.painter().layout_job(job);
                let position = egui::pos2(
                    rect.center().x - galley.size().x / 2.0,
                    rect.bottom() - 20.0,
                );
                ui.painter()
                    .galley(position, galley, ui.visuals().text_color());
                response
                    .clone()
                    .on_hover_text(document.page_description(page));
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::Button,
                        true,
                        current == page,
                        document.page_description(page),
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
    document: &PdfDocument,
    destination: &mut Option<SidebarTarget>,
) {
    for (index, entry) in entries.iter().enumerate() {
        ui.push_id(index, |ui| {
            let page = entry
                .dest
                .map(|dest| dest.loc.page_number as usize)
                .filter(|&page| page < document.page_count());
            let mut label = |ui: &mut egui::Ui| {
                let text = match page {
                    Some(page) => {
                        format!("{}  ·  {}", entry.title, document.page_description(page))
                    }
                    None => entry.title.clone(),
                };
                if ui
                    .add_enabled(
                        page.is_some(),
                        egui::Button::selectable(page == Some(document.current_page()), text)
                            .wrap(),
                    )
                    .clicked()
                {
                    *destination = entry.dest.map(SidebarTarget::Destination);
                }
            };
            if entry.down.is_empty() {
                label(ui);
            } else {
                let id = ui.make_persistent_id("outline_entry");
                let state = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    true,
                );
                let (toggle, _, _) = state.show_header(ui, label).body(|ui| {
                    outline_rows(ui, &entry.down, document, destination);
                });
                let open = egui::collapsing_header::CollapsingState::load(ui.ctx(), id)
                    .unwrap()
                    .is_open();
                toggle.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::CollapsingHeader,
                        ui.is_enabled(),
                        format!(
                            "{} {}",
                            if open { "Collapse" } else { "Expand" },
                            entry.title
                        ),
                    )
                });
                ui.ctx()
                    .accesskit_node_builder(toggle.id, |node| node.set_expanded(open));
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
