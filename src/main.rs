#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod document;
mod inspection;
mod inspector;
mod library;
mod links;
#[cfg(target_os = "macos")]
mod macos;
mod native_ui;
mod navigation;
mod ocr;
mod page_text;
mod persistence;
mod printing;
mod render_worker;
mod renderer;
mod search;
mod selection;
mod sidebar;
mod structured_text;
mod viewer;
mod zoom;

use std::{env, ffi::OsString, path::PathBuf, sync::Arc, time::Instant};

use anyhow::{Context, Result, bail};
use egui::{Key, Modifiers};
use renderer::Renderer;
use viewer::Viewer;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    window::{Window, WindowId},
};
use zeroize::{Zeroize, Zeroizing};

struct PasswordPrompt {
    path: PathBuf,
    input: Zeroizing<String>,
    incorrect: bool,
    focus: bool,
    reading: Option<persistence::ReadingState>,
}

#[derive(Debug)]
enum AppEvent {
    #[cfg(target_os = "macos")]
    OpenFile(PathBuf),
    AccessKit(egui_winit::accesskit_winit::Event),
}

impl From<egui_winit::accesskit_winit::Event> for AppEvent {
    fn from(event: egui_winit::accesskit_winit::Event) -> Self {
        Self::AccessKit(event)
    }
}

struct App {
    viewer: Option<Viewer>,
    store: persistence::Store,
    library: library::Library,
    state_error: Option<String>,
    open_error: Option<String>,
    password_prompt: Option<PasswordPrompt>,
    renderer: Option<Renderer>,
    repaint_at: Option<Instant>,
    fatal_error: Option<anyhow::Error>,
    proxy: Option<EventLoopProxy<AppEvent>>,
    native_ui: native_ui::NativeUi,
}

impl App {
    fn new() -> Self {
        let (store, state_error) = persistence::Store::load();
        Self::with_state(store, state_error)
    }

    fn with_state(store: persistence::Store, state_error: Option<String>) -> Self {
        let mut native_ui = native_ui::NativeUi::default();
        native_ui.appearance = store.state.appearance;
        Self {
            viewer: None,
            store,
            library: library::Library::default(),
            state_error,
            open_error: None,
            password_prompt: None,
            renderer: None,
            repaint_at: None,
            fatal_error: None,
            proxy: None,
            native_ui,
        }
    }

    fn open(&mut self, path: PathBuf) {
        self.password_prompt = None;
        self.open_error = None;
        match document::PdfDocument::open_with_password(&path, None) {
            Ok(Some(document)) => self.finish_open(document, None),
            Ok(None) => {
                self.password_prompt = Some(PasswordPrompt {
                    path,
                    input: Zeroizing::new(String::new()),
                    incorrect: false,
                    focus: true,
                    reading: None,
                })
            }
            Err(error) => self.open_error = Some(format!("{error:#}")),
        }
        if let Some(renderer) = &self.renderer {
            renderer.window().request_redraw();
        }
    }

    fn submit_password(&mut self) {
        let Some(mut prompt) = self.password_prompt.take() else {
            return;
        };
        let result = document::PdfDocument::open_with_password(&prompt.path, Some(&prompt.input));
        prompt.input.zeroize();
        match result {
            Ok(Some(document)) => self.finish_open(document, prompt.reading.as_ref()),
            Ok(None) => {
                prompt.incorrect = true;
                prompt.focus = true;
                self.password_prompt = Some(prompt);
            }
            Err(error) => self.open_error = Some(format!("{error:#}")),
        }
        if let Some(renderer) = &self.renderer {
            renderer.window().request_redraw();
        }
    }

    fn finish_open(
        &mut self,
        document: document::PdfDocument,
        bookmark: Option<&persistence::ReadingState>,
    ) {
        self.capture_state();
        let mut viewer = Viewer::new(document);
        viewer.restore_sidebar(&self.store.state.sidebar);
        if let Some(reading) = bookmark.or_else(|| self.store.state.reading(viewer.state_key())) {
            viewer.restore_reading(reading);
        }
        self.store
            .state
            .opened(viewer.state_key().to_path_buf(), viewer.reading_state());
        self.viewer = Some(viewer);
        self.open_error = None;
    }

    fn open_bookmark(&mut self, bookmark: persistence::Bookmark) {
        if self
            .viewer
            .as_ref()
            .is_none_or(|viewer| viewer.state_key() != bookmark.path)
        {
            self.open(bookmark.path.clone());
        }
        if let Some(prompt) = &mut self.password_prompt {
            prompt.reading = Some(bookmark.reading);
            return;
        }
        // Failed opens leave the previous viewer untouched, including its location.
        if let Some(viewer) = &mut self.viewer
            && viewer.state_key() == bookmark.path
        {
            viewer.restore_reading(&bookmark.reading);
        }
    }

    fn capture_state(&mut self) {
        self.store.state.appearance = self.native_ui.appearance;
        if let Some(viewer) = &self.viewer {
            self.store
                .state
                .update_reading(viewer.state_key(), viewer.reading_state());
            self.store.state.sidebar = viewer.sidebar_state();
        }
        if let Some(renderer) = &self.renderer {
            let window = renderer.window();
            self.store.state.window.maximized = window.is_maximized();
            if !window.is_maximized()
                && window.fullscreen().is_none()
                && window.is_minimized() != Some(true)
            {
                let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                if size.width >= 320.0 && size.height >= 320.0 {
                    self.store.state.window.size = [size.width, size.height];
                }
                if let Ok(position) = window.outer_position() {
                    self.store.state.window.position = Some([position.x, position.y]);
                }
            }
        }
    }

    fn save_state(&mut self, force: bool) {
        self.capture_state();
        if let Err(error) = self.store.save(force) {
            self.state_error = Some(format!("Could not save reading state: {error:#}"));
        }
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            #[cfg(target_os = "macos")]
            AppEvent::OpenFile(path) => {
                self.open(path);
                if let Some(renderer) = &self.renderer {
                    renderer.window().focus_window();
                }
            }
            AppEvent::AccessKit(event) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.on_accesskit_event(event);
                }
            }
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() || self.fatal_error.is_some() {
            return;
        }
        let geometry = &self.store.state.window;
        let mut attributes = Window::default_attributes()
            .with_title("Review")
            .with_visible(false)
            .with_inner_size(winit::dpi::LogicalSize::new(
                geometry.size[0],
                geometry.size[1],
            ))
            .with_min_inner_size(winit::dpi::LogicalSize::new(480, 320))
            .with_maximized(geometry.maximized);
        if let Some([x, y]) = geometry.position {
            // Don't reopen entirely outside the desktop after a monitor is removed.
            let visible = event_loop.available_monitors().any(|monitor| {
                let origin = monitor.position();
                let size = monitor.size();
                i64::from(x) + 64 > i64::from(origin.x)
                    && i64::from(y) + 64 > i64::from(origin.y)
                    && i64::from(x) < i64::from(origin.x) + i64::from(size.width) - 64
                    && i64::from(y) < i64::from(origin.y) + i64::from(size.height) - 64
            });
            if visible {
                attributes = attributes.with_position(winit::dpi::PhysicalPosition::new(x, y));
            }
        }
        #[cfg(target_os = "linux")]
        let attributes = {
            use winit::platform::{
                wayland::WindowAttributesExtWayland, x11::WindowAttributesExtX11,
            };
            let attributes = WindowAttributesExtWayland::with_name(attributes, "review", "Review");
            WindowAttributesExtX11::with_name(attributes, "review", "Review")
        };
        let renderer = (|| {
            let window = Arc::new(
                event_loop
                    .create_window(attributes)
                    .context("failed to create window")?,
            );
            pollster::block_on(Renderer::new(
                window,
                event_loop,
                self.proxy.clone().unwrap(),
            ))
            .context("failed to initialize graphics")
        })();
        match renderer {
            Ok(renderer) => {
                self.native_ui.appearance.apply(&renderer.context);
                renderer.window().request_redraw();
                self.renderer = Some(renderer);
            }
            Err(error) => {
                self.fatal_error = Some(error);
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if renderer.window().id() != window_id {
            return;
        }
        renderer.on_event(&event);
        let mut force_save = matches!(event, WindowEvent::CloseRequested);
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => renderer.resize(size),
            WindowEvent::DroppedFile(path) => self.open(path),
            WindowEvent::RedrawRequested => {
                let input = renderer.take_input();
                let mut open_requested = false;
                let mut quit = false;
                let mut submit_password = false;
                let mut library_action = None;
                let mut output = renderer.context.run_ui(input, |ui| {
                    let library_enabled = self.password_prompt.is_none()
                        && !self.native_ui.help_open
                        && !self.viewer.as_ref().is_some_and(Viewer::modal_open)
                        && !ui.input(|input| input.key_pressed(Key::F1));
                    library_action = self.library.ui(
                        ui,
                        &mut self.store.state,
                        self.viewer.as_ref(),
                        library_enabled,
                    );
                    (open_requested, quit, submit_password) = app_ui(
                        &mut self.viewer,
                        &mut self.open_error,
                        &mut self.password_prompt,
                        ui,
                        &mut self.native_ui,
                    );
                    if let Some(error) = &self.state_error {
                        let mut dismiss = false;
                        egui::Window::new("Reading state").collapsible(false).show(
                            ui.ctx(),
                            |ui| {
                                ui.label(error);
                                dismiss = ui.button("Close").clicked();
                            },
                        );
                        if dismiss {
                            self.state_error = None;
                        }
                    }
                });
                let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
                self.repaint_at = Instant::now().checked_add(delay);
                let title = self
                    .viewer
                    .as_ref()
                    .map_or_else(|| "Review".into(), Viewer::title);
                renderer.window().set_title(&title);
                if let Some(update) = &mut output.platform_output.accesskit_update
                    && let Some(tree) = &update.tree
                    && let Some((_, root)) =
                        update.nodes.iter_mut().find(|(id, _)| *id == tree.root)
                {
                    root.set_label(title);
                }
                if let Err(error) = renderer.render(output) {
                    eprintln!("failed to draw frame: {error:#}");
                }
                if let Some(viewer) = &mut self.viewer {
                    viewer.print_if_requested(renderer.window());
                }
                if quit {
                    force_save = true;
                    event_loop.exit();
                } else if submit_password {
                    self.submit_password();
                } else if open_requested {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Open PDF")
                        .add_filter("PDF documents", &["pdf"]);
                    // rfd's Linux parent export borrows raw Wayland pointers
                    // across threads. Avoid that path while winit is blocked.
                    #[cfg(not(target_os = "linux"))]
                    {
                        dialog = dialog.set_parent(renderer.window());
                    }
                    if let Some(viewer) = &self.viewer
                        && let Some(directory) = viewer.path().parent()
                    {
                        dialog = dialog.set_directory(directory);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.open(path);
                    }
                } else if let Some(viewer) = &mut self.viewer
                    && let Some((index, filename)) = viewer.inspector.take_save_request()
                {
                    let dialog = rfd::FileDialog::new()
                        .set_title("Save attachment (will not open)")
                        .set_file_name(filename);
                    #[cfg(not(target_os = "linux"))]
                    let dialog = dialog.set_parent(renderer.window());
                    if let Some(path) = dialog.save_file() {
                        viewer.save_attachment(index, &path);
                    }
                    renderer.window().request_redraw();
                }
                match library_action {
                    Some(library::Action::Open(path)) => self.open(path),
                    Some(library::Action::Bookmark(bookmark)) => {
                        self.open_bookmark(bookmark);
                        if let Some(renderer) = &self.renderer {
                            renderer.window().request_redraw();
                        }
                    }
                    Some(library::Action::ClearHistory) => {
                        self.store.state.clear_history();
                        force_save = true;
                    }
                    None => {}
                }
            }
            _ => {}
        }
        self.save_state(force_save);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.save_state(false);
        let deadline = self
            .repaint_at
            .into_iter()
            .chain(self.store.save_deadline())
            .min();
        if let Some(deadline) = deadline {
            if deadline <= Instant::now() {
                if let Some(renderer) = &self.renderer {
                    renderer.window().request_redraw();
                }
                self.repaint_at = None;
                event_loop.set_control_flow(ControlFlow::Wait);
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            }
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.save_state(true);
        // Drop/join egui's clipboard worker before winit destroys the display.
        self.renderer = None;
    }
}

fn app_ui(
    viewer: &mut Option<Viewer>,
    open_error: &mut Option<String>,
    password_prompt: &mut Option<PasswordPrompt>,
    root: &mut egui::Ui,
    native_ui: &mut native_ui::NativeUi,
) -> (bool, bool, bool) {
    let ctx = root.ctx().clone();
    // Draw the modal first so its backdrop blocks pointer input immediately.
    // Isolate all input from Viewer::ui, including its global search shortcuts.
    let password_active = password_prompt.is_some();
    let mut submit_password = false;
    if let Some(prompt) = password_prompt {
        let cancel_key = ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        let mut cancel = cancel_key;
        egui::Modal::new(egui::Id::new("pdf_password_prompt")).show(&ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Password required");
            ui.label(
                prompt
                    .path
                    .file_name()
                    .unwrap_or(prompt.path.as_os_str())
                    .to_string_lossy(),
            );
            ui.label("Enter the password to open this PDF.");
            ui.add_space(8.0);
            let password_label = ui.label("Password");
            let mut edit = egui::TextEdit::singleline(&mut *prompt.input)
                .id(egui::Id::new("pdf_password"))
                .password(true)
                .desired_width(f32::INFINITY)
                .show(ui);
            // Password fields must not retain plaintext undo history in egui.
            edit.state.clear_undoer();
            edit.state.store(&ctx, edit.response.id);
            let field = edit.response.response.labelled_by(password_label.id);
            if prompt.focus {
                field.request_focus();
                prompt.focus = false;
            }
            // egui chooses directional focus before TextEdit installs its
            // arrow filter on the first focused pass. Keep arrows used to edit
            // the password from moving focus at the end of that pass.
            if field.has_focus()
                && ui.input(|input| {
                    input.key_pressed(Key::ArrowLeft) || input.key_pressed(Key::ArrowRight)
                })
            {
                ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
            }
            if prompt.incorrect {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "Incorrect password. Try again.",
                );
            }
            ui.label("Passwords are kept only in memory while the PDF is open.");
            ui.horizontal(|ui| {
                submit_password = ui.button("Open PDF").clicked()
                    || (field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)));
                cancel |= ui.button("Cancel").clicked();
            });
        });
        if cancel {
            *password_prompt = None;
            submit_password = false;
        }
    }
    let modal_input =
        password_active.then(|| ctx.input_mut(|input| std::mem::take(&mut input.events)));
    let quit = ctx.input_mut(|input| {
        input.consume_key(Modifiers::COMMAND, Key::Q)
            || input.consume_key(Modifiers::COMMAND, Key::W)
    });
    let blocked = native_ui.begin(root) || password_active;
    let mut open_requested = !blocked
        && !viewer.as_ref().is_some_and(Viewer::modal_open)
        && ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::O));
    if !blocked
        && open_error.is_some()
        && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
    {
        *open_error = None;
    }
    if blocked {
        root.disable();
    }
    if let Some(viewer) = viewer {
        viewer.ui(root, &mut open_requested);
    } else {
        egui::CentralPanel::default().show_inside(root, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(((ui.available_height() - 90.0) * 0.5).max(0.0));
                ui.heading("Review");
                ui.label("Open a PDF to get started");
                if ui.button("Open…").clicked() {
                    open_requested = true;
                }
                ui.label("Ctrl+O / Cmd+O, or drop a PDF here");
            });
        });
    }
    if let Some(error) = open_error.as_ref() {
        let mut dismiss = false;
        egui::Window::new("Cannot open PDF")
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(&ctx, |ui| {
                ui.label(error);
                dismiss = ui.button("Close").clicked();
            });
        if dismiss {
            *open_error = None;
        }
    }
    if let Some(events) = modal_input {
        ctx.input_mut(|input| input.events = events);
    }
    if !password_active {
        native_ui.finish(&ctx);
    }
    (
        open_requested && !password_active,
        quit && !password_active,
        submit_password,
    )
}

#[derive(Debug, PartialEq)]
enum Startup {
    Help,
    Open(Option<PathBuf>),
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Startup> {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(Startup::Open(None));
    };
    if first == "--help" || first == "-h" {
        return Ok(Startup::Help);
    }
    let path = if first == "--" {
        args.next().context("expected a PDF path after --")?
    } else {
        if first.to_string_lossy().starts_with('-') {
            bail!(
                "unknown option: {}; use --help for usage",
                first.to_string_lossy()
            );
        }
        first
    };
    if args.next().is_some() {
        bail!("Review opens one PDF at a time");
    }
    Ok(Startup::Open(Some(path.into())))
}

fn main() -> Result<()> {
    let startup = parse_args(env::args_os().skip(1))?;
    let Startup::Open(path) = startup else {
        println!(
            "Usage: review [document.pdf]\n\nOpen a PDF, or launch without arguments and use Open (Ctrl+O / Cmd+O).\n\nOptions:\n  -h, --help  Show this help without opening a window\n  --          Treat the following argument as a file path"
        );
        return Ok(());
    };
    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .context("failed to create event loop")?;
    #[cfg(target_os = "macos")]
    let _open_documents = macos::OpenDocuments::install(event_loop.create_proxy());
    let mut app = App::new();
    app.proxy = Some(event_loop.create_proxy());
    if let Some(path) = path {
        app.open(path);
    }
    let event_result = event_loop.run_app(&mut app);
    if let Some(error) = app.fatal_error {
        return Err(error);
    }
    event_result.context("event loop failed")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_event(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn password_frame(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> ((bool, bool, bool), egui::FullOutput) {
        let mut action = (false, false, false);
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 720.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                action = app_ui(
                    &mut app.viewer,
                    &mut app.open_error,
                    &mut app.password_prompt,
                    ui,
                    &mut app.native_ui,
                )
            },
        );
        (action, output)
    }

    #[test]
    fn password_retry_cancel_and_replacement_preserve_existing_view() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("locked.pdf");
        document::tests::encrypted_fixture(
            &path,
            "open-secret",
            mupdf::pdf::Permission::ACCESSIBILITY,
            mupdf::pdf::Encryption::Aes256,
        );
        let mut app = App::with_state(persistence::Store::temporary(directory.path()), None);
        app.viewer = Some(Viewer::new(document::tests::sample_document()));
        let ctx = egui::Context::default();
        password_frame(
            &mut app,
            &ctx,
            vec![
                key_event(Key::ArrowRight, Modifiers::NONE),
                key_event(Key::Plus, Modifiers::NONE),
            ],
        );
        let title = app.viewer.as_ref().unwrap().title();
        assert!(title.ends_with("2/2 — 125%"));
        app.open(path.clone());
        assert!(app.password_prompt.is_some());
        assert!(app.open_error.is_none());
        password_frame(&mut app, &ctx, vec![]);
        let (action, _) = password_frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::Text("wrong-q".into()),
                key_event(Key::ArrowRight, Modifiers::NONE),
                key_event(Key::Plus, Modifiers::NONE),
                key_event(Key::Q, Modifiers::NONE),
                key_event(Key::O, Modifiers::COMMAND),
            ],
        );
        assert_eq!(action, (false, false, false));
        assert_eq!(app.viewer.as_ref().unwrap().title(), title);
        let (action, _) =
            password_frame(&mut app, &ctx, vec![key_event(Key::Enter, Modifiers::NONE)]);
        assert!(action.2);
        app.submit_password();
        assert!(app.password_prompt.as_ref().unwrap().incorrect);
        assert!(app.password_prompt.as_ref().unwrap().input.is_empty());
        assert_eq!(app.viewer.as_ref().unwrap().title(), title);
        password_frame(&mut app, &ctx, vec![]);
        password_frame(&mut app, &ctx, vec![key_event(Key::Z, Modifiers::COMMAND)]);
        assert!(app.password_prompt.as_ref().unwrap().input.is_empty());
        let (action, _) = password_frame(
            &mut app,
            &ctx,
            vec![key_event(Key::Escape, Modifiers::NONE)],
        );
        assert_eq!(action, (false, false, false));
        assert!(app.password_prompt.is_none());
        assert_eq!(app.viewer.as_ref().unwrap().title(), title);
        app.open(path);
        app.password_prompt
            .as_mut()
            .unwrap()
            .input
            .push_str("open-secret");
        app.submit_password();
        assert!(app.password_prompt.is_none());
        assert!(app.open_error.is_none());
        assert_eq!(
            app.viewer.as_ref().unwrap().title(),
            "Review — locked.pdf — 1/2 — Fit page"
        );
    }

    #[test]
    fn password_is_masked_and_cancel_keeps_empty_window_open() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::with_state(persistence::Store::temporary(directory.path()), None);
        app.password_prompt = Some(PasswordPrompt {
            path: "locked.pdf".into(),
            input: Zeroizing::new("sensitive-text".into()),
            incorrect: false,
            focus: true,
            reading: None,
        });
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        password_frame(&mut app, &ctx, vec![]);
        let (_, output) = password_frame(&mut app, &ctx, vec![]);
        let tree = output.platform_output.accesskit_update.as_ref().unwrap();
        assert!(tree.nodes.iter().all(|(_, node)| {
            !node
                .value()
                .is_some_and(|value| value.contains("sensitive-text"))
        }));
        let password = &tree
            .nodes
            .iter()
            .find(|(id, _)| *id == egui::Id::new("pdf_password").accesskit_id())
            .unwrap()
            .1;
        assert_eq!(password.role(), egui::accesskit::Role::PasswordInput);
        assert!(!password.labelled_by().is_empty());
        let drawn_text: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| {
                if let egui::Shape::Text(text) = &s.shape {
                    Some(text.galley.text())
                } else {
                    None
                }
            })
            .collect();
        assert!(
            !drawn_text
                .iter()
                .any(|text| text.contains("sensitive-text"))
        );
        assert!(drawn_text.iter().any(|text| text.contains("••••")));
        let (action, _) = password_frame(
            &mut app,
            &ctx,
            vec![key_event(Key::Escape, Modifiers::NONE)],
        );
        assert_eq!(action, (false, false, false));
        assert!(app.password_prompt.is_none());
        assert!(app.viewer.is_none());
    }

    #[test]
    fn another_open_or_authentication_error_discards_pending_password() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("locked.pdf");
        document::tests::encrypted_fixture(
            &path,
            "open-secret",
            mupdf::pdf::Permission::ACCESSIBILITY,
            mupdf::pdf::Encryption::Aes256,
        );
        let mut app = App::with_state(persistence::Store::temporary(directory.path()), None);
        app.viewer = Some(Viewer::new(document::tests::sample_document()));
        app.open(path.clone());
        app.password_prompt
            .as_mut()
            .unwrap()
            .input
            .push_str("secret\0tail");
        app.submit_password();
        assert!(app.password_prompt.is_none());
        assert!(!app.open_error.as_ref().unwrap().contains("secret"));
        assert!(app.viewer.as_ref().unwrap().title().contains("sample.pdf"));
        app.open(path);
        app.password_prompt
            .as_mut()
            .unwrap()
            .input
            .push_str("unfinished");
        let replacement = directory.path().join("replacement.pdf");
        std::fs::write(&replacement, document::tests::sample_pdf("", false)).unwrap();
        app.open(replacement);
        assert!(app.password_prompt.is_none());
        assert!(app.open_error.is_none());
        assert!(
            app.viewer
                .as_ref()
                .unwrap()
                .title()
                .contains("replacement.pdf")
        );
    }

    fn frame(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> ((bool, bool), egui::FullOutput) {
        let (action, output) = password_frame(app, ctx, events);
        ((action.0, action.1), output)
    }

    fn key(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            modifiers,
            physical_key: None,
            pressed: true,
            repeat: false,
        }
    }

    #[test]
    fn help_blocks_document_commands_and_escape_never_quits() {
        let mut app = App::new();
        app.viewer = Some(Viewer::new(document::tests::sample_document()));
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![key(Key::G, Modifiers::COMMAND)]);
        let page_focus = ctx.memory(|memory| memory.focused()).unwrap();
        frame(&mut app, &ctx, vec![key(Key::F1, Modifiers::NONE)]);
        assert!(app.native_ui.help_open);
        assert_ne!(ctx.memory(|memory| memory.focused()), Some(page_focus));
        assert_eq!(
            frame(
                &mut app,
                &ctx,
                vec![
                    key(Key::O, Modifiers::COMMAND),
                    key(Key::P, Modifiers::COMMAND),
                    key(Key::ArrowRight, Modifiers::NONE)
                ]
            )
            .0,
            (false, false)
        );
        assert!(app.viewer.as_ref().unwrap().title().contains(" — 1/2 — "));
        assert!(!app.viewer.as_ref().unwrap().modal_open());
        assert_eq!(
            frame(&mut app, &ctx, vec![key(Key::Escape, Modifiers::NONE)]).0,
            (false, false)
        );
        assert!(!app.native_ui.help_open);
        // A new frame lets the modal's focus filter release the restored field.
        frame(&mut app, &ctx, vec![]);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(page_focus));
        for key_code in [Key::Escape, Key::Q] {
            assert_eq!(
                frame(&mut app, &ctx, vec![key(key_code, Modifiers::NONE)]).0,
                (false, false)
            );
        }
        for key_code in [Key::Q, Key::W] {
            assert_eq!(
                frame(&mut app, &ctx, vec![key(key_code, Modifiers::COMMAND)]).0,
                (false, true)
            );
        }
    }

    #[test]
    fn print_modal_blocks_open_until_dismissed() {
        let mut app = App::new();
        app.viewer = Some(Viewer::new(document::tests::sample_document()));
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, vec![key(Key::P, Modifiers::COMMAND)]);
        assert!(app.viewer.as_ref().unwrap().modal_open());
        assert_eq!(
            frame(&mut app, &ctx, vec![key(Key::O, Modifiers::COMMAND)]).0,
            (false, false)
        );
        assert_eq!(
            frame(&mut app, &ctx, vec![key(Key::Escape, Modifiers::NONE)]).0,
            (false, false)
        );
        assert!(!app.viewer.as_ref().unwrap().modal_open());
        assert_eq!(
            frame(&mut app, &ctx, vec![key(Key::O, Modifiers::COMMAND)]).0,
            (true, false)
        );
    }

    #[test]
    fn fields_have_names_and_page_text_has_read_only_accessible_content() {
        let mut app = App::new();
        app.viewer = Some(Viewer::new(document::tests::sample_document()));
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        frame(&mut app, &ctx, vec![key(Key::F, Modifiers::COMMAND)]);
        frame(
            &mut app,
            &ctx,
            vec![key(Key::T, Modifiers::COMMAND | Modifiers::SHIFT)],
        );
        let (_, output) = frame(&mut app, &ctx, vec![]);
        let tree = output.platform_output.accesskit_update.unwrap();
        use egui::accesskit::Role;
        for label in ["Zoom in", "Zoom out", "Page text", "Shortcut help"] {
            assert!(
                tree.nodes
                    .iter()
                    .any(|(_, node)| node.label() == Some(label)),
                "{label}"
            );
        }
        for (id, label) in [
            (egui::Id::new("page_input"), "Page"),
            (egui::Id::new("search_query"), "Find"),
            (egui::Id::new("page_text"), "Page 1 text"),
        ] {
            let node = &tree
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id.accesskit_id())
                .unwrap()
                .1;
            assert!(
                node.labelled_by().iter().any(|label_id| tree
                    .nodes
                    .iter()
                    .any(|(node_id, label_node)| node_id == label_id
                        && label_node.value() == Some(label))),
                "{label}"
            );
        }
        let text_node = &tree
            .nodes
            .iter()
            .find(|(id, _)| *id == egui::Id::new("page_text").accesskit_id())
            .unwrap()
            .1;
        assert_eq!(text_node.role(), Role::MultilineTextInput);
        assert!(text_node.is_read_only());
        let text: String = tree
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::TextRun)
            .filter_map(|(_, node)| node.value())
            .collect();
        assert!(text.contains("Alpha alpha"));
        assert!(text.contains("Needle"));
        frame(
            &mut app,
            &ctx,
            vec![egui::Event::Text("not editable".into())],
        );
        let (_, output) = frame(&mut app, &ctx, vec![]);
        assert!(
            !output
                .platform_output
                .accesskit_update
                .unwrap()
                .nodes
                .iter()
                .any(|(_, node)| node
                    .value()
                    .is_some_and(|text| text.contains("not editable")))
        );
    }

    #[test]
    fn focus_cycle_is_bidirectional_and_arrows_do_not_turn_pages_in_fields() {
        let mut app = App::new();
        app.viewer = Some(Viewer::new(document::tests::sample_document()));
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![key(Key::F, Modifiers::COMMAND)]);
        frame(
            &mut app,
            &ctx,
            vec![key(Key::T, Modifiers::COMMAND | Modifiers::SHIFT)],
        );
        for (key_code, modifiers, expected) in [
            (Key::F6, Modifiers::NONE, "page_input"),
            (Key::F6, Modifiers::NONE, "zoom_input"),
            (Key::F6, Modifiers::NONE, "search_query"),
            (Key::F6, Modifiers::NONE, "page_text"),
            (Key::F6, Modifiers::SHIFT, "search_query"),
        ] {
            frame(&mut app, &ctx, vec![key(key_code, modifiers)]);
            assert_eq!(
                ctx.memory(|memory| memory.focused()),
                Some(egui::Id::new(expected))
            );
            frame(&mut app, &ctx, vec![]); // requested repaint settles egui's focus lock
            frame(&mut app, &ctx, vec![key(Key::ArrowRight, Modifiers::NONE)]);
            assert!(app.viewer.as_ref().unwrap().title().contains(" — 1/2 — "));
        }
    }

    #[test]
    fn startup_accepts_empty_help_and_quoted_paths() {
        let parse = |args: &[&str]| parse_args(args.iter().map(OsString::from));
        assert_eq!(parse(&[]).unwrap(), Startup::Open(None));
        for flag in ["--help", "-h"] {
            assert_eq!(parse(&[flag]).unwrap(), Startup::Help);
        }
        assert_eq!(
            parse(&["papers/a résumé.pdf"]).unwrap(),
            Startup::Open(Some("papers/a résumé.pdf".into()))
        );
        assert_eq!(
            parse(&["--", "-draft.pdf"]).unwrap(),
            Startup::Open(Some("-draft.pdf".into()))
        );
        for args in [&["a.pdf", "b.pdf"][..], &["--"], &["--unknown"]] {
            assert!(parse(args).is_err());
        }
    }

    #[test]
    fn failed_open_preserves_document_and_error_is_dismissible() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::with_state(persistence::Store::temporary(directory.path()), None);
        app.viewer = Some(Viewer::new(document::tests::sample_document()));
        let location = persistence::ReadingState {
            page: 1,
            scroll: [13.0, 29.0],
            zoom: zoom::Zoom::Percent(1.25),
        };
        app.viewer.as_mut().unwrap().restore_reading(&location);
        let saved_state = app.store.state.clone();
        app.open("/missing/review-no-such-file.pdf".into());
        assert!(
            app.open_error
                .as_ref()
                .unwrap()
                .contains("failed to open PDF")
        );
        assert!(app.viewer.as_ref().unwrap().title().contains("sample.pdf"));
        let pageless = String::from_utf8(document::tests::sample_pdf("", false))
            .unwrap()
            .replace("/Kids [3 0 R 4 0 R] /Count 2", "/Kids [] /Count 0");
        for (name, bytes) in [
            ("empty.pdf", Vec::new()),
            ("invalid.pdf", b"not a PDF document".to_vec()),
            ("pageless.pdf", pageless.into_bytes()),
        ] {
            let path = directory.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            app.open(path);
            assert!(app.open_error.is_some(), "{name} must be rejected");
            assert!(app.password_prompt.is_none());
            assert_eq!(app.viewer.as_ref().unwrap().reading_state(), location);
            assert_eq!(app.store.state, saved_state);
        }
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(960.0, 720.0),
            )),
            events: vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = egui::Context::default().run_ui(input, |ui| {
            assert_eq!(
                app_ui(
                    &mut app.viewer,
                    &mut app.open_error,
                    &mut app.password_prompt,
                    ui,
                    &mut app.native_ui,
                ),
                (false, false, false)
            );
        });
        assert!(app.open_error.is_none());
    }

    #[test]
    fn successful_open_replaces_document_and_resets_view() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::with_state(persistence::Store::temporary(directory.path()), None);
        app.viewer = Some(Viewer::new(document::tests::sample_document()));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(960.0, 720.0),
            )),
            events: [Key::ArrowRight, Key::Num1, Key::Plus]
                .map(|key| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                })
                .to_vec(),
            ..Default::default()
        };
        let _ = egui::Context::default().run_ui(input, |ui| {
            app_ui(
                &mut app.viewer,
                &mut app.open_error,
                &mut app.password_prompt,
                ui,
                &mut app.native_ui,
            );
        });
        assert!(app.viewer.as_ref().unwrap().title().ends_with("2/2 — 125%"));
        let path = directory.path().join("new document.pdf");
        std::fs::write(&path, document::tests::sample_pdf("", false)).unwrap();
        app.open_error = Some("previous error".into());
        app.open(path);
        assert_eq!(
            app.viewer.as_ref().unwrap().title(),
            "Review — new document.pdf — 1/2 — Fit page"
        );
        assert!(app.open_error.is_none());
    }

    #[test]
    fn app_restart_replacement_bookmarks_and_clear_history() {
        use persistence::{Bookmark, ReadingState, SidebarState, Store};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("first.pdf");
        let second = directory.path().join("second.pdf");
        for path in [&path, &second] {
            std::fs::write(path, document::tests::sample_pdf("", false)).unwrap();
        }
        let location = ReadingState {
            page: 1,
            scroll: [73.0, 129.0],
            zoom: zoom::Zoom::Percent(2.75),
        };
        let mut app = App::with_state(Store::temporary(directory.path()), None);
        app.open(path.clone());
        app.viewer.as_mut().unwrap().restore_reading(&location);
        app.viewer.as_mut().unwrap().restore_sidebar(&SidebarState {
            open: false,
            width: 317.0,
            pages: true,
        });
        app.store
            .state
            .toggle_bookmark(persistence::file_key(&path), location.clone());
        app.open(second.clone());
        assert_eq!(
            app.store.state.reading(&persistence::file_key(&path)),
            Some(&location)
        );
        app.save_state(true);
        drop(app);
        let mut app = App::with_state(Store::temporary(directory.path()), None);
        assert!(app.viewer.is_none()); // No automatic document reopen.
        app.open(path.clone());
        assert_eq!(app.viewer.as_ref().unwrap().reading_state(), location);
        assert_eq!(
            app.viewer.as_ref().unwrap().sidebar_state(),
            SidebarState {
                open: false,
                width: 317.0,
                pages: true
            }
        );
        app.open(second);
        app.open_bookmark(app.store.state.bookmarks[0].clone());
        assert_eq!(app.viewer.as_ref().unwrap().reading_state(), location);
        app.open_bookmark(Bookmark {
            path: directory.path().join("missing.pdf"),
            reading: ReadingState::default(),
        });
        assert_eq!(app.viewer.as_ref().unwrap().reading_state(), location);
        assert_eq!(app.store.state.recent.len(), 2);
        app.store.state.clear_history();
        app.save_state(true);
        assert!(app.store.state.recent.is_empty());
        let mut app = App::with_state(Store::temporary(directory.path()), None);
        app.open(path);
        assert_eq!(
            app.viewer.as_ref().unwrap().reading_state(),
            ReadingState::default()
        );
        assert_eq!(app.store.state.bookmarks.len(), 1);
    }

    #[test]
    fn encrypted_recent_and_bookmark_restore_only_after_authentication() {
        use persistence::{ReadingState, Store};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("locked.pdf");
        document::tests::encrypted_fixture(
            &path,
            "open-secret",
            mupdf::pdf::Permission::ACCESSIBILITY,
            mupdf::pdf::Encryption::Aes256,
        );
        let plain = directory.path().join("plain.pdf");
        std::fs::write(&plain, document::tests::sample_pdf("", false)).unwrap();
        let recent = ReadingState {
            page: 1,
            scroll: [49.0, 213.0],
            zoom: zoom::Zoom::Percent(3.75),
        };
        let bookmark = ReadingState {
            page: 0,
            scroll: [17.0, 87.0],
            zoom: zoom::Zoom::Percent(2.25),
        };
        let mut store = Store::temporary(directory.path());
        store
            .state
            .opened(persistence::file_key(&path), recent.clone());
        store
            .state
            .toggle_bookmark(persistence::file_key(&path), bookmark.clone());
        store.save(true).unwrap();
        let mut app = App::with_state(Store::temporary(directory.path()), None);
        app.open(plain.clone());
        let preserved = app.store.state.clone();
        app.open(path.clone()); // The recent-file route always opens without credentials.
        assert!(app.password_prompt.is_some());
        assert_eq!(app.store.state, preserved);
        assert_eq!(
            app.viewer.as_ref().unwrap().reading_state(),
            ReadingState::default()
        );
        app.password_prompt
            .as_mut()
            .unwrap()
            .input
            .push_str("wrong-secret");
        app.submit_password();
        assert_eq!(app.store.state, preserved);
        let ctx = egui::Context::default();
        password_frame(
            &mut app,
            &ctx,
            vec![key_event(Key::Escape, Modifiers::NONE)],
        );
        assert!(app.password_prompt.is_none());
        assert_eq!(app.store.state, preserved);
        app.open(path.clone());
        app.password_prompt
            .as_mut()
            .unwrap()
            .input
            .push_str("open-secret");
        app.submit_password();
        assert_eq!(app.viewer.as_ref().unwrap().reading_state(), recent);
        app.open(plain);
        app.open_bookmark(app.store.state.bookmarks[0].clone());
        assert!(app.password_prompt.is_some());
        assert_eq!(
            app.viewer.as_ref().unwrap().reading_state(),
            ReadingState::default()
        );
        app.password_prompt
            .as_mut()
            .unwrap()
            .input
            .push_str("wrong-secret");
        app.submit_password();
        assert!(app.password_prompt.as_ref().unwrap().incorrect);
        app.password_prompt
            .as_mut()
            .unwrap()
            .input
            .push_str("open-secret");
        app.submit_password();
        assert_eq!(app.viewer.as_ref().unwrap().reading_state(), bookmark);
        app.save_state(true);
        let json = std::fs::read_to_string(directory.path().join("state.json")).unwrap();
        assert!(!json.contains("open-secret"));
        assert!(!json.contains("wrong-secret"));
        assert!(!json.contains("password"));
        drop(app);
        let mut app = App::with_state(Store::temporary(directory.path()), None);
        app.open(path);
        assert!(app.password_prompt.is_some());
        assert!(app.viewer.is_none());
    }

    #[test]
    fn open_shortcut_works_with_and_without_a_document() {
        for viewer in [None, Some(Viewer::new(document::tests::sample_document()))] {
            let mut viewer = viewer;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 720.0),
                )),
                events: vec![egui::Event::Key {
                    key: Key::O,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::COMMAND,
                }],
                ..Default::default()
            };
            let _ = egui::Context::default().run_ui(input, |ui| {
                assert_eq!(
                    app_ui(
                        &mut viewer,
                        &mut None,
                        &mut None,
                        ui,
                        &mut native_ui::NativeUi::default()
                    ),
                    (true, false, false)
                );
            });
        }
    }
}
