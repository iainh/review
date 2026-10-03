use egui::{Color32, Context, Key, Modifiers, TextureHandle, Vec2};

use crate::{document::PdfDocument, search::Search};

pub struct Viewer {
    document: PdfDocument,
    zoom: f32,
    page_input: String,
    error: Option<String>,
    page_texture: Option<TextureHandle>,
    rendered: Option<(usize, (u32, u32), f32)>,
    search: Search,
    reveal_match: bool,
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
            search: Search::default(),
            reveal_match: false,
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

    fn advance_match(&mut self, backwards: bool) {
        if let Some(page) = self.search.advance(self.document.current_page(), backwards) {
            self.go_to_page(page);
            self.reveal_match = true;
        }
    }

    pub fn ui(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        let focus_page = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::G));
        let mut focus_search = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::F));
        if focus_search {
            self.search.open = true;
        }
        if self.search.open
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.search.open = false;
            self.search.clear_results();
            ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new("search_query")));
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::SHIFT, Key::F3)) {
            self.advance_match(true);
        } else if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::F3)) {
            self.advance_match(false);
        }
        let enter_backwards = ctx.input(|input| input.events.iter().any(|event| matches!(
            event, egui::Event::Key { key: Key::Enter, pressed: true, modifiers, .. } if modifiers.shift
        )));
        match self.search.step(&self.document) {
            Ok(Some(page)) => {
                self.go_to_page(page);
                self.reveal_match = true;
            }
            Err(error) => self.error = Some(format!("Search failed: {error:#}")),
            _ => {}
        }
        if self.search.next_page.is_some() {
            ctx.request_repaint();
        }
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
                if (field.has_focus() || field.lost_focus())
                    && ui.input(|input| input.key_pressed(Key::Escape))
                {
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
                ui.separator();
                if ui
                    .button("Search")
                    .on_hover_text("Ctrl+F / Cmd+F")
                    .clicked()
                {
                    self.search.open = true;
                    focus_search = true;
                }
            });
            if let Some(error) = &self.error {
                ui.colored_label(Color32::LIGHT_RED, error);
            }
        });

        if self.search.open {
            egui::Panel::top("search_bar").show_inside(root, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Find");
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.search.query)
                            .id(egui::Id::new("search_query"))
                            .desired_width(220.0)
                            .hint_text("Search this document"),
                    );
                    if field.changed() {
                        self.search.clear_results();
                    }
                    if focus_search {
                        select_text(&ctx, &field, self.search.query.chars().count());
                    }
                    let enter =
                        field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                    if ui.button("Find").clicked() || (enter && self.search.submitted.is_empty()) {
                        self.search.start(&self.document);
                        ctx.request_repaint();
                    } else if enter {
                        self.advance_match(enter_backwards);
                    }
                    if enter {
                        field.request_focus();
                    }
                    let ready = self.search.next_page.is_none() && !self.search.matches.is_empty();
                    if ui
                        .add_enabled(ready, egui::Button::new("Previous match"))
                        .clicked()
                    {
                        self.advance_match(true);
                    }
                    if ui
                        .add_enabled(ready, egui::Button::new("Next match"))
                        .clicked()
                    {
                        self.advance_match(false);
                    }
                    if let Some(page) = self.search.next_page {
                        ui.label(format!(
                            "Searching… {}/{}",
                            page,
                            self.document.page_count()
                        ));
                    } else if !self.search.submitted.is_empty() {
                        if self.search.matches.is_empty() {
                            ui.label("No matches");
                        } else {
                            ui.label(format!(
                                "{} / {} matches",
                                self.search.selected.unwrap_or(0) + 1,
                                self.search.matches.len()
                            ));
                        }
                    }
                    if ui.button("Close").clicked() {
                        self.search.open = false;
                        self.search.clear_results();
                    }
                });
            });
        }

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
                .id_salt(("page", self.document.current_page()))
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
                        for (index, hit) in self
                            .search
                            .matches
                            .iter()
                            .enumerate()
                            .filter(|(_, hit)| hit.page == self.document.current_page())
                        {
                            let selected = self.search.selected == Some(index);
                            let mut bounds = egui::Rect::NOTHING;
                            for quad in &hit.quads {
                                let points: Vec<_> = quad
                                    .iter()
                                    .map(|point| {
                                        page.min + Vec2::new(point[0] * size.x, point[1] * size.y)
                                    })
                                    .collect();
                                for point in &points {
                                    bounds.extend_with(*point);
                                }
                                let colour = if selected {
                                    Color32::from_rgba_unmultiplied(255, 145, 0, 110)
                                } else {
                                    Color32::from_rgba_unmultiplied(255, 225, 0, 75)
                                };
                                ui.painter().add(egui::Shape::convex_polygon(
                                    points,
                                    colour,
                                    egui::Stroke::NONE,
                                ));
                            }
                            if selected && self.reveal_match {
                                ui.scroll_to_rect(bounds.expand(24.0), None);
                                self.reveal_match = false;
                            }
                        }
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
    use super::{Viewer, parse_page};

    #[test]
    fn shift_f3_uses_event_modifiers_after_shift_has_been_released() {
        let mut viewer = Viewer::new(crate::document::tests::sample_document());
        viewer.search.query = "alpha".into();
        viewer.search.start(&viewer.document);
        for _ in 0..2 {
            viewer.search.step(&viewer.document).unwrap();
        }
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(960.0, 720.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::F3,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            }],
            modifiers: egui::Modifiers::NONE,
            ..Default::default()
        };
        let _ = egui::Context::default().run_ui(input, |ui| viewer.ui(ui));
        assert_eq!(viewer.search.selected, Some(2));
        assert_eq!(viewer.document.current_page(), 1);
    }

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
