use egui::{Color32, Context, Key, Modifiers, TextureHandle, Vec2};

use crate::{
    document::PdfDocument,
    inspector::Inspector,
    links::{self, LinkTarget, PageLink},
    navigation::{History, ViewState, destination_view},
    page_text::PageText,
    persistence::{ReadingState, SidebarState},
    printing::PrintDialog,
    render_worker::{Priority, RenderKey, RenderWorker},
    search::Search,
    selection::Selection,
    sidebar::{Sidebar, SidebarTarget},
    zoom::{POINT_SCALE, Zoom},
};

pub struct Viewer {
    document: PdfDocument,
    state_key: std::path::PathBuf,
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
    selection: Selection,
    page_text: PageText,
    history: History,
    position: [f32; 2],
    restore_position: bool,
    viewport: [f32; 2],
    links: Vec<PageLink>,
    links_page: Option<usize>,
    pub inspector: Inspector,
}

impl Viewer {
    pub fn new(document: PdfDocument) -> Self {
        let sidebar = Sidebar::new(&document);
        let render_worker = RenderWorker::new(document.worker_source());
        let state_key = crate::persistence::file_key(document.path());
        Self {
            document,
            state_key,
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
            selection: Selection::default(),
            page_text: PageText::default(),
            history: History::default(),
            position: [0.0; 2],
            restore_position: false,
            viewport: [960.0, 720.0],
            links: Vec::new(),
            links_page: None,
            inspector: Inspector::default(),
        }
    }

    pub fn title(&self) -> String {
        let label = self
            .document
            .page_label(self.document.current_page())
            .ok()
            .filter(|label| {
                !label.is_empty() && *label != (self.document.current_page() + 1).to_string()
            })
            .map_or_else(String::new, |label| format!(" [{label}]"));
        format!(
            "Review — {} — {}/{}{} — {}",
            self.document.name(),
            self.document.current_page() + 1,
            self.document.page_count(),
            label,
            self.zoom.label()
        )
    }

    pub fn path(&self) -> &std::path::Path {
        self.document.path()
    }

    pub fn print_if_requested(&mut self, window: &winit::window::Window) {
        self.printing.run_requested(&self.document, window);
    }

    pub fn modal_open(&self) -> bool {
        self.printing.open
    }

    fn view_state(&self) -> ViewState {
        ViewState {
            page: self.document.current_page(),
            position: self.position,
            zoom: self.zoom,
        }
    }

    pub fn state_key(&self) -> &std::path::Path {
        &self.state_key
    }

    pub fn reading_state(&self) -> ReadingState {
        ReadingState {
            page: self.document.current_page(),
            scroll: self.position,
            zoom: self.zoom,
        }
    }

    fn apply_view(&mut self, view: ViewState) {
        self.document.go_to_page(view.page);
        self.zoom = view.zoom;
        self.position = view.position;
        self.restore_position = true;
        self.reveal_match = false;
        self.page_input = (view.page + 1).to_string();
        self.error = None;
    }

    fn visit(&mut self, view: ViewState) {
        self.history.visit(self.view_state(), view);
        self.apply_view(view);
    }

    fn go_to_destination(&mut self, destination: mupdf::link::LinkDestination) {
        let page = destination.loc.page_number as usize;
        match self.document.page_bounds(page) {
            Ok(bounds) => self.visit(destination_view(
                self.view_state(),
                page,
                destination.kind,
                bounds,
                self.viewport,
            )),
            Err(error) => self.error = Some(format!("Cannot follow destination: {error:#}")),
        }
    }

    fn navigate_history(&mut self, backwards: bool) {
        let current = self.view_state();
        let view = if backwards {
            self.history.back(current)
        } else {
            self.history.forward(current)
        };
        if let Some(view) = view {
            self.apply_view(view);
        }
    }

    pub fn save_attachment(&mut self, index: usize, path: &std::path::Path) {
        self.inspector
            .save_attachment(index, path, self.document.path());
    }

    pub fn sidebar_state(&self) -> SidebarState {
        self.sidebar.state()
    }

    pub fn restore_sidebar(&mut self, state: &SidebarState) {
        self.sidebar.restore(state);
    }

    pub fn restore_reading(&mut self, state: &ReadingState) {
        // A file may have been replaced by a shorter PDF since the last visit.
        self.apply_view(ViewState {
            page: state.page.min(self.document.page_count() - 1),
            position: state.scroll,
            zoom: state.zoom,
        });
        if let Zoom::Percent(value) = self.zoom {
            self.effective_zoom = value;
        }
    }

    fn go_to_page(&mut self, page: usize) {
        if page < self.document.page_count() && page != self.document.current_page() {
            self.visit(ViewState {
                page,
                position: [0.0; 2],
                ..self.view_state()
            });
        }
        self.page_input = (self.document.current_page() + 1).to_string();
        self.error = None;
    }

    fn change_page(&mut self, delta: i32) {
        let current = self.view_state();
        if self.document.change_page(delta) {
            let next = ViewState {
                page: self.document.current_page(),
                position: [0.0; 2],
                ..current
            };
            self.history.visit(current, next);
            self.apply_view(next);
        }
    }

    fn submit_page(&mut self) {
        match self.document.resolve_page(&self.page_input) {
            Ok(page) => self.go_to_page(page),
            Err(error) => self.error = Some(format!("{error:#}")),
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
        let previous = (self.document.current_page(), self.zoom);
        if root.is_enabled() && ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::P))
        {
            if self.document.permissions().print {
                self.printing.open = true;
            } else {
                self.error = Some("This PDF does not allow printing".into());
            }
        }
        // Register the modal backdrop before the viewer and keep its keyboard
        // events out of global page, search, zoom and sidebar shortcuts.
        let print_active = self.printing.open && root.is_enabled();
        if print_active {
            self.printing.ui(&ctx, &self.document);
        }
        let print_input =
            print_active.then(|| ctx.input_mut(|input| std::mem::take(&mut input.events)));
        let shortcuts = root.is_enabled() && !print_active;
        if shortcuts && ctx.input_mut(|input| input.consume_key(Modifiers::ALT, Key::ArrowLeft)) {
            self.navigate_history(true);
        } else if shortcuts
            && ctx.input_mut(|input| input.consume_key(Modifiers::ALT, Key::ArrowRight))
        {
            self.navigate_history(false);
        }
        let mut focus_page =
            shortcuts && ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::G));
        let mut focus_zoom =
            shortcuts && ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::L));
        if shortcuts && ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::D)) {
            self.inspector.open = !self.inspector.open;
        }
        if shortcuts
            && self.inspector.open
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.inspector.open = false;
        }
        if shortcuts && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::F9)) {
            self.sidebar.open = !self.sidebar.open;
        }
        let mut focus_search =
            shortcuts && ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::F));
        if focus_search {
            self.search.open = true;
        }
        if shortcuts
            && self.search.open
            && !ctx.memory(|memory| {
                ["page_input", "zoom_input", "page_text"]
                    .iter()
                    .any(|id| memory.had_focus_last_frame(egui::Id::new(id)))
            })
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.search.open = false;
            self.search.clear_results();
            ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new("search_query")));
        }
        if shortcuts && ctx.input_mut(|input| input.consume_key(Modifiers::SHIFT, Key::F3)) {
            self.advance_match(true);
        } else if shortcuts && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::F3)) {
            self.advance_match(false);
        }
        if shortcuts
            && ctx
                .input_mut(|input| input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::T))
        {
            self.page_text.toggle(&ctx);
        }
        if shortcuts {
            let backwards = ctx.input_mut(|input| input.consume_key(Modifiers::SHIFT, Key::F6));
            if backwards || ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::F6)) {
                let mut fields = vec![egui::Id::new("page_input"), egui::Id::new("zoom_input")];
                if self.search.open {
                    fields.push(egui::Id::new("search_query"));
                }
                if self.page_text.open {
                    fields.push(egui::Id::new("page_text"));
                }
                let focused = ctx.memory(|memory| memory.focused());
                let next = match fields.iter().position(|id| Some(*id) == focused) {
                    Some(index) if backwards => (index + fields.len() - 1) % fields.len(),
                    Some(index) => (index + 1) % fields.len(),
                    None if backwards => fields.len() - 1,
                    None => 0,
                };
                ctx.memory_mut(|memory| memory.request_focus(fields[next]));
                ctx.request_repaint();
                focus_page |= next == 0;
                focus_zoom |= next == 1;
                focus_search |= fields[next] == egui::Id::new("search_query");
            }
            ctx.input_mut(|input| {
                if input.consume_key(Modifiers::COMMAND, Key::Plus)
                    || input.consume_key(Modifiers::COMMAND, Key::Equals)
                {
                    self.change_zoom(1.25);
                }
                if input.consume_key(Modifiers::COMMAND, Key::Minus) {
                    self.change_zoom(0.8);
                }
                if input.consume_key(Modifiers::COMMAND, Key::Num0) {
                    self.zoom = Zoom::FitPage;
                }
            });
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
                let page_label = ui.label("Page");
                let field = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.page_input)
                            .id(egui::Id::new("page_input"))
                            .desired_width(48.0),
                    )
                    .labelled_by(page_label.id)
                    .on_hover_text("Physical page number or exact PDF label (Ctrl+G / Cmd+G). Numbers always select physical pages.");
                if focus_page {
                    select_text(&ctx, &field, self.page_input.chars().count());
                }
                ui.label(format!("/ {}", self.document.page_count()));
                if (field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)))
                    || ui.button("Go").clicked()
                {
                    self.submit_page();
                }
                if shortcuts
                    && (field.has_focus() || field.lost_focus())
                    && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
                {
                    field.surrender_focus();
                    self.go_to_page(self.document.current_page());
                }
                if let Ok(label) = self.document.page_label(self.document.current_page())
                    && !label.is_empty() && label != (self.document.current_page() + 1).to_string() {
                    ui.label(format!("Label: {label}"));
                }
                ui.separator();
                let zoom_out = ui.button("−").on_hover_text("Zoom out");
                zoom_out.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Zoom out")
                });
                if zoom_out.clicked() {
                    self.change_zoom(0.8);
                }
                let field = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.zoom_input)
                            .id(egui::Id::new("zoom_input"))
                            .desired_width(44.0),
                    )
                    .on_hover_text("Zoom percentage (Ctrl+L / Cmd+L), 10–1600%");
                ctx.accesskit_node_builder(field.id, |node| node.set_label("Zoom percentage"));
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
                if shortcuts
                    && (field.has_focus() || field.lost_focus())
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
                let zoom_in = ui.button("+").on_hover_text("Zoom in");
                zoom_in.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Zoom in")
                });
                if zoom_in.clicked() {
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
                if ui
                    .selectable_label(self.page_text.open, "Page text")
                    .on_hover_text("Read extracted page text (Ctrl+Shift+T / Cmd+Shift+T)")
                    .clicked()
                {
                    self.page_text.toggle(&ctx);
                }
                ui.separator();
                if ui
                    .add_enabled(self.history.can_back(), egui::Button::new("Back"))
                    .on_hover_text("Alt+Left")
                    .clicked()
                {
                    self.navigate_history(true);
                }
                if ui
                    .add_enabled(self.history.can_forward(), egui::Button::new("Forward"))
                    .on_hover_text("Alt+Right")
                    .clicked()
                {
                    self.navigate_history(false);
                }
                if ui.selectable_label(self.inspector.open, "Properties").on_hover_text("Ctrl+D / Cmd+D").clicked() {
                    self.inspector.open = !self.inspector.open;
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
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });

        if self.search.open {
            egui::Panel::top("search_bar").show_inside(root, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let search_label = ui.label("Find");
                    let field = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.search.query)
                                .id(egui::Id::new("search_query"))
                                .desired_width(220.0)
                                .hint_text("Search this document"),
                        )
                        .labelled_by(search_label.id);
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
                        field.surrender_focus();
                    }
                });
            });
        }

        if shortcuts {
            self.selection.escape(&ctx);
        }
        // Unmodified document keys must not steal arrows or editing keys from
        // focused controls. Modified app commands are handled above.
        if shortcuts && !self.inspector.open && ctx.memory(|memory| memory.focused().is_none()) {
            let previous = (self.document.current_page(), self.zoom);
            ctx.input_mut(|input| {
                if input.consume_key(Modifiers::NONE, Key::ArrowLeft)
                    || input.consume_key(Modifiers::NONE, Key::PageUp)
                {
                    self.change_page(-1);
                }
                if input.consume_key(Modifiers::NONE, Key::ArrowRight)
                    || input.consume_key(Modifiers::NONE, Key::PageDown)
                {
                    self.change_page(1);
                }
                if input.consume_key(Modifiers::NONE, Key::Home) {
                    self.go_to_page(0);
                }
                if input.consume_key(Modifiers::NONE, Key::End) {
                    self.go_to_page(self.document.page_count() - 1);
                }
                if input.consume_key(Modifiers::NONE, Key::Num1) {
                    self.zoom = Zoom::Percent(1.0);
                    self.effective_zoom = 1.0;
                }
                if input.consume_key(Modifiers::NONE, Key::Plus)
                    || input.consume_key(Modifiers::NONE, Key::Equals)
                {
                    self.change_zoom(1.25);
                }
                if input.consume_key(Modifiers::NONE, Key::Minus) {
                    self.change_zoom(0.8);
                }
                if input.consume_key(Modifiers::NONE, Key::Num0) {
                    self.zoom = Zoom::FitPage;
                }
                if input.consume_key(Modifiers::NONE, Key::Num2) {
                    self.zoom = Zoom::FitWidth;
                }
            });
            if previous != (self.document.current_page(), self.zoom) {
                self.error = None;
            }
        }

        if let Some(target) = self
            .sidebar
            .ui(root, &self.document, &mut self.render_worker)
        {
            match target {
                SidebarTarget::Page(page) => self.go_to_page(page),
                SidebarTarget::Destination(destination) => self.go_to_destination(destination),
            }
            ctx.request_repaint();
        }
        self.page_text.ui(root, &self.document);

        egui::CentralPanel::default().show_inside(root, |ui| {
            let available = ui.available_size();
            self.viewport = [available.x, available.y];
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
            if self.links_page != Some(self.document.current_page()) {
                self.links_page = Some(self.document.current_page());
                self.links = match self.document.links(self.document.current_page()) {
                    Ok(links) => links,
                    Err(error) => {
                        self.error = Some(format!("Cannot load page links: {error:#}"));
                        Vec::new()
                    }
                };
            }
            let mut scroll = egui::ScrollArea::both()
                .id_salt(("page", &self.state_key))
                .animated(false)
                .auto_shrink([false, false]);
            let logical_scale = scale / dpi;
            let mut page_origin = Vec2::ZERO;
            if let Some(texture) = &self.page_texture {
                let previous_scale = self.rendered.map_or(scale, RenderKey::scale);
                let size = texture.size_vec2() / dpi * (scale / previous_scale);
                page_origin = (available.max(size + Vec2::splat(32.0)) - size) * 0.5;
                if self.restore_position {
                    let offset = page_origin + Vec2::from(self.position) * logical_scale;
                    scroll = scroll.scroll_offset(offset);
                    self.restore_position = false;
                }
            }
            let mut activated = None;
            let output = scroll.show(ui, |ui| {
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
                    if let Err(error) = self.selection.ui(
                        ui,
                        &self.document,
                        page,
                        self.document.permissions().copy,
                    ) {
                        self.error = Some(format!("Failed to read page text: {error:#}"));
                    }
                    activated = links::ui(ui, &self.links, page);
                } else if pending {
                    ui.vertical_centered(|ui| {
                        ui.spinner();
                        ui.label("Rendering page…");
                    });
                }
            });
            if self.page_texture.is_some() {
                let position = (output.state.offset - page_origin).max(Vec2::ZERO) / logical_scale;
                self.position = [position.x, position.y];
            }
            if let Some(target) = activated {
                match target {
                    LinkTarget::Internal(destination) => self.go_to_destination(destination),
                    LinkTarget::External(url) => ctx.open_url(egui::OpenUrl::new_tab(url.as_str())),
                }
                ctx.request_repaint();
            }
        });
        if shortcuts && self.inspector.ui(&ctx, &mut self.document) {
            self.render_worker = RenderWorker::new(self.document.worker_source());
            self.rendered = None;
            self.page_texture = None;
            self.sidebar.clear_previews();
            self.search.clear_results();
            self.selection.clear();
            self.page_text.invalidate();
            self.links_page = None;
            ctx.request_repaint();
        }
        self.render_worker.end_frame(&ctx);
        if let Some(events) = print_input {
            ctx.input_mut(|input| input.events = events);
        }
        if !print_active && root.is_enabled() {
            self.printing.ui(&ctx, &self.document);
        }
        if previous != (self.document.current_page(), self.zoom) {
            ctx.request_repaint();
        }
    }
}

fn select_text(ctx: &Context, response: &egui::Response, len: usize) {
    if !response.has_focus() {
        response.request_focus();
    }
    ctx.request_repaint();
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
    use super::Viewer;

    fn frame(
        viewer: &mut Viewer,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 720.0),
                )),
                max_texture_side: Some(8192),
                events,
                ..Default::default()
            },
            |ui| viewer.ui(ui, &mut false),
        )
    }

    fn links_viewer() -> (tempfile::TempDir, Viewer) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("links.pdf");
        std::fs::write(&path, crate::links::tests::fixture()).unwrap();
        let mut viewer = Viewer::new(crate::document::PdfDocument::open(path).unwrap());
        viewer.sidebar.open = false;
        (directory, viewer)
    }

    fn settled_frame(viewer: &mut Viewer, ctx: &egui::Context) -> egui::FullOutput {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let output = frame(viewer, ctx, vec![]);
            let ready = viewer.rendered.is_some_and(|key| {
                key.page == viewer.document.current_page()
                    && (key.scale() / (crate::zoom::POINT_SCALE * ctx.pixels_per_point())
                        - viewer.effective_zoom)
                        .abs()
                        < 0.0001
            }) && viewer.page_texture.is_some()
                && !viewer.restore_position;
            if ready {
                return output;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "render did not settle: {:?}",
                viewer.error
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn destination_and_history_restore_rendered_scroll_position_and_zoom() {
        let (_directory, mut viewer) = links_viewer();
        let ctx = egui::Context::default();
        viewer.zoom = crate::zoom::Zoom::Percent(3.0);
        viewer.position = [33.0, 77.0];
        viewer.restore_position = true;
        settled_frame(&mut viewer, &ctx);
        let original = viewer.view_state();
        assert_eq!(original.position, [33.0, 77.0]);
        let crate::links::LinkTarget::Internal(dest) = viewer.links[0].target else {
            panic!("expected destination")
        };
        viewer.go_to_destination(dest);
        settled_frame(&mut viewer, &ctx);
        let target = viewer.view_state();
        assert_eq!(target.page, 1);
        assert_eq!(target.zoom, crate::zoom::Zoom::Percent(2.25));
        assert_eq!(target.position, [70.0, 300.0]);
        // Event modifiers must not accidentally also trigger previous/next page.
        let key = |key| egui::Event::Key {
            key,
            modifiers: egui::Modifiers::ALT,
            physical_key: None,
            pressed: true,
            repeat: false,
        };
        viewer.printing.open = true;
        frame(&mut viewer, &ctx, vec![key(egui::Key::ArrowLeft)]);
        assert_eq!(viewer.view_state(), target);
        viewer.printing.open = false;
        frame(&mut viewer, &ctx, vec![key(egui::Key::ArrowLeft)]);
        settled_frame(&mut viewer, &ctx);
        assert_eq!(viewer.view_state(), original);
        frame(&mut viewer, &ctx, vec![key(egui::Key::ArrowRight)]);
        settled_frame(&mut viewer, &ctx);
        assert_eq!(viewer.view_state(), target);
        viewer.navigate_history(true);
        viewer.go_to_page(1);
        assert!(!viewer.history.can_forward());
    }

    #[test]
    fn link_hover_click_and_copy_do_not_open_external_urls_without_a_primary_click() {
        let (_directory, mut viewer) = links_viewer();
        let ctx = egui::Context::default();
        let output = settled_frame(&mut viewer, &ctx);
        let texture = viewer.page_texture.as_ref().unwrap().id();
        let page = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id == texture => Some(mesh.calc_bounds()),
                _ => None,
            })
            .unwrap();
        let point = viewer.links[2].screen_bounds(page).center();
        let output = frame(&mut viewer, &ctx, vec![egui::Event::PointerMoved(point)]);
        assert_eq!(
            output.platform_output.cursor_icon,
            egui::CursorIcon::PointingHand
        );
        assert!(output.platform_output.commands.is_empty());
        let pointer = |pos, button, pressed| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            &mut viewer,
            &ctx,
            vec![pointer(point, egui::PointerButton::Secondary, true)],
        );
        frame(
            &mut viewer,
            &ctx,
            vec![pointer(point, egui::PointerButton::Secondary, false)],
        );
        // Popups use an invisible sizing pass on their first frame.
        let output = frame(&mut viewer, &ctx, vec![]);
        let copy = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Copy Link" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap();
        assert!(output.platform_output.commands.is_empty());
        frame(
            &mut viewer,
            &ctx,
            vec![
                egui::Event::PointerMoved(copy),
                pointer(copy, egui::PointerButton::Primary, true),
            ],
        );
        let output = frame(
            &mut viewer,
            &ctx,
            vec![pointer(copy, egui::PointerButton::Primary, false)],
        );
        assert!(output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "https://example.org/paper.pdf#page=3")));
        assert!(
            !output
                .platform_output
                .commands
                .iter()
                .any(|command| matches!(command, egui::OutputCommand::OpenUrl(_)))
        );
        frame(
            &mut viewer,
            &ctx,
            vec![
                egui::Event::PointerMoved(point),
                pointer(point, egui::PointerButton::Primary, true),
            ],
        );
        let output = frame(
            &mut viewer,
            &ctx,
            vec![pointer(point, egui::PointerButton::Primary, false)],
        );
        assert!(output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::OpenUrl(url) if url.url == "https://example.org/paper.pdf#page=3")));
        // Platform output is inspected, never dispatched to a browser in tests.
        assert_eq!(viewer.document.current_page(), 0);
        assert!(!viewer.history.can_back());
    }

    #[test]
    fn viewer_enforces_copy_permission_without_blocking_reusable_extraction() {
        use crate::document::{PdfDocument, tests::encrypted_fixture};
        use mupdf::pdf::{Encryption, Permission};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("restricted.pdf");
        encrypted_fixture(&path, "", Permission::ACCESSIBILITY, Encryption::Aes256);
        for (password, allowed) in [(None, false), (Some("owner-secret"), true)] {
            let document = PdfDocument::open_with_password(&path, password)
                .unwrap()
                .unwrap();
            assert_eq!(
                document.structured_text(0).unwrap().plain_text(),
                "Chapter one"
            );
            let mut viewer = Viewer::new(document);
            let ctx = egui::Context::default();
            let run = |viewer: &mut Viewer, events| {
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(960.0, 720.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| viewer.ui(ui, &mut false),
                )
            };
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while viewer.rendered.is_none() {
                run(&mut viewer, vec![]);
                assert!(std::time::Instant::now() < deadline, "page did not render");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(viewer.page_texture.is_some(), "{:?}", viewer.error);
            run(
                &mut viewer,
                vec![egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                }],
            );
            let output = run(&mut viewer, vec![egui::Event::Copy]);
            let copied = output
                .platform_output
                .commands
                .iter()
                .find_map(|cmd| match cmd {
                    egui::OutputCommand::CopyText(text) => Some(text.as_str()),
                    _ => None,
                });
            assert_eq!(copied, allowed.then_some("Chapter one"));
        }
    }

    #[test]
    fn selected_page_does_not_steal_field_copy_and_escape_clears_selection() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            crate::document::tests::sample_pdf(
                "BT /F1 16 Tf 40 350 Td (Selectable text) Tj ET",
                false,
            ),
        )
        .unwrap();
        let mut viewer = Viewer::new(crate::document::PdfDocument::open(file.path()).unwrap());
        let ctx = egui::Context::default();
        let command = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        let run = |viewer: &mut Viewer, events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(960.0, 720.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| viewer.ui(ui, &mut false),
            )
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while viewer.rendered.is_none() {
            run(&mut viewer, vec![]);
            assert!(std::time::Instant::now() < deadline, "page did not render");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(viewer.page_texture.is_some(), "{:?}", viewer.error);
        run(&mut viewer, vec![command(egui::Key::A)]);
        run(&mut viewer, vec![command(egui::Key::L)]);
        run(&mut viewer, vec![egui::Event::Text("137.5".into())]);
        run(&mut viewer, vec![command(egui::Key::A)]);
        let output = run(&mut viewer, vec![egui::Event::Copy]);
        assert!(
            output
                .platform_output
                .commands
                .iter()
                .any(|cmd| matches!(cmd,
            egui::OutputCommand::CopyText(text) if text == "137.5"))
        );
        let escape = || egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        run(&mut viewer, vec![escape()]); // Field cancels.
        assert_eq!(viewer.zoom, crate::zoom::Zoom::FitPage);
        run(&mut viewer, vec![escape()]); // Selection clears.
        let output = run(&mut viewer, vec![egui::Event::Copy]);
        assert!(
            !output
                .platform_output
                .commands
                .iter()
                .any(|command| matches!(command, egui::OutputCommand::CopyText(_)))
        );
        run(&mut viewer, vec![escape()]);
        assert_eq!(viewer.document.current_page(), 0);
    }

    #[test]
    fn restored_page_can_be_changed_with_page_entry() {
        let mut viewer = Viewer::new(crate::document::tests::sample_document());
        viewer.restore_reading(&crate::persistence::ReadingState {
            page: 1,
            zoom: crate::zoom::Zoom::Percent(6.0),
            ..Default::default()
        });
        let ctx = egui::Context::default();
        let mut library = crate::library::Library::default();
        let mut state = crate::persistence::State::default();
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        for events in [
            vec![],
            vec![key(egui::Key::G, egui::Modifiers::COMMAND)],
            vec![egui::Event::Text("1".into())],
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        ] {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    library.ui(ui, &mut state, Some(&viewer), true);
                    viewer.ui(ui, &mut false);
                },
            );
        }
        assert_eq!(viewer.document.current_page(), 0);
    }

    #[test]
    fn restored_scroll_is_applied_after_render_and_navigation_resets_it() {
        use crate::{persistence::ReadingState, zoom::Zoom};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scroll.pdf");
        std::fs::write(&path, crate::document::tests::sample_pdf("", false)).unwrap();
        let mut viewer = Viewer::new(crate::document::PdfDocument::open(path).unwrap());
        let location = ReadingState {
            page: 0,
            scroll: [37.0, 193.0],
            zoom: Zoom::Percent(3.0),
        };
        viewer.restore_reading(&location);
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(960.0, 720.0),
            )),
            ..Default::default()
        };
        // Exercise the empty asynchronous frame as well as the finished canvas.
        let _ = ctx.run_ui(input.clone(), |ui| viewer.ui(ui, &mut false));
        assert_eq!(viewer.reading_state(), location);
        for _ in 0..200 {
            let _ = ctx.run_ui(input.clone(), |ui| viewer.ui(ui, &mut false));
            if viewer.page_texture.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(viewer.page_texture.is_some());
        assert_eq!(viewer.reading_state(), location);
        viewer.change_page(1);
        let _ = ctx.run_ui(input.clone(), |ui| viewer.ui(ui, &mut false));
        assert_eq!(viewer.reading_state().scroll, [0.0, 0.0]);
        viewer.restore_reading(&ReadingState {
            page: usize::MAX,
            scroll: [1e6, 1e6],
            zoom: Zoom::FitPage,
        });
        for _ in 0..200 {
            let _ = ctx.run_ui(input.clone(), |ui| viewer.ui(ui, &mut false));
            if viewer.page_texture.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(viewer.page_texture.is_some());
        assert_eq!(viewer.reading_state().page, 1);
        assert_eq!(viewer.reading_state().scroll, [0.0, 0.0]);
        assert_eq!(viewer.page_input, "2");
    }

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
        let document = crate::document::tests::sample_document();
        assert_eq!(document.resolve_page("1").unwrap(), 0);
        assert_eq!(document.resolve_page(" 2 ").unwrap(), 1);
        for input in ["0", "3", "-1", "1.5", "abc", "", "999999999999999999999999"] {
            assert!(document.resolve_page(input).is_err(), "{input}");
        }
    }
}
