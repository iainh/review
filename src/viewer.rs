use egui::{Color32, Context, Key, Modifiers, TextureHandle, Vec2};

use crate::document::PdfDocument;

pub struct Viewer {
    document: PdfDocument,
    zoom: f32,
    page_input: String,
    error: Option<String>,
    page_texture: Option<TextureHandle>,
    rendered: Option<(usize, (u32, u32), f32)>,
    pub quit: bool,
}

impl Viewer {
    pub fn new(document: PdfDocument) -> Self {
        Self {
            document,
            zoom: 1.0,
            page_input: "1".into(),
            error: None,
            page_texture: None,
            rendered: None,
            quit: false,
        }
    }

    pub fn title(&self) -> String {
        format!(
            "Review — {} — {}/{} — {:.0}%",
            self.document.name(),
            self.document.current_page() + 1,
            self.document.page_count(),
            self.zoom * 100.0
        )
    }

    fn go_to_page(&mut self, page: usize) {
        self.document.go_to_page(page);
        self.page_input = (self.document.current_page() + 1).to_string();
        self.error = None;
    }

    fn change_page(&mut self, delta: i32) {
        self.document.change_page(delta);
        self.go_to_page(self.document.current_page());
    }

    fn submit_page(&mut self) {
        match parse_page(&self.page_input, self.document.page_count()) {
            Some(page) => self.go_to_page(page),
            None => {
                self.error = Some(format!(
                    "Enter a page from 1 to {}",
                    self.document.page_count()
                ))
            }
        }
    }

    pub fn ui(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        let focus_page = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::G));
        if !ctx.egui_wants_keyboard_input() {
            ctx.input(|input| {
                if input.key_pressed(Key::ArrowLeft) || input.key_pressed(Key::PageUp) {
                    self.change_page(-1);
                }
                if input.key_pressed(Key::ArrowRight) || input.key_pressed(Key::PageDown) {
                    self.change_page(1);
                }
                if input.key_pressed(Key::Plus) || input.key_pressed(Key::Equals) {
                    self.zoom = (self.zoom * 1.25).min(4.0);
                }
                if input.key_pressed(Key::Minus) {
                    self.zoom = (self.zoom * 0.8).max(0.25);
                }
                if input.key_pressed(Key::Num0) {
                    self.zoom = 1.0;
                }
                self.quit = input.key_pressed(Key::Q) || input.key_pressed(Key::Escape);
            });
        }

        egui::Panel::top("toolbar").show_inside(root, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.document.current_page() > 0,
                        egui::Button::new("Previous"),
                    )
                    .clicked()
                {
                    self.change_page(-1);
                }
                if ui
                    .add_enabled(
                        self.document.current_page() + 1 < self.document.page_count(),
                        egui::Button::new("Next"),
                    )
                    .clicked()
                {
                    self.change_page(1);
                }
                ui.separator();
                ui.label("Page");
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.page_input)
                        .id_source("page_input")
                        .desired_width(48.0),
                );
                if focus_page {
                    select_text(&ctx, &field, self.page_input.chars().count());
                }
                ui.label(format!("/ {}", self.document.page_count()));
                if (field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)))
                    || ui.button("Go").clicked()
                {
                    self.submit_page();
                }
                if field.has_focus() && ui.input(|input| input.key_pressed(Key::Escape)) {
                    field.surrender_focus();
                    self.go_to_page(self.document.current_page());
                }
                ui.separator();
                if ui.button("−").clicked() {
                    self.zoom = (self.zoom * 0.8).max(0.25);
                }
                ui.label(format!("{:.0}%", self.zoom * 100.0));
                if ui.button("+").clicked() {
                    self.zoom = (self.zoom * 1.25).min(4.0);
                }
                if ui.button("Fit page").clicked() {
                    self.zoom = 1.0;
                }
            });
            if let Some(error) = &self.error {
                ui.colored_label(Color32::LIGHT_RED, error);
            }
        });

        egui::CentralPanel::default().show_inside(root, |ui| {
            let available = ui.available_size();
            let dpi = ctx.pixels_per_point();
            let viewport = ((available.x * dpi) as u32, (available.y * dpi) as u32);
            let key = (self.document.current_page(), viewport, self.zoom);
            if self.rendered != Some(key) {
                match self.document.render_current(viewport, self.zoom) {
                    Ok(image) => {
                        self.page_texture = Some(ctx.load_texture(
                            "PDF page",
                            egui::ColorImage::from_rgba_unmultiplied(
                                [image.width as usize, image.height as usize],
                                &image.rgba,
                            ),
                            egui::TextureOptions::LINEAR,
                        ));
                        self.rendered = Some(key);
                    }
                    Err(error) => {
                        self.page_texture = None;
                        self.error = Some(format!("Failed to render page: {error:#}"));
                    }
                }
            }
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if let Some(texture) = &self.page_texture {
                        let size = texture.size_vec2() / dpi;
                        let canvas = available.max(size + Vec2::splat(32.0));
                        let (rect, _) = ui.allocate_exact_size(canvas, egui::Sense::hover());
                        let page = egui::Rect::from_center_size(rect.center(), size);
                        ui.painter().image(
                            texture.id(),
                            page,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                });
        });
    }
}

fn parse_page(input: &str, count: usize) -> Option<usize> {
    input
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|page| (1..=count).contains(page))
        .map(|page| page - 1)
}

fn select_text(ctx: &Context, response: &egui::Response, len: usize) {
    response.request_focus();
    if let Some(mut state) = egui::TextEdit::load_state(ctx, response.id) {
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(len),
            )));
        state.store(ctx, response.id);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_page;

    #[test]
    fn page_numbers_are_one_based_and_checked() {
        assert_eq!(parse_page("1", 45), Some(0));
        assert_eq!(parse_page(" 17 ", 45), Some(16));
        assert_eq!(parse_page("45", 45), Some(44));
        for input in [
            "0",
            "46",
            "-1",
            "1.5",
            "abc",
            "",
            "999999999999999999999999",
        ] {
            assert_eq!(parse_page(input, 45), None, "{input}");
        }
    }
}
