use egui::{Color32, Context, Key, Modifiers};

use crate::{
    annotations::{AnnotationUi, Kind},
    document::PdfDocument,
    forms::Forms,
    inspector::Inspector,
    layout::LayoutMode,
    links::LinkTarget,
    navigation::{History, ViewState, destination_view},
    ocr::Ocr,
    page_text::PageText,
    persistence::{ReadingState, SidebarState},
    printing::PrintDialog,
    reading::{ReadingFrame, ReadingSurface},
    render_worker::RenderWorker,
    search::Search,
    selection::Selection,
    sidebar::{Sidebar, SidebarTarget},
    zoom::Zoom,
};

pub struct Viewer {
    document: PdfDocument,
    state_key: std::path::PathBuf,
    zoom: Zoom,
    effective_zoom: f32,
    zoom_input: String,
    page_input: String,
    error: Option<String>,
    reading: ReadingSurface,
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
    pub inspector: Inspector,
    ocr: Ocr,
    text_revision: u64,
    annotations: AnnotationUi,
    forms: Forms,
    pub save_requested: Option<bool>,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.inspector.invalidate();
    }
}

impl Viewer {
    pub fn new(document: PdfDocument) -> Self {
        let sidebar = Sidebar::new(&document);
        let render_worker = RenderWorker::new(document.worker_source());
        let state_key = crate::persistence::file_key(document.path());
        let text_revision = document.text_revision();
        let search = Search::new(&document);
        Self {
            document,
            state_key,
            zoom: Zoom::FitPage,
            effective_zoom: 1.0,
            zoom_input: "100".into(),
            page_input: "1".into(),
            error: None,
            reading: ReadingSurface::default(),
            render_worker,
            search,
            reveal_match: false,
            sidebar,
            printing: PrintDialog::default(),
            selection: Selection::default(),
            page_text: PageText::default(),
            history: History::default(),
            position: [0.0; 2],
            restore_position: false,
            viewport: [960.0, 720.0],
            inspector: Inspector::default(),
            ocr: Ocr::default(),
            text_revision,
            annotations: AnnotationUi::default(),
            forms: Forms::default(),
            save_requested: None,
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
            "Review — {}{} — {}/{}{} — {}",
            self.document.name(),
            if self.document.is_dirty() { " *" } else { "" },
            self.document.current_page() + 1,
            self.document.page_count(),
            label,
            self.zoom.label()
        )
    }

    pub fn path(&self) -> &std::path::Path {
        self.document.path()
    }

    pub fn is_dirty(&self) -> bool {
        self.document.is_dirty()
    }

    pub fn save(&mut self, save_as: bool) -> bool {
        let path = if save_as {
            let mut chooser = rfd::FileDialog::new()
                .set_title("Save PDF As")
                .add_filter("PDF documents", &["pdf"])
                .set_file_name(self.document.name());
            if let Some(parent) = self.path().parent() {
                chooser = chooser.set_directory(parent);
            }
            let Some(path) = chooser.save_file() else {
                return false;
            };
            path
        } else {
            self.path().to_path_buf()
        };
        match self.document.save(&path) {
            Ok(()) => {
                let key = crate::persistence::file_key(self.path());
                if self.state_key != key {
                    self.state_key = key;
                    self.restore_position = true;
                }
                self.error = None;
                true
            }
            Err(error) => {
                self.error = Some(format!("Could not save PDF: {error:#}"));
                false
            }
        }
    }

    fn invalidate_document(&mut self, ctx: &Context) {
        self.render_worker = RenderWorker::new(self.document.worker_source());
        self.reading.displayed.clear();
        self.sidebar.clear_previews();
        self.selection.clear();
        self.search.reload_document(&self.document, ctx);
        self.page_text.invalidate();
        self.annotations.invalidate();
        self.forms.invalidate();
    }

    fn edited(&mut self, result: anyhow::Result<()>, ctx: &Context) {
        match result {
            Ok(()) => {
                self.ocr.cancel();
                self.invalidate_document(ctx);
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Document change failed: {error:#}")),
        }
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
            layout: self.reading.mode,
            rotation: self.reading.rotation,
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
            layout: self.reading.mode,
            rotation: self.reading.rotation,
        }
    }

    fn apply_view(&mut self, view: ViewState) {
        self.selection.clear();
        self.document.go_to_page(view.page);
        self.zoom = view.zoom;
        self.reading.mode = view.layout;
        self.reading.rotation = view.rotation;
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
            layout: state.layout,
            rotation: state.rotation,
        });
        if let Zoom::Percent(value) = self.zoom {
            self.effective_zoom = value;
        }
    }

    fn page_start(&self, page: usize) -> [f32; 2] {
        let corner = self.reading.rotation.inverse([0.0; 2]);
        self.document
            .page_size(page)
            .map_or([0.0; 2], |size| [corner[0] * size.0, corner[1] * size.1])
    }

    fn go_to_page(&mut self, page: usize) {
        if page < self.document.page_count() && page != self.document.current_page() {
            self.visit(ViewState {
                page,
                position: self.page_start(page),
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
                position: self.page_start(self.document.current_page()),
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
        self.ocr.poll(&self.document);
        if self.text_revision != self.document.text_revision() {
            self.text_revision = self.document.text_revision();
            self.selection.clear();
            self.reveal_match = false;
        }
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
        let fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
        if shortcuts && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::F11)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
        }
        if shortcuts && ctx.input_mut(|input| input.consume_key(Modifiers::ALT, Key::ArrowLeft)) {
            self.navigate_history(true);
        } else if shortcuts
            && ctx.input_mut(|input| input.consume_key(Modifiers::ALT, Key::ArrowRight))
        {
            self.navigate_history(false);
        }
        if shortcuts
            && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S))
        {
            self.save_requested = Some(true);
        } else if shortcuts && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S)) {
            self.save_requested = Some(false);
        }
        if shortcuts && !ctx.egui_wants_keyboard_input() {
            if ctx.input_mut(|i| {
                i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                    || i.consume_key(Modifiers::COMMAND, Key::Y)
            }) && self.document.can_redo()
            {
                self.inspector.invalidate();
                let result = self.document.redo();
                self.edited(result, &ctx);
            } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z))
                && self.document.can_undo()
            {
                self.inspector.invalidate();
                let result = self.document.undo();
                self.edited(result, &ctx);
            }
            if self.annotations.tool.is_some()
                && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
            {
                self.annotations.tool = None;
                self.annotations.invalidate();
            }
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
        self.search.refresh_text(&self.document, &ctx);
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
                fields.extend(self.forms.focus_ids());
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
                if ui.selectable_label(self.ocr.open, "OCR").clicked() {
                    if self.ocr.open {
                        self.ocr.cancel();
                    }
                    self.ocr.open = !self.ocr.open;
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
                if ui.button("Save").on_hover_text("Ctrl+S / Cmd+S").clicked() { self.save_requested = Some(false); }
                if ui.button("Save As…").on_hover_text("Ctrl+Shift+S / Cmd+Shift+S").clicked() { self.save_requested = Some(true); }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.selectable_label(self.annotations.open, "Annotations").clicked() {
                    self.annotations.open = !self.annotations.open;
                    self.annotations.tool = None;
                    self.forms.open = false;
                }
                let quads = self.selection.quads(self.document.current_page());
                for kind in [Kind::Highlight, Kind::Underline, Kind::StrikeOut] {
                    if ui.add_enabled(self.document.permissions().annotate && !quads.is_empty(), egui::Button::new(kind.label())).on_disabled_hover_text("Select text first; this PDF must allow annotations").clicked() {
                        self.inspector.invalidate();
                        let result = self.annotations.add_markup(&mut self.document, kind, quads.clone());
                        self.edited(result, &ctx);
                    }
                }
                if ui.add_enabled(self.document.can_undo(), egui::Button::new("Undo")).clicked() { self.inspector.invalidate(); let result = self.document.undo(); self.edited(result, &ctx); }
                if ui.add_enabled(self.document.can_redo(), egui::Button::new("Redo")).clicked() { self.inspector.invalidate(); let result = self.document.redo(); self.edited(result, &ctx); }
                if ui.selectable_label(self.forms.open, "Forms").clicked() {
                    self.forms.open = !self.forms.open;
                    self.annotations.open = false;
                    self.annotations.tool = None;
                    self.annotations.invalidate();
                }
                if self.document.is_dirty() { ui.label("Unsaved changes"); }
            });
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_label("Layout")
                    .selected_text(self.reading.mode.label())
                    .show_ui(ui, |ui| {
                        for mode in [LayoutMode::Single, LayoutMode::Continuous, LayoutMode::Facing] {
                            ui.selectable_value(&mut self.reading.mode, mode, mode.label());
                        }
                    });
                if ui.button("Rotate left").on_hover_text("Shift+R").clicked() {
                    self.reading.rotation.turn(false);
                }
                if ui.button("Rotate right").on_hover_text("R").clicked() {
                    self.reading.rotation.turn(true);
                }
                ui.separator();
                ui.selectable_value(&mut self.reading.hand, false, "Select text");
                ui.selectable_value(&mut self.reading.hand, true, "Hand tool").on_hover_text("Drag to pan (H)");
                if ui.selectable_label(fullscreen, "Fullscreen").on_hover_text("F11").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
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

        self.ocr.ui(root, &self.document);

        if self.search.open {
            egui::Panel::top("search_bar").show_inside(root, |ui| {
                let mut reveal_result = false;
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
                    let changed = ui
                        .checkbox(&mut self.search.options.case_sensitive, "Case sensitive")
                        .changed()
                        | ui.checkbox(&mut self.search.options.whole_word, "Whole words")
                            .changed();
                    if changed && !self.search.submitted.is_empty() {
                        self.search.start(&self.document, &ctx);
                    }
                    // Apply edits/cancellation before polling, so an old page
                    // cannot auto-navigate in the frame that replaces a query.
                    let selected = self.search.selected;
                    if let Some(page) = self.search.poll()
                        && !self
                            .forms
                            .focus_ids()
                            .any(|id| ctx.memory(|memory| memory.focused() == Some(id)))
                    {
                        self.go_to_page(page);
                        self.reveal_match = true;
                    }
                    reveal_result = selected != self.search.selected;
                    let enter =
                        field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                    if ui.button("Find").clicked() || (enter && self.search.submitted.is_empty()) {
                        self.search.start(&self.document, &ctx);
                    } else if enter {
                        self.advance_match(enter_backwards);
                    }
                    if enter {
                        field.request_focus();
                    }
                    let ready = !self.search.matches.is_empty();
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
                    if self.search.scanning() {
                        ui.spinner();
                        ui.label(format!(
                            "Searching… {}/{} pages · {} / {} matches so far",
                            self.search.scanned,
                            self.search.total,
                            self.search.selected.map_or(0, |index| index + 1),
                            self.search.matches.len()
                        ));
                    } else if self.search.error.is_none() && !self.search.submitted.is_empty() {
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
                if let Some(error) = &self.search.error {
                    ui.colored_label(Color32::LIGHT_RED, format!("Search failed: {error}"));
                }
                if !self.search.matches.is_empty() {
                    let mut results = egui::ScrollArea::vertical()
                        .id_salt("search_results")
                        .max_height(112.0)
                        .min_scrolled_height(112.0)
                        .auto_shrink([false, true]);
                    if (self.reveal_match || reveal_result)
                        && let Some(index) = self.search.selected
                    {
                        results = results.vertical_scroll_offset(
                            index as f32 * (22.0 + ui.spacing().item_spacing.y),
                        );
                    }
                    results.show_rows(ui, 22.0, self.search.matches.len(), |ui, rows| {
                        for index in rows {
                            let hit = &self.search.matches[index];
                            let mut label = egui::text::LayoutJob::default();
                            let font = egui::TextStyle::Body.resolve(ui.style());
                            let colour = ui.visuals().text_color();
                            let normal = egui::TextFormat {
                                font_id: font,
                                color: colour,
                                ..Default::default()
                            };
                            label.append(
                                &format!("{}   ", self.document.page_description(hit.page)),
                                0.0,
                                normal.clone(),
                            );
                            label.append(&hit.snippet[..hit.emphasis.start], 0.0, normal.clone());
                            let mut emphasized = normal.clone();
                            emphasized.background =
                                Color32::from_rgba_unmultiplied(255, 145, 0, 70);
                            label.append(&hit.snippet[hit.emphasis.clone()], 0.0, emphasized);
                            label.append(&hit.snippet[hit.emphasis.end..], 0.0, normal);
                            if ui
                                .add(
                                    egui::Button::selectable(
                                        self.search.selected == Some(index),
                                        label,
                                    )
                                    .min_size(egui::vec2(0.0, 22.0))
                                    .wrap_mode(egui::TextWrapMode::Truncate),
                                )
                                .on_hover_text(&hit.snippet)
                                .clicked()
                            {
                                let page = hit.page;
                                self.search.selected = Some(index);
                                self.go_to_page(page);
                                self.reveal_match = true;
                            }
                        }
                    });
                }
            });
        }

        if shortcuts {
            self.selection.escape(&ctx);
        }
        if shortcuts
            && !self.inspector.open
            && fullscreen
            && !ctx.egui_wants_keyboard_input()
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        // Unmodified document keys must not steal arrows or editing keys from
        // focused controls. Modified app commands are handled above.
        if shortcuts && !self.inspector.open && ctx.memory(|memory| memory.focused().is_none()) {
            let previous = (self.document.current_page(), self.zoom);
            ctx.input_mut(|input| {
                if input.consume_key(Modifiers::SHIFT, Key::R) {
                    self.reading.rotation.turn(false);
                } else if input.consume_key(Modifiers::NONE, Key::R) {
                    self.reading.rotation.turn(true);
                }
                if input.consume_key(Modifiers::NONE, Key::H) {
                    self.reading.hand = !self.reading.hand;
                }
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

        match self
            .annotations
            .panel(root, &mut self.document, || self.inspector.invalidate())
        {
            Ok(true) => self.edited(Ok(()), &ctx),
            Err(error) => self.edited(Err(error), &ctx),
            _ => {}
        }
        match self
            .forms
            .panel(root, &mut self.document, || self.inspector.invalidate())
        {
            Ok(true) => self.edited(Ok(()), &ctx),
            Err(error) => self.edited(Err(error), &ctx),
            _ => {}
        }
        let mut page_edited = false;
        egui::CentralPanel::default().show_inside(root, |ui| {
            if !shortcuts {
                ui.disable();
            }
            let available = ui.available_size();
            self.viewport = [available.x, available.y];
            let mut view = self.view_state();
            let activated = match self.reading.ui(
                ui,
                ReadingFrame {
                    document: &self.document,
                    state_key: &self.state_key,
                    worker: &mut self.render_worker,
                    selection: &mut self.selection,
                    search: &self.search,
                    selecting: self.annotations.tool.is_none(),
                    view: &mut view,
                    restore: &mut self.restore_position,
                    reveal_match: &mut self.reveal_match,
                },
            ) {
                Ok(target) => target,
                Err(error) => {
                    self.error = Some(format!("{error:#}"));
                    None
                }
            };
            if self.document.go_to_page(view.page) {
                self.page_input = (view.page + 1).to_string();
                ctx.request_repaint();
            }
            self.position = view.position;
            self.zoom = view.zoom;
            if self.effective_zoom != self.reading.effective_zoom {
                self.effective_zoom = self.reading.effective_zoom;
                ctx.request_repaint();
            }
            if let Some(target) = activated {
                match target {
                    LinkTarget::Internal(destination) => self.go_to_destination(destination),
                    LinkTarget::External(url) => ctx.open_url(egui::OpenUrl::new_tab(url.as_str())),
                }
                ctx.request_repaint();
            }
            if let Some(viewport) = self.reading.viewport {
                let mut overlay = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt("annotation_overlay")
                        .max_rect(viewport),
                );
                overlay.set_clip_rect(viewport);
                match self.annotations.page_ui(
                    &mut overlay,
                    &mut self.document,
                    &self.reading.screen_pages,
                    || self.inspector.invalidate(),
                ) {
                    Ok(changed) => page_edited |= changed,
                    Err(error) => self.error = Some(format!("Annotation change failed: {error:#}")),
                }
                if self.annotations.tool.is_none() {
                    match self.forms.page_ui(
                        &mut overlay,
                        &mut self.document,
                        &self.reading.screen_pages,
                    ) {
                        Ok(true) => {
                            self.annotations.open = false;
                            self.annotations.invalidate();
                        }
                        Err(error) => {
                            self.error = Some(format!("Cannot read form fields: {error:#}"))
                        }
                        _ => {}
                    }
                }
            }
        });
        if shortcuts && self.inspector.ui(&ctx, &mut self.document) {
            self.render_worker = RenderWorker::new(self.document.worker_source());
            self.reading.displayed.clear();
            self.sidebar.clear_previews();
            self.search.reload_document(&self.document, &ctx);
            self.selection.clear();
            self.page_text.invalidate();
            self.annotations.invalidate();
            self.forms.invalidate();
            ctx.request_repaint();
        }
        if page_edited {
            self.edited(Ok(()), &ctx);
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

    #[test]
    fn edits_undo_and_redo_replace_the_submitted_search_snapshot() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            crate::document::tests::sample_pdf(
                "BT /F1 16 Tf 40 350 Td (Original amber) Tj ET",
                false,
            ),
        )
        .unwrap();
        let mut viewer = Viewer::new(crate::document::PdfDocument::open(file.path()).unwrap());
        let ctx = egui::Context::default();
        viewer.search.query = "amber".into();
        viewer.search.options.case_sensitive = true;
        viewer.search.start(&viewer.document, &ctx);
        crate::search::tests::finish(&mut viewer.search);
        assert_eq!(viewer.search.matches.len(), 1);
        assert_eq!(viewer.search.matches[0].snippet, "Original amber");

        // Page content differs between snapshots. Annotation-only edits would
        // not expose a worker that still extracts text from the old snapshot.
        let result = viewer.document.edit(|pdf| {
            let buffer = mupdf::Buffer::from_bytes(
                b"BT /F1 16 Tf 40 350 Td (Changed amber twice amber) Tj ET",
            )?;
            let stream = pdf.add_stream(&buffer, None, false)?;
            pdf.find_page(0)?.dict_put("Contents", stream)?;
            Ok(())
        });
        viewer.edited(result, &ctx);
        for (undo, expected) in [
            (None, "Changed amber twice amber"),
            (Some(true), "Original amber"),
            (Some(false), "Changed amber twice amber"),
        ] {
            if let Some(undo) = undo {
                let result = if undo {
                    viewer.document.undo()
                } else {
                    viewer.document.redo()
                };
                viewer.edited(result, &ctx);
            }
            assert!(viewer.error.is_none(), "{:?}", viewer.error);
            assert_eq!(viewer.search.submitted, "amber");
            assert!(viewer.search.options.case_sensitive);
            crate::search::tests::finish(&mut viewer.search);
            assert_eq!(
                viewer.search.matches.len(),
                if undo == Some(true) { 1 } else { 2 }
            );
            assert!(
                viewer
                    .search
                    .matches
                    .iter()
                    .all(|hit| hit.page == 0 && hit.snippet == expected)
            );
        }
    }

    #[test]
    fn document_undo_shortcuts_do_not_undo_annotations_while_a_field_has_focus() {
        use crate::annotations::{Geometry, Kind};
        let mut document = crate::document::tests::sample_document();
        document
            .add_annotation(
                0,
                Kind::Note,
                Geometry::Note([0.31, 0.72]),
                "keyboard note",
                [0.9, 0.2, 0.1],
                2.0,
            )
            .unwrap();
        let mut viewer = Viewer::new(document);
        let context = egui::Context::default();
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            modifiers,
            pressed: true,
            repeat: false,
        };
        frame(
            &mut viewer,
            &context,
            vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
        );
        assert!(viewer.document.annotations(0).unwrap().is_empty());
        assert!(!viewer.is_dirty());
        frame(
            &mut viewer,
            &context,
            vec![key(
                egui::Key::Z,
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            )],
        );
        assert_eq!(viewer.document.annotations(0).unwrap().len(), 1);
        assert!(viewer.is_dirty());
        frame(
            &mut viewer,
            &context,
            vec![key(egui::Key::G, egui::Modifiers::COMMAND)],
        );
        assert!(context.egui_wants_keyboard_input());
        frame(
            &mut viewer,
            &context,
            vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
        );
        assert_eq!(viewer.document.annotations(0).unwrap().len(), 1);
        assert!(viewer.is_dirty());
    }

    fn settled_frame(viewer: &mut Viewer, ctx: &egui::Context) -> egui::FullOutput {
        // Native worker/font startup competes with parallel tests on CI. This
        // verifies the settled view, not a five-second rendering benchmark.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let output = frame(viewer, ctx, vec![]);
            let ready = !viewer.reading.displayed.is_empty()
                && viewer.reading.displayed.values().all(|p| {
                    p.rendered.is_some_and(|key| {
                        (key.scale() / (crate::zoom::POINT_SCALE * ctx.pixels_per_point())
                            - viewer.effective_zoom)
                            .abs()
                            < 0.0001
                    }) && p.texture.is_some()
                })
                && !viewer.restore_position;
            if ready {
                return output;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "render did not settle: error={:?}, rendered={:?}, zoom={}, restore={}",
                viewer.error,
                viewer
                    .reading
                    .displayed
                    .values()
                    .map(|p| p.rendered)
                    .collect::<Vec<_>>(),
                viewer.effective_zoom,
                viewer.restore_position
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
        let crate::links::LinkTarget::Internal(dest) = viewer.reading.displayed[&0].links[0].target
        else {
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
    fn multi_page_history_preserves_viewport_origins_outside_the_active_page() {
        use crate::{
            layout::{LayoutMode, Rotation},
            zoom::Zoom,
        };
        for mode in [LayoutMode::Continuous, LayoutMode::Facing] {
            for rotation in [
                Rotation::None,
                Rotation::Clockwise,
                Rotation::Half,
                Rotation::Counterclockwise,
            ] {
                let (_directory, mut viewer) = links_viewer();
                let ctx = egui::Context::default();
                viewer.reading.mode = mode;
                viewer.reading.rotation = rotation;
                viewer.zoom = Zoom::Percent(0.35);
                settled_frame(&mut viewer, &ctx);
                let point = viewer
                    .reading
                    .screen_pages
                    .iter()
                    .find(|(p, _)| *p == 1)
                    .unwrap()
                    .1
                    .rect
                    .center();
                for pressed in [true, false] {
                    frame(
                        &mut viewer,
                        &ctx,
                        vec![
                            egui::Event::PointerMoved(point),
                            egui::Event::PointerButton {
                                pos: point,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                    );
                }
                let original = viewer.view_state();
                assert_eq!(original.page, 1);
                let size = viewer.document.page_size(1).unwrap();
                assert!(
                    original.position[0] < 0.0
                        || original.position[0] > size.0
                        || original.position[1] < 0.0
                        || original.position[1] > size.1
                );
                let rects: Vec<_> = viewer
                    .reading
                    .screen_pages
                    .iter()
                    .map(|(p, t)| (*p, t.rect))
                    .collect();
                viewer.go_to_page(0);
                settled_frame(&mut viewer, &ctx);
                viewer.navigate_history(true);
                settled_frame(&mut viewer, &ctx);
                assert_eq!(viewer.view_state(), original);
                assert_eq!(
                    viewer
                        .reading
                        .screen_pages
                        .iter()
                        .map(|(p, t)| (*p, t.rect))
                        .collect::<Vec<_>>(),
                    rects
                );
            }
        }
    }

    #[test]
    fn rotated_continuous_history_and_pointer_zoom_use_original_pdf_points() {
        use crate::{
            layout::{LayoutMode, Rotation},
            zoom::Zoom,
        };
        let (_directory, mut viewer) = links_viewer();
        let ctx = egui::Context::default();
        viewer.reading.mode = LayoutMode::Continuous;
        viewer.reading.rotation = Rotation::Clockwise;
        viewer.zoom = Zoom::Percent(3.0);
        viewer.position = [133.0, 171.0];
        viewer.restore_position = true;
        settled_frame(&mut viewer, &ctx);
        let original = viewer.view_state();
        assert!((original.position[0] - 133.0).abs() < 0.001);
        assert!((original.position[1] - 171.0).abs() < 0.001);
        let crate::links::LinkTarget::Internal(dest) = viewer.reading.displayed[&0].links[0].target
        else {
            panic!()
        };
        viewer.go_to_destination(dest);
        settled_frame(&mut viewer, &ctx);
        let target = viewer.view_state();
        assert_eq!(target.page, 1);
        assert!((target.position[0] - 70.0).abs() < 0.001);
        assert!((target.position[1] - 300.0).abs() < 0.001);
        viewer.navigate_history(true);
        settled_frame(&mut viewer, &ctx);
        assert_eq!(viewer.view_state(), original);
        viewer.reading.rotation = Rotation::Half;
        viewer.navigate_history(false);
        settled_frame(&mut viewer, &ctx);
        assert_eq!(viewer.view_state(), target);

        let pointer = egui::pos2(423.0, 351.0);
        let before = viewer
            .reading
            .screen_pages
            .iter()
            .find(|(p, _)| *p == 1)
            .unwrap()
            .1;
        assert!(before.rect.contains(pointer));
        let original_point = before.normalized(pointer);
        frame(
            &mut viewer,
            &ctx,
            vec![egui::Event::PointerMoved(pointer), egui::Event::Zoom(1.3)],
        );
        settled_frame(&mut viewer, &ctx);
        let after = viewer
            .reading
            .screen_pages
            .iter()
            .find(|(p, _)| *p == 1)
            .unwrap()
            .1;
        // egui rounds the scroll translation to a physical pixel boundary.
        assert!((after.screen(original_point) - pointer).length() <= 1.0 / ctx.pixels_per_point());
        assert_eq!(viewer.zoom, Zoom::Percent(2.25 * 1.3));

        viewer.reading.hand = true;
        frame(&mut viewer, &ctx, vec![]);
        let old = viewer.view_state();
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut viewer, &ctx, vec![button(pointer, true)]);
        let end = pointer + egui::vec2(57.0, -83.0);
        frame(&mut viewer, &ctx, vec![egui::Event::PointerMoved(end)]);
        frame(&mut viewer, &ctx, vec![button(end, false)]);
        settled_frame(&mut viewer, &ctx);
        let next = viewer.view_state();
        let scale = 2.25 * 1.3 * crate::zoom::POINT_SCALE;
        assert!(((next.position[0] - old.position[0]) * scale - 83.0).abs() <= 1.0);
        assert!(((next.position[1] - old.position[1]) * scale - 57.0).abs() <= 1.0);
    }

    #[test]
    fn cross_page_selection_works_in_both_layouts_and_all_rotations() {
        use crate::{
            layout::{LayoutMode, Rotation},
            zoom::Zoom,
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("selection.pdf");
        std::fs::write(&path, crate::document::tests::sample_pdf(
            "BT /F1 16 Tf 40 350 Td (Alpha alpha) Tj 0 -24 Td (Needle) Tj 0 -24 Td (phrase) Tj ET", false)).unwrap();
        for mode in [LayoutMode::Continuous, LayoutMode::Facing] {
            for rotation in [
                Rotation::None,
                Rotation::Clockwise,
                Rotation::Half,
                Rotation::Counterclockwise,
            ] {
                let mut viewer = Viewer::new(crate::document::PdfDocument::open(&path).unwrap());
                viewer.sidebar.open = false;
                viewer.reading.mode = mode;
                viewer.reading.rotation = rotation;
                viewer.zoom = Zoom::Percent(0.4);
                let ctx = egui::Context::default();
                settled_frame(&mut viewer, &ctx);
                let a = viewer
                    .reading
                    .screen_pages
                    .iter()
                    .find(|(p, _)| *p == 0)
                    .unwrap()
                    .1
                    .screen([31.0 / 300.0, 64.0 / 400.0]);
                let b = viewer
                    .reading
                    .screen_pages
                    .iter()
                    .find(|(p, _)| *p == 1)
                    .unwrap()
                    .1
                    .screen([61.0 / 400.0, 74.0 / 200.0]);
                for (start, end) in [(a, b), (b, a)] {
                    let button = |pos, pressed| egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    };
                    frame(&mut viewer, &ctx, vec![egui::Event::PointerMoved(start)]);
                    frame(&mut viewer, &ctx, vec![button(start, true)]);
                    frame(&mut viewer, &ctx, vec![egui::Event::PointerMoved(end)]);
                    frame(&mut viewer, &ctx, vec![button(end, false)]);
                    let output = frame(&mut viewer, &ctx, vec![egui::Event::Copy]);
                    let copied = output
                        .platform_output
                        .commands
                        .iter()
                        .find_map(|cmd| match cmd {
                            egui::OutputCommand::CopyText(text) => Some(text.as_str()),
                            _ => None,
                        });
                    assert_eq!(
                        copied,
                        Some("Alpha alpha\nNeedle\nphrase\nLast"),
                        "{mode:?} {rotation:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn link_hover_click_and_copy_do_not_open_external_urls_without_a_primary_click() {
        let (_directory, mut viewer) = links_viewer();
        let ctx = egui::Context::default();
        let output = settled_frame(&mut viewer, &ctx);
        let texture = viewer.reading.displayed[&0].texture.as_ref().unwrap().id();
        let page = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id == texture => Some(mesh.calc_bounds()),
                _ => None,
            })
            .unwrap();
        let point = viewer.reading.displayed[&0].links[2]
            .screen_bounds(page)
            .center();
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
        // Allow the closed popup's cached hit region to expire before clicking
        // the page again, as the native repaint after dismissal does.
        frame(&mut viewer, &ctx, vec![]);
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
            settled_frame(&mut viewer, &ctx);
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
        settled_frame(&mut viewer, &ctx);
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
            ..Default::default()
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
        settled_frame(&mut viewer, &ctx);
        assert_eq!(viewer.reading_state(), location);
        viewer.change_page(1);
        let _ = ctx.run_ui(input.clone(), |ui| viewer.ui(ui, &mut false));
        assert_eq!(viewer.reading_state().scroll, [0.0, 0.0]);
        viewer.restore_reading(&ReadingState {
            page: usize::MAX,
            scroll: [1e6, 1e6],
            zoom: Zoom::FitPage,
            ..Default::default()
        });
        settled_frame(&mut viewer, &ctx);
        assert_eq!(viewer.reading_state().page, 1);
        // Fit-page has no scroll range: its centred page starts below/right of
        // the viewport origin, which is a signed original-page point.
        assert!(viewer.reading_state().scroll.iter().all(|p| *p < 0.0));
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
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            crate::document::tests::sample_pdf("BT /F1 16 Tf 40 350 Td (Alpha alpha) Tj ET", false),
        )
        .unwrap();
        let mut viewer = Viewer::new(crate::document::PdfDocument::open(file.path()).unwrap());
        let ctx = egui::Context::default();
        viewer.search.query = "alpha".into();
        viewer.search.start(&viewer.document, &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while viewer.search.scanning() {
            viewer.search.poll();
            assert!(
                std::time::Instant::now() < deadline,
                "search did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
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
        let _ = ctx.run_ui(input, |ui| viewer.ui(ui, &mut false));
        assert_eq!(viewer.search.selected, Some(2));
        assert_eq!(viewer.document.current_page(), 1);
    }

    #[test]
    fn recognition_restarts_submitted_search_and_removes_stale_matches() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("blank.pdf");
        std::fs::write(&path, crate::document::tests::sample_pdf("", false)).unwrap();
        let mut viewer = Viewer::new(crate::document::PdfDocument::open(&path).unwrap());
        viewer.search.query = "amber".into();
        viewer.search.open = true;
        let ctx = egui::Context::default();
        viewer.search.start(&viewer.document, &ctx);
        crate::search::tests::finish(&mut viewer.search);
        assert!(viewer.search.matches.is_empty());
        let run = |viewer: &mut Viewer| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(960.0, 720.0),
                    )),
                    ..Default::default()
                },
                |ui| viewer.ui(ui, &mut false),
            )
        };
        viewer
            .document
            .set_recognized_text(0, crate::ocr::tests::word_text("amber fox"))
            .unwrap();
        run(&mut viewer);
        crate::search::tests::finish(&mut viewer.search);
        run(&mut viewer);
        assert_eq!(viewer.search.matches.len(), 1);
        viewer
            .document
            .set_recognized_text(0, crate::ocr::tests::word_text("violet river"))
            .unwrap();
        run(&mut viewer);
        assert!(viewer.search.matches.is_empty());
        crate::search::tests::finish(&mut viewer.search);
        run(&mut viewer);
        assert!(viewer.search.matches.is_empty());
        assert_eq!(viewer.text_revision, 2);
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

    #[test]
    fn refreshing_search_does_not_navigate_away_from_a_focused_form_entry() {
        let mut document = crate::forms::tests::document();
        document
            .edit(|pdf| {
                let buffer =
                    mupdf::Buffer::from_bytes(b"BT /F1 12 Tf 40 350 Td (remote target) Tj ET")?;
                let stream = pdf.add_stream(&buffer, None, false)?;
                pdf.find_page(1)?.dict_put("Contents", stream)?;
                Ok(())
            })
            .unwrap();
        let mut viewer = Viewer::new(document);
        viewer.forms.open = true;
        let ctx = egui::Context::default();
        frame(&mut viewer, &ctx, Vec::new());
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new(("form", 10))));
        frame(&mut viewer, &ctx, Vec::new());
        viewer.search.open = true;
        viewer.search.query = "remote".into();
        viewer.search.start(&viewer.document, &ctx);
        let result =
            viewer
                .document
                .set_form_value(0, 10, crate::forms::Value::Text("Typed".into()));
        viewer.edited(result, &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while viewer.search.scanning() {
            assert!(
                std::time::Instant::now() < deadline,
                "search did not finish"
            );
            frame(&mut viewer, &ctx, Vec::new());
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(viewer.search.matches.len(), 1);
        assert_eq!(viewer.search.matches[0].page, 1);
        assert_eq!(viewer.document.current_page(), 0);
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(egui::Id::new(("form", 10)))
        );
    }
}
