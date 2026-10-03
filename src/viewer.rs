use egui::{Color32, Context, Key, Modifiers, TextureHandle, Vec2};

use crate::{
    document::PdfDocument,
    printing::PrintDialog,
    render_worker::{Priority, RenderKey, RenderWorker},
    search::Search,
    sidebar::Sidebar,
    zoom::{POINT_SCALE, Zoom},
};

pub struct Viewer {
    document: PdfDocument,
    zoom: Zoom,
    effective_zoom: f32,
    zoom_input: String,
    page_input: String,
    error: Option<String>,
    page_texture: Option<TextureHandle>,
    rendered: Option<RenderKey>,
    render_worker: RenderWorker,
    search: Search,
    reveal_match: bool,
    sidebar: Sidebar,
    printing: PrintDialog,
    pub quit: bool,
}

impl Viewer {
    pub fn new(document: PdfDocument) -> Self {
        let sidebar = Sidebar::new(&document);
        let render_worker = RenderWorker::new(document.worker_source());
        Self {
            document,
            zoom: Zoom::FitPage,
            effective_zoom: 1.0,
            zoom_input: "100".into(),
            page_input: "1".into(),
            error: None,
            page_texture: None,
            rendered: None,
            render_worker,
            search: Search::default(),
            reveal_match: false,
            sidebar,
            printing: PrintDialog::default(),
            quit: false,
        }
    }

    pub fn title(&self) -> String {
        format!(
            "Review — {} — {}/{} — {}",
            self.document.name(),
            self.document.current_page() + 1,
            self.document.page_count(),
            self.zoom.label()
        )
    }

    pub fn path(&self) -> &std::path::Path {
        self.document.path()
    }

    pub fn print_if_requested(&mut self, window: &winit::window::Window) {
        self.printing.run_requested(&self.document, window);
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

    fn change_zoom(&mut self, factor: f32) {
        self.zoom.change(factor, self.effective_zoom);
        self.error = None;
        if let Zoom::Percent(value) = self.zoom {
            self.effective_zoom = value;
        }
    }

    pub fn ui(&mut self, root: &mut egui::Ui, open_requested: &mut bool) {
        self.render_worker.begin_frame();
        let ctx = root.ctx().clone();
        if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::P)) {
            if self.document.permissions().print {
                self.printing.open = true;
            } else {
                self.error = Some("This PDF does not allow printing".into());
            }
        }
        // Register the modal backdrop before the viewer and keep its keyboard
        // events out of global page, search, zoom and sidebar shortcuts.
        let print_active = self.printing.open;
        if print_active {
            self.printing.ui(&ctx, &self.document);
        }
        let print_input =
            print_active.then(|| ctx.input_mut(|input| std::mem::take(&mut input.events)));
        let focus_page = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::G));
        let focus_zoom = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::L));
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::F9)) {
            self.sidebar.open = !self.sidebar.open;
        }
        let mut focus_search = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::F));
        if focus_search {
            self.search.open = true;
        }
        if !self.printing.open
            && self.search.open
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

        egui::Panel::top("toolbar").show_inside(root, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .selectable_label(self.sidebar.open, "Sidebar")
                    .on_hover_text("F9")
                    .clicked()
                {
                    self.sidebar.open = !self.sidebar.open;
                }
                ui.separator();
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
                    && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
                {
                    field.surrender_focus();
                    self.go_to_page(self.document.current_page());
                }
                ui.separator();
                if ui.button("−").clicked() {
                    self.change_zoom(0.8);
                }
                let field = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.zoom_input)
                            .id(egui::Id::new("zoom_input"))
                            .desired_width(44.0),
                    )
                    .on_hover_text("Zoom percentage (Ctrl+L / Cmd+L), 10–1600%");
                if focus_zoom {
                    select_text(&ctx, &field, self.zoom_input.chars().count());
                }
                ui.label("%");
                if field.lost_focus()
                    && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter))
                {
                    match Zoom::parse(&self.zoom_input) {
                        Some(zoom) => {
                            self.zoom = zoom;
                            self.error = None;
                        }
                        None => self.error = Some("Enter a zoom from 10 to 1600%".into()),
                    }
                }
                if (field.has_focus() || field.lost_focus())
                    && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
                {
                    field.surrender_focus();
                }
                if !focus_zoom && !field.has_focus() && !field.lost_focus() {
                    let text = format!("{:.0}", self.effective_zoom * 100.0);
                    if self.zoom_input != text {
                        self.zoom_input = text;
                        ctx.request_repaint();
                    }
                }
                if ui.button("+").clicked() {
                    self.change_zoom(1.25);
                }
                if ui
                    .selectable_label(self.zoom == Zoom::FitPage, "Fit page")
                    .clicked()
                {
                    self.zoom = Zoom::FitPage;
                }
                if ui
                    .selectable_label(self.zoom == Zoom::FitWidth, "Fit width")
                    .clicked()
                {
                    self.zoom = Zoom::FitWidth;
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
                ui.separator();
                if ui.button("Open…").on_hover_text("Ctrl+O / Cmd+O").clicked() {
                    *open_requested = true;
                }
                if ui
                    .add_enabled(
                        self.document.permissions().print,
                        egui::Button::new("Print…"),
                    )
                    .on_hover_text("Ctrl+P / Cmd+P")
                    .on_disabled_hover_text("This PDF does not allow printing")
                    .clicked()
                {
                    self.printing.open = true;
                }
            });
            let permissions = self.document.permissions();
            if !permissions.print || !permissions.print_high_quality || !permissions.copy {
                let printing = if !permissions.print {
                    "not allowed"
                } else if !permissions.print_high_quality {
                    "low quality only"
                } else {
                    "allowed"
                };
                ui.label(format!(
                    "PDF permissions: printing {printing}; copying {}. Viewing and search remain available.",
                    if permissions.copy { "allowed" } else { "not allowed" }
                ));
            }
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

        // Let text fields consume Escape and settle focus before handling
        // document shortcuts. egui clears focus at the start of an Escape frame.
        if !self.printing.open && !ctx.egui_wants_keyboard_input() {
            let previous = (self.document.current_page(), self.zoom);
            ctx.input(|input| {
                if input.key_pressed(Key::ArrowLeft) || input.key_pressed(Key::PageUp) {
                    self.change_page(-1);
                }
                if input.key_pressed(Key::ArrowRight) || input.key_pressed(Key::PageDown) {
                    self.change_page(1);
                }
                if input.key_pressed(Key::Num1) {
                    self.zoom = Zoom::Percent(1.0);
                    self.effective_zoom = 1.0;
                }
                if input.key_pressed(Key::Plus) || input.key_pressed(Key::Equals) {
                    self.change_zoom(1.25);
                }
                if input.key_pressed(Key::Minus) {
                    self.change_zoom(0.8);
                }
                if input.key_pressed(Key::Num0) {
                    self.zoom = Zoom::FitPage;
                }
                if input.key_pressed(Key::Num2) {
                    self.zoom = Zoom::FitWidth;
                }
                self.quit = input.key_pressed(Key::Q) || input.key_pressed(Key::Escape);
            });
            if previous != (self.document.current_page(), self.zoom) {
                self.error = None;
                ctx.request_repaint();
            }
        }

        if let Some(page) = self
            .sidebar
            .ui(root, &self.document, &mut self.render_worker)
        {
            self.go_to_page(page);
            ctx.request_repaint();
        }

        egui::CentralPanel::default().show_inside(root, |ui| {
            let available = ui.available_size();
            let dpi = ctx.pixels_per_point();
            let viewport = ((available.x * dpi) as u32, (available.y * dpi) as u32);
            let size = match self.document.page_size(self.document.current_page()) {
                Ok(size) => size,
                Err(error) => {
                    self.error = Some(format!("Failed to read page size: {error:#}"));
                    return;
                }
            };
            let scale = self.zoom.scale(viewport, size, dpi);
            let effective = scale / (POINT_SCALE * dpi);
            if self.effective_zoom != effective {
                self.effective_zoom = effective;
                ctx.request_repaint();
            }
            let key = RenderKey::new(self.document.current_page(), scale);
            if self
                .rendered
                .is_some_and(|previous| previous.page != key.page)
            {
                self.page_texture = None;
                self.rendered = None;
            }
            let mut pending = false;
            if self.rendered != Some(key) {
                match self.render_worker.image(key, Priority::Page) {
                    Some(Ok(image)) => {
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
                    Some(Err(error)) => {
                        self.page_texture = None;
                        self.error = Some(format!("Failed to render page: {error:#}"));
                        self.rendered = Some(key);
                        ctx.request_repaint();
                    }
                    None => pending = true,
                }
            }
            // Leave room for visible thumbnails. Large renders do not prefetch,
            // so low-priority neighbours cannot thrash the bounded pixel cache.
            if size.0 * size.1 * scale * scale * 4.0 < 32.0 * 1024.0 * 1024.0 {
                for neighbour in [key.page.checked_sub(1), key.page.checked_add(1)]
                    .into_iter()
                    .flatten()
                {
                    if neighbour < self.document.page_count()
                        && let Ok(size) = self.document.page_size(neighbour)
                    {
                        let neighbour_scale = self.zoom.scale(viewport, size, dpi);
                        if size.0 * size.1 * neighbour_scale * neighbour_scale * 4.0
                            < 32.0 * 1024.0 * 1024.0
                        {
                            self.render_worker.image(
                                RenderKey::new(neighbour, neighbour_scale),
                                Priority::Prefetch,
                            );
                        }
                    }
                }
            }
            egui::ScrollArea::both()
                .id_salt(("page", self.document.current_page()))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if let Some(texture) = &self.page_texture {
                        let previous_scale = self.rendered.map_or(scale, RenderKey::scale);
                        let size = texture.size_vec2() / dpi * (scale / previous_scale);
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
                    } else if pending {
                        ui.vertical_centered(|ui| {
                            ui.spinner();
                            ui.label("Rendering page…");
                        });
                    }
                });
        });
        self.render_worker.end_frame(&ctx);
        if let Some(events) = print_input {
            ctx.input_mut(|input| input.events = events);
        }
        if !print_active {
            self.printing.ui(&ctx, &self.document);
        }
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
    fn zoom_entry_applies_typed_value_and_escape_cancels_without_quitting() {
        let mut viewer = Viewer::new(crate::document::tests::sample_document());
        let ctx = egui::Context::default();
        let key = |key, modifiers| egui::Event::Key {
            key,
            modifiers,
            physical_key: None,
            pressed: true,
            repeat: false,
        };
        for events in [
            vec![key(egui::Key::L, egui::Modifiers::COMMAND)],
            vec![egui::Event::Text("137.5".into())],
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        ] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 720.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| viewer.ui(ui, &mut false));
        }
        assert_eq!(viewer.zoom, crate::zoom::Zoom::Percent(1.375));
        for events in [
            vec![key(egui::Key::L, egui::Modifiers::COMMAND)],
            vec![egui::Event::Text("300".into())],
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
        ] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 720.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| viewer.ui(ui, &mut false));
        }
        assert_eq!(viewer.zoom, crate::zoom::Zoom::Percent(1.375));
        assert!(!viewer.quit);
    }

    #[test]
    fn print_shortcut_and_escape_preserve_search_and_navigation() {
        let mut viewer = Viewer::new(crate::document::tests::sample_document());
        viewer.search.open = true;
        let ctx = egui::Context::default();
        for (key, modifiers, open) in [
            (egui::Key::P, egui::Modifiers::COMMAND, true),
            (egui::Key::ArrowRight, egui::Modifiers::NONE, true),
            (egui::Key::F9, egui::Modifiers::NONE, true),
            (egui::Key::Escape, egui::Modifiers::NONE, false),
        ] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 720.0),
                )),
                events: vec![egui::Event::Key {
                    key,
                    modifiers,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                }],
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| viewer.ui(ui, &mut false));
            assert_eq!(viewer.printing.open, open);
            assert!(viewer.search.open);
            assert!(viewer.sidebar.open);
            assert_eq!(viewer.document.current_page(), 0);
            assert!(!viewer.quit);
        }
    }

    #[test]
    fn print_shortcut_respects_document_permission() {
        use mupdf::pdf::{Encryption, Permission};
        let directory = tempfile::tempdir().unwrap();
        for (permissions, allowed) in [
            (Permission::ACCESSIBILITY, false),
            (Permission::PRINT, true),
        ] {
            // A stopped render worker may still be releasing its native file
            // handle; keep each fixture independent on Windows as well.
            let path = directory
                .path()
                .join(format!("restricted-{}.pdf", permissions.bits()));
            crate::document::tests::encrypted_fixture(&path, "", permissions, Encryption::Aes256);
            let mut viewer = Viewer::new(crate::document::PdfDocument::open(&path).unwrap());
            let ctx = egui::Context::default();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 720.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::P,
                    modifiers: egui::Modifiers::COMMAND,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                }],
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| viewer.ui(ui, &mut false));
            assert_eq!(viewer.printing.open, allowed);
            assert_eq!(viewer.error.is_none(), allowed);
            assert!(!viewer.quit);
        }
    }

    #[test]
    fn page_editing_consumes_q_and_escape_without_quitting() {
        let mut viewer = Viewer::new(crate::document::tests::sample_document());
        let ctx = egui::Context::default();
        let key = |key, modifiers| egui::Event::Key {
            key,
            modifiers,
            physical_key: None,
            pressed: true,
            repeat: false,
        };
        for (event, expected) in [
            (key(egui::Key::G, egui::Modifiers::COMMAND), "1"),
            (egui::Event::Text("q".into()), "q"),
            (key(egui::Key::Escape, egui::Modifiers::NONE), "1"),
        ] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 720.0),
                )),
                events: vec![event],
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| viewer.ui(ui, &mut false));
            assert!(!viewer.quit);
            assert_eq!(viewer.page_input, expected);
            assert_eq!(viewer.document.current_page(), 0);
        }
    }

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
        let _ = egui::Context::default().run_ui(input, |ui| viewer.ui(ui, &mut false));
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
