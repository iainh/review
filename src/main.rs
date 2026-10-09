#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod annotations;
mod desktop;
mod document;
mod fonts;
mod forms;
mod icons;
mod inspection;
mod inspector;
mod layout;
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
mod reading;
mod render_worker;
mod renderer;
mod search;
mod selection;
mod sidebar;
mod structured_text;
mod tabs;
mod viewer;
mod zoom;

use std::{collections::VecDeque, env, ffi::OsString, path::PathBuf, sync::Arc, time::Instant};

use anyhow::{Context, Result, bail};
use egui::{Key, Modifiers};
use renderer::Renderer;
use viewer::Viewer;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    window::{Icon, Window, WindowId},
};
use zeroize::{Zeroize, Zeroizing};

fn app_icon() -> Icon {
    let image = image::load_from_memory(include_bytes!("../assets/review-256.png"))
        .expect("bundled application icon must be a valid PNG")
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height)
        .expect("bundled application icon must contain valid RGBA pixels")
}

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
    #[cfg(target_os = "macos")]
    Menu(desktop::Action),
    Repaint,
    AccessKit(egui_winit::accesskit_winit::Event),
}

impl From<egui_winit::accesskit_winit::Event> for AppEvent {
    fn from(event: egui_winit::accesskit_winit::Event) -> Self {
        Self::AccessKit(event)
    }
}

enum PendingAction {
    CloseTab(u64),
    CloseWindow(VecDeque<u64>),
}

#[derive(Clone, Copy)]
enum UnsavedDecision {
    Save(bool),
    Discard,
    Cancel,
}

struct App {
    viewer: Option<Viewer>,
    tabs: tabs::Tabs,
    session_cleared: bool,
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
    desktop: desktop::Desktop,
    pending: Option<PendingAction>,
    save_failed: bool,
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
            tabs: tabs::Tabs::default(),
            session_cleared: false,
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
            desktop: desktop::Desktop::default(),
            pending: None,
            save_failed: false,
        }
    }

    fn open(&mut self, path: PathBuf) {
        self.open_with_reading(path, None);
    }

    fn open_with_reading(&mut self, path: PathBuf, reading: Option<persistence::ReadingState>) {
        // Opening adds a tab, retaining unsaved documents. Do not replace a
        // pending close target when an OS open/drop arrives during confirmation.
        if self.pending.is_some() {
            return;
        }
        self.open_unchecked(path, reading);
    }

    fn open_unchecked(&mut self, path: PathBuf, reading: Option<persistence::ReadingState>) {
        self.password_prompt = None;
        self.open_error = None;
        let key = persistence::file_key(&path);
        if let Some(id) = self.tabs.find(&key)
            && self.tabs.select(id, &mut self.viewer)
        {
            self.session_cleared = false;
            let viewer = self.viewer.as_mut().unwrap();
            if let Some(reading) = &reading {
                viewer.restore_reading(reading);
            }
            self.store.state.opened(key, viewer.reading_state());
            self.tab_changed();
            return;
        }
        if self.tabs.find(&key).is_none() && self.tabs.len() >= persistence::MAX_SESSION {
            self.open_error = Some(
                "The 16-document limit is reached. Close a tab before opening another PDF.".into(),
            );
            self.tab_changed();
            return;
        }
        match document::PdfDocument::open_with_password(&path, None) {
            Ok(Some(document)) => self.finish_open(document, reading.as_ref()),
            Ok(None) => {
                self.password_prompt = Some(PasswordPrompt {
                    path,
                    input: Zeroizing::new(String::new()),
                    incorrect: false,
                    focus: true,
                    reading,
                })
            }
            Err(error) => self.open_error = Some(format!("{error:#}")),
        }
        if let Some(renderer) = &self.renderer {
            renderer.window().request_redraw();
        }
    }

    fn request_close(&mut self, event_loop: &ActiveEventLoop) {
        if self.begin_close() {
            event_loop.exit();
        }
    }

    fn begin_close(&mut self) -> bool {
        if self.pending.is_some() {
            return false;
        }
        self.password_prompt = None;
        self.capture_state();
        let dirty: VecDeque<_> = self.tabs.dirty_ids(self.viewer.as_ref()).into();
        let Some(&first) = dirty.front() else {
            return true;
        };
        self.select_tab(first);
        self.pending = Some(PendingAction::CloseWindow(dirty));
        self.save_failed = false;
        false
    }

    /// Return whether the caller should exit. Save failure retains the target.
    /// Window-close decisions never drop documents until every prompt succeeds,
    /// so cancelling a later prompt also retains earlier discarded buffers.
    fn resolve_unsaved(&mut self, decision: UnsavedDecision) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        self.save_failed = false;
        match decision {
            UnsavedDecision::Cancel => return false,
            UnsavedDecision::Save(save_as) => {
                if !self.save_current(save_as) {
                    self.pending = Some(pending);
                    self.save_failed = true;
                    return false;
                }
            }
            UnsavedDecision::Discard => {}
        }
        match pending {
            PendingAction::CloseTab(id) => {
                self.close_tab_unchecked(id);
                false
            }
            PendingAction::CloseWindow(mut remaining) => {
                remaining.pop_front();
                if let Some(&next) = remaining.front() {
                    self.select_tab(next);
                    self.pending = Some(PendingAction::CloseWindow(remaining));
                    false
                } else {
                    true
                }
            }
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
        let restored = self
            .tabs
            .find(viewer.state_key())
            .and_then(|id| self.tabs.file(id));
        viewer.restore_sidebar(restored.map_or(&self.store.state.sidebar, |file| &file.sidebar));
        if let Some(reading) = bookmark
            .or_else(|| restored.map(|file| &file.reading))
            .or_else(|| self.store.state.reading(viewer.state_key()))
        {
            viewer.restore_reading(reading);
        }
        self.store
            .state
            .opened(viewer.state_key().to_path_buf(), viewer.reading_state());
        self.tabs.opened(viewer, &mut self.viewer);
        self.session_cleared = false;
        self.tab_changed();
        self.open_error = None;
    }

    fn tab_changed(&self) {
        if let Some(renderer) = &self.renderer {
            renderer.context.memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
            renderer.window().request_redraw();
        }
    }

    fn select_tab(&mut self, id: u64) {
        if self.tabs.select(id, &mut self.viewer) {
            self.open_error = None;
            self.tab_changed();
        } else if let Some(file) = self.tabs.file(id) {
            self.open(file.path.clone());
        }
    }

    fn close_tab(&mut self, id: u64) {
        if self.pending.is_some() {
            return;
        }
        if self.tabs.is_dirty(id, self.viewer.as_ref()) {
            self.select_tab(id);
            self.pending = Some(PendingAction::CloseTab(id));
            self.save_failed = false;
        } else {
            self.close_tab_unchecked(id);
        }
    }

    fn close_tab_unchecked(&mut self, id: u64) {
        self.capture_state();
        if let Some(neighbour) = self.tabs.close(id, &mut self.viewer) {
            self.select_tab(neighbour);
        }
        self.tab_changed();
    }

    fn restore_session(&mut self) {
        if self.store.state.restore_session {
            self.tabs.restore(&self.store.state.session);
            if let Some(id) = self.tabs.active_id() {
                self.select_tab(id);
            }
        }
    }

    fn open_bookmark(&mut self, bookmark: persistence::Bookmark) {
        if let Some(viewer) = &mut self.viewer
            && viewer.state_key() == bookmark.path
        {
            viewer.restore_reading(&bookmark.reading);
        } else {
            self.open_with_reading(bookmark.path, Some(bookmark.reading));
        }
    }

    fn save_current(&mut self, save_as: bool) -> bool {
        self.capture_state();
        let session = self.tabs.session();
        let open_paths: Vec<_> = session
            .files
            .into_iter()
            .enumerate()
            .filter(|(index, _)| *index != session.active)
            .map(|(_, file)| file.path)
            .collect();
        let Some(viewer) = &mut self.viewer else {
            return false;
        };
        let saved = if open_paths.is_empty() {
            viewer.save(save_as)
        } else {
            viewer.save_with_open_paths(save_as, &open_paths)
        };
        if !saved {
            return false;
        }
        self.store
            .state
            .opened(viewer.state_key().to_path_buf(), viewer.reading_state());
        self.capture_state();
        true
    }

    fn capture_state(&mut self) {
        self.store.state.appearance = self.native_ui.appearance;
        self.tabs.capture(self.viewer.as_ref());
        let session = self.tabs.session();
        for file in &session.files {
            self.store
                .state
                .update_reading(&file.path, file.reading.clone());
        }
        if self.store.state.restore_session && !self.session_cleared {
            self.store.state.session = session;
        } else {
            self.store.state.session = persistence::Session::default();
        }
        if let Some(viewer) = &self.viewer {
            self.store
                .state
                .update_reading(viewer.state_key(), viewer.reading_state());
            self.store.state.sidebar = viewer.sidebar_state();
        }
        if let Some(renderer) = &self.renderer {
            let window = renderer.window();
            let maximized = renderer.is_maximized();
            self.store.state.window.maximized = maximized;
            if !maximized && window.fullscreen().is_none() && window.is_minimized() != Some(true) {
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
            #[cfg(target_os = "macos")]
            AppEvent::Menu(action) => {
                self.desktop.enqueue(action);
                if let Some(renderer) = &self.renderer {
                    renderer.window().request_redraw();
                }
            }
            AppEvent::Repaint => {
                if let Some(renderer) = &self.renderer {
                    renderer.window().request_redraw();
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
            .with_window_icon(Some(app_icon()))
            .with_visible(false)
            .with_decorations(false)
            // AppKit must leave the pixels clipped by the content layer clear.
            .with_transparent(cfg!(target_os = "macos"))
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
        #[cfg(target_os = "windows")]
        let attributes = {
            use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
            // DWM chooses the radius and suppresses rounding when maximized/snapped.
            attributes.with_corner_preference(CornerPreference::Round)
        };
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
                geometry.maximized,
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
            WindowEvent::CloseRequested => self.request_close(event_loop),
            WindowEvent::Resized(size) => renderer.resize(size),
            WindowEvent::DroppedFile(path) => self.open(path),
            WindowEvent::RedrawRequested => {
                let input = renderer.take_input();
                let mut open_requested = false;
                let mut quit = false;
                let mut submit_password = false;
                let mut library_action = None;
                let mut tab_action = None;
                let mut desktop_action = None;
                let mut recent_requested = None;
                let mut bookmark_requested = None;
                let mut menu_active = false;
                let unsaved_active = self.pending.is_some();
                let mut unsaved_decision = None;
                let mut output = renderer.context.run_ui(input, |ui| {
                    let ctx = ui.ctx().clone();
                    if unsaved_active {
                        unsaved_decision = unsaved_ui(&ctx, self.save_failed);
                    }
                    let events =
                        unsaved_active.then(|| ctx.input_mut(|i| std::mem::take(&mut i.events)));
                    ui.add_enabled_ui(!unsaved_active, |ui| {
                        let library_enabled = !unsaved_active
                            && self.password_prompt.is_none()
                            && !self.native_ui.help_open
                            && !self.viewer.as_ref().is_some_and(Viewer::modal_open)
                            && !ui.input(|input| input.key_pressed(Key::F1));
                        self.desktop
                            .sync_library(&self.store.state, self.viewer.as_ref());
                        #[cfg(target_os = "macos")]
                        macos::sync_menu(
                            &self.store.state,
                            self.viewer.as_ref(),
                            self.native_ui.appearance,
                            self.viewer.as_ref().is_some_and(Viewer::can_print),
                            !library_enabled,
                        );
                        let (action, owns_input) = self.desktop.show(
                            ui,
                            self.viewer.is_some(),
                            self.viewer.as_ref().is_some_and(Viewer::can_print),
                            self.native_ui.appearance,
                            !library_enabled,
                        );
                        desktop_action = action.or(desktop_action.take());
                        menu_active |= owns_input;
                        // Match modal input isolation: menu arrows and clicks
                        // must not also navigate the PDF or change tabs.
                        let menu_events =
                            menu_active.then(|| ctx.input_mut(|i| std::mem::take(&mut i.events)));
                        let content_opacity = ui.opacity();
                        ui.add_enabled_ui(!menu_active, |ui| {
                            // Menus lock input without dimming the document like a modal.
                            if menu_active {
                                ui.set_opacity(content_opacity);
                            }
                            tab_action = self.tabs.ui(
                                ui,
                                library_enabled && !menu_active,
                                self.viewer.as_ref(),
                            );
                            if self.library.ui(
                                ui,
                                &mut self.store.state,
                                self.viewer.as_ref(),
                                library_enabled && !menu_active,
                            ) {
                                library_action = Some(());
                            }
                            let id = self.tabs.active_id().unwrap_or(0);
                            ui.push_id(("document", id), |ui| {
                                (open_requested, quit, submit_password) = app_ui(
                                    &mut self.viewer,
                                    &mut self.open_error,
                                    &mut self.password_prompt,
                                    ui,
                                    &mut self.native_ui,
                                );
                            });
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
                        if let Some(events) = menu_events {
                            ctx.input_mut(|i| i.events = events);
                        }
                    });
                    if let Some(events) = events {
                        ctx.input_mut(|i| i.events = events);
                    }
                });
                quit |= desktop::close_requested(&output);
                #[cfg(target_os = "macos")]
                let had_desktop_action = desktop_action.is_some();
                match desktop_action {
                    Some(desktop::Action::Open) => open_requested = true,
                    Some(desktop::Action::OpenRecent(path)) => recent_requested = Some(path),
                    Some(desktop::Action::OpenBookmark(bookmark)) => {
                        bookmark_requested = Some(bookmark);
                    }
                    Some(desktop::Action::ToggleBookmark) => {
                        if let Some(viewer) = &self.viewer {
                            self.store.state.toggle_bookmark(
                                viewer.state_key().to_path_buf(),
                                viewer.reading_state(),
                            );
                        }
                    }
                    Some(desktop::Action::RemoveBookmark(index)) => {
                        if index < self.store.state.bookmarks.len() {
                            self.store.state.bookmarks.remove(index);
                        }
                    }
                    Some(desktop::Action::ClearHistory) => {
                        self.library.confirm_clear();
                        renderer.window().request_redraw();
                    }
                    Some(desktop::Action::RestoreSession(enabled)) => {
                        self.store.state.restore_session = enabled;
                        self.session_cleared = false;
                        force_save = true;
                    }
                    Some(desktop::Action::Save(save_as)) => {
                        if let Some(viewer) = &mut self.viewer {
                            viewer.save_requested = Some(save_as);
                        }
                    }
                    Some(desktop::Action::Print) => {
                        if let Some(viewer) = &mut self.viewer {
                            viewer.request_print();
                        }
                    }
                    Some(desktop::Action::CloseTab) => {
                        tab_action = self.tabs.active_id().map(tabs::Action::Close);
                    }
                    Some(desktop::Action::Quit) => quit = true,
                    Some(desktop::Action::Appearance(appearance)) => {
                        self.native_ui.appearance = appearance;
                        appearance.apply(&renderer.context);
                    }
                    Some(desktop::Action::Fullscreen) => {
                        output
                            .viewport_output
                            .get_mut(&egui::ViewportId::ROOT)
                            .unwrap()
                            .commands
                            .push(egui::ViewportCommand::Fullscreen(
                                renderer.window().fullscreen().is_none(),
                            ));
                    }
                    Some(desktop::Action::Help) => self.native_ui.open_help(&renderer.context),
                    None => {}
                }
                #[cfg(target_os = "macos")]
                if had_desktop_action {
                    // Native menu state is synchronized before dispatch. Repaint
                    // once more so checks and enabled items reflect this action.
                    renderer.window().request_redraw();
                }
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
                if let Some(save_as) = self.viewer.as_mut().and_then(|v| v.save_requested.take()) {
                    self.save_current(save_as);
                    if let Some(renderer) = &self.renderer {
                        renderer.window().request_redraw();
                    }
                }
                if let Some(decision) = unsaved_decision {
                    if self.resolve_unsaved(decision) {
                        force_save = true;
                        event_loop.exit();
                    }
                    if let Some(renderer) = &self.renderer {
                        renderer.window().request_redraw();
                    }
                } else if quit && !unsaved_active {
                    force_save = true;
                    self.request_close(event_loop);
                } else if submit_password {
                    self.submit_password();
                } else if let Some(bookmark) = bookmark_requested {
                    self.open_bookmark(bookmark);
                } else if let Some(path) = recent_requested {
                    self.open(path);
                } else if open_requested {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Open PDF")
                        .add_filter("PDF documents", &["pdf"]);
                    // rfd's Linux parent export borrows raw Wayland pointers
                    // across threads. Avoid that path while winit is blocked.
                    #[cfg(not(target_os = "linux"))]
                    {
                        dialog = dialog.set_parent(self.renderer.as_ref().unwrap().window());
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
                    let dialog = dialog.set_parent(self.renderer.as_ref().unwrap().window());
                    if let Some(path) = dialog.save_file() {
                        viewer.save_attachment(index, &path);
                    }
                    self.renderer.as_ref().unwrap().window().request_redraw();
                }
                if let Some(()) = library_action {
                    self.store.state.clear_history();
                    self.session_cleared = true;
                    force_save = true;
                }
                match tab_action {
                    Some(tabs::Action::Select(id)) => self.select_tab(id),
                    Some(tabs::Action::Close(id)) => self.close_tab(id),
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
        // Capture Enter before the text edit consumes it. Submission belongs
        // to the password modal and must also survive a layout re-pass.
        let enter_key = ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
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
                submit_password = ui.button("Open PDF").clicked() || enter_key;
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
    // Ctrl/Cmd+W belongs to the tab strip, not the window-level quit command.
    let quit = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::Q));
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
        let frame = egui::Frame::central_panel(root.style()).fill(root.visuals().faint_bg_color);
        egui::CentralPanel::default()
            .frame(frame)
            .show_inside(root, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(((ui.available_height() - 190.0) * 0.5).max(0.0));
                    ui.label(
                        egui::RichText::new(char::from(icons::Icon::FileText).to_string())
                            .size(40.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(14.0);
                    ui.label(egui::RichText::new("No document open").size(20.0));
                    ui.add_space(8.0);
                    ui.weak("Open a PDF, or drop one anywhere in this window.");
                    ui.add_space(16.0);
                    if native_ui::studio_button(
                        ui,
                        egui::Button::new("Open…")
                            .selected(true)
                            .min_size(egui::vec2(110.0, 30.0)),
                        true,
                    )
                    .clicked()
                    {
                        open_requested = true;
                    }
                    ui.add_space(8.0);
                    ui.weak("Ctrl+O / Cmd+O");
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

fn unsaved_ui(ctx: &egui::Context, save_failed: bool) -> Option<UnsavedDecision> {
    let mut decision = None;
    egui::Modal::new(egui::Id::new("unsaved_changes")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.heading("Save changes before continuing?");
        ui.label(
            "This PDF has unsaved changes. Discarding cannot be undone after the document closes.",
        );
        if save_failed {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                "The PDF was not saved. Cancel to review the error, or choose Save As.",
            );
        }
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                decision = Some(UnsavedDecision::Save(false));
            }
            if ui.button("Save As…").clicked() {
                decision = Some(UnsavedDecision::Save(true));
            }
            if ui.button("Discard").clicked() {
                decision = Some(UnsavedDecision::Discard);
            }
            if ui.button("Cancel").clicked()
                || ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
            {
                decision = Some(UnsavedDecision::Cancel);
            }
        });
    });
    decision
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
        bail!("expected at most one startup PDF; open additional documents in the window");
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
    macos::install(event_loop.create_proxy());
    let mut app = App::new();
    app.proxy = Some(event_loop.create_proxy());
    if let Some(path) = path {
        app.open(path);
    } else {
        app.restore_session();
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

    #[test]
    #[ignore = "reopens native tab-close output from REVIEW_FIXTURE_DIR"]
    fn verify_native_tab_close_output() {
        let directory = PathBuf::from(env::var_os("REVIEW_FIXTURE_DIR").unwrap());
        for (name, contents) in [
            ("annotations.pdf", "first unsaved tab"),
            ("second-annotations.pdf", "Existing note"),
        ] {
            let document = document::PdfDocument::open(directory.join(name)).unwrap();
            let annotations = document.annotations(0).unwrap();
            assert_eq!(annotations.len(), 1);
            assert_eq!(annotations[0].contents, contents);
        }
    }

    #[test]
    fn dirty_inactive_tab_close_requires_a_successful_save_or_discard() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let source = root.join("source.pdf");
        let replacement = root.join("replacement.pdf");
        std::fs::write(&source, document::tests::sample_pdf("", false)).unwrap();
        std::fs::write(&replacement, document::tests::sample_pdf("", false)).unwrap();
        let mut document = document::PdfDocument::open(&source).unwrap();
        document
            .add_annotation(
                0,
                annotations::Kind::Note,
                annotations::Geometry::Note([0.3, 0.7]),
                "keep this",
                [0.9, 0.2, 0.1],
                2.0,
            )
            .unwrap();
        let mut app = App::with_state(persistence::Store::temporary(directory.path()), None);
        app.viewer = Some(Viewer::new(document));
        app.open(replacement.clone());
        assert!(app.pending.is_none());
        assert_eq!(app.tabs.len(), 2);
        let source_id = app.tabs.find(&source).unwrap();
        app.close_tab(source_id);
        assert!(matches!(app.pending, Some(PendingAction::CloseTab(id)) if id == source_id));
        assert_eq!(app.viewer.as_ref().unwrap().path(), source);
        assert!(!app.resolve_unsaved(UnsavedDecision::Cancel));
        assert!(app.pending.is_none());
        assert!(app.viewer.as_ref().unwrap().is_dirty());
        app.open(replacement.clone());
        app.close_tab(source_id);
        let original_permissions = std::fs::metadata(&source).unwrap().permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        std::fs::set_permissions(&source, permissions).unwrap();
        assert!(!app.resolve_unsaved(UnsavedDecision::Save(false)));
        assert!(app.pending.is_some());
        assert!(app.viewer.as_ref().unwrap().is_dirty());
        assert_eq!(app.viewer.as_ref().unwrap().path(), source);
        std::fs::set_permissions(&source, original_permissions).unwrap();
        assert!(!app.resolve_unsaved(UnsavedDecision::Save(false)));
        assert_eq!(app.viewer.as_ref().unwrap().path(), replacement);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            document::PdfDocument::open(&source)
                .unwrap()
                .annotations(0)
                .unwrap()[0]
                .contents,
            "keep this"
        );
        assert!(app.begin_close());
    }

    #[test]
    fn dirty_bookmark_open_keeps_source_and_destination_through_password() {
        for encrypted in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().canonicalize().unwrap();
            let source = root.join("source.pdf");
            let target = root.join("target.pdf");
            std::fs::write(&source, document::tests::sample_pdf("", false)).unwrap();
            if encrypted {
                document::tests::encrypted_fixture(
                    &target,
                    "open-secret",
                    mupdf::pdf::Permission::ANNOTATE,
                    mupdf::pdf::Encryption::Aes256,
                );
            } else {
                std::fs::write(&target, document::tests::sample_pdf("", false)).unwrap();
            }
            let mut document = document::PdfDocument::open(&source).unwrap();
            document
                .add_annotation(
                    0,
                    annotations::Kind::Note,
                    annotations::Geometry::Note([0.25, 0.71]),
                    "unsaved",
                    [0.3, 0.7, 0.9],
                    2.0,
                )
                .unwrap();
            let mut app = App::with_state(persistence::Store::temporary(directory.path()), None);
            app.viewer = Some(Viewer::new(document));
            let reading = persistence::ReadingState {
                page: 1,
                scroll: [17.0, 93.0],
                zoom: zoom::Zoom::Percent(2.0),
                ..Default::default()
            };
            let bookmark = persistence::Bookmark {
                path: target.clone(),
                reading: reading.clone(),
            };
            app.open_bookmark(bookmark.clone());
            assert!(app.pending.is_none());
            if encrypted {
                assert_eq!(app.viewer.as_ref().unwrap().path(), source);
                assert!(app.viewer.as_ref().unwrap().is_dirty());
                assert_eq!(
                    app.password_prompt.as_ref().unwrap().reading.as_ref(),
                    Some(&reading)
                );
                app.password_prompt
                    .as_mut()
                    .unwrap()
                    .input
                    .push_str("open-secret");
                app.submit_password();
            }
            let viewer = app.viewer.as_ref().unwrap();
            assert_eq!(viewer.path(), target);
            assert_eq!(viewer.reading_state(), reading);
            app.select_tab(app.tabs.find(&source).unwrap());
            assert!(app.viewer.as_ref().unwrap().is_dirty());
            // A bookmark for a live inactive tab must apply its destination,
            // rather than just focusing that tab's last reading position.
            let mut bookmark = bookmark;
            bookmark.reading.page = 0;
            app.open_bookmark(bookmark.clone());
            assert_eq!(
                app.viewer.as_ref().unwrap().reading_state(),
                bookmark.reading
            );
            assert!(
                document::PdfDocument::open(&source)
                    .unwrap()
                    .annotations(0)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn window_close_queue_preserves_discarded_buffers_on_cancel_and_saves_each_target() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::with_state(persistence::Store::temporary(directory.path()), None);
        let mut ids = Vec::new();
        for name in ["first.pdf", "clean.pdf", "last.pdf"] {
            let path = directory.path().join(name);
            std::fs::write(&path, document::tests::sample_pdf("", false)).unwrap();
            let mut document = document::PdfDocument::open(&path).unwrap();
            if name != "clean.pdf" {
                document
                    .add_annotation(
                        0,
                        annotations::Kind::Note,
                        annotations::Geometry::Note([0.2, 0.7]),
                        name,
                        [0.8, 0.3, 0.1],
                        2.0,
                    )
                    .unwrap();
            }
            app.finish_open(document, None);
            ids.push(app.tabs.active_id().unwrap());
        }
        app.password_prompt = Some(PasswordPrompt {
            path: directory.path().join("locked.pdf"),
            input: Zeroizing::new("session-only".into()),
            incorrect: false,
            focus: true,
            reading: None,
        });
        assert!(!app.begin_close());
        assert!(app.password_prompt.is_none());
        assert_eq!(app.tabs.active_id(), Some(ids[0]));
        assert!(!app.begin_close()); // repeated native close cannot reset the queue
        app.open(directory.path().join("ignored-open.pdf"));
        app.close_tab(ids[1]);
        assert_eq!(app.tabs.len(), 3);
        assert!(!app.resolve_unsaved(UnsavedDecision::Discard));
        assert_eq!(app.tabs.active_id(), Some(ids[2]));
        assert!(!app.resolve_unsaved(UnsavedDecision::Cancel));
        assert!(app.pending.is_none());
        assert_eq!(
            app.tabs.dirty_ids(app.viewer.as_ref()),
            vec![ids[0], ids[2]]
        );
        assert_eq!(app.tabs.len(), 3);
        assert!(!app.begin_close());
        assert!(!app.resolve_unsaved(UnsavedDecision::Save(false)));
        assert_eq!(app.tabs.active_id(), Some(ids[2]));
        assert_eq!(app.tabs.dirty_ids(app.viewer.as_ref()), vec![ids[2]]);
        assert!(app.resolve_unsaved(UnsavedDecision::Save(false)));
        assert!(app.tabs.dirty_ids(app.viewer.as_ref()).is_empty());
        for name in ["first.pdf", "last.pdf"] {
            assert_eq!(
                document::PdfDocument::open(directory.path().join(name))
                    .unwrap()
                    .annotations(0)
                    .unwrap()[0]
                    .contents,
                name
            );
        }
    }

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
        assert_eq!(
            frame(&mut app, &ctx, vec![key(Key::Q, Modifiers::COMMAND)]).0,
            (false, true)
        );
        assert_eq!(
            frame(&mut app, &ctx, vec![key(Key::W, Modifiers::COMMAND)]).0,
            (false, false)
        );
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
        for label in ["Zoom in", "Zoom out", "Close page text"] {
            assert!(
                tree.nodes
                    .iter()
                    .any(|(_, node)| node.label() == Some(label)),
                "{label}"
            );
        }
        let viewer = app.viewer.as_ref().unwrap();
        for (id, label) in [
            (viewer.field_id("page_input"), "Page"),
            (viewer.field_id("search_query"), "Find"),
            (viewer.field_id("page_text"), "Page 1 text"),
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
            .find(|(id, _)| *id == viewer.field_id("page_text").accesskit_id())
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
                Some(app.viewer.as_ref().unwrap().field_id(expected))
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        };
        let bookmark = ReadingState {
            page: 0,
            scroll: [17.0, 87.0],
            zoom: zoom::Zoom::Percent(2.25),
            ..Default::default()
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
        // Closing drops its session-only credentials. Focusing an already-open
        // unlocked tab does not reopen/authenticate the same document.
        app.close_tab(app.tabs.active_id().unwrap());
        app.open(plain);
        let plain_reading = app.viewer.as_ref().unwrap().reading_state();
        app.open_bookmark(app.store.state.bookmarks[0].clone());
        assert!(app.password_prompt.is_some());
        assert_eq!(app.viewer.as_ref().unwrap().reading_state(), plain_reading);
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
    fn tabs_keep_independent_views_deduplicate_and_close_by_identity() {
        use persistence::{ReadingState, SidebarState, Store};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let paths: Vec<_> = ["first.pdf", "second.pdf", "third.pdf"]
            .map(|name| {
                let path = root.join(name);
                std::fs::write(&path, document::tests::sample_pdf("", false)).unwrap();
                path
            })
            .into();
        let mut app = App::with_state(Store::temporary(directory.path()), None);
        app.open(paths[0].clone());
        let first = app.tabs.active_id().unwrap();
        let location = ReadingState {
            page: 1,
            scroll: [-37.0, 193.0],
            zoom: zoom::Zoom::Percent(3.25),
            layout: layout::LayoutMode::Facing,
            rotation: layout::Rotation::Counterclockwise,
        };
        let sidebar = SidebarState {
            open: false,
            width: 311.0,
            pages: true,
        };
        app.viewer.as_mut().unwrap().restore_reading(&location);
        app.viewer.as_mut().unwrap().restore_sidebar(&sidebar);
        app.open(paths[1].clone());
        let second = app.tabs.active_id().unwrap();
        app.viewer.as_mut().unwrap().restore_reading(&ReadingState {
            page: 0,
            scroll: [4.0, 7.0],
            zoom: zoom::Zoom::FitWidth,
            layout: layout::LayoutMode::Continuous,
            rotation: layout::Rotation::Clockwise,
        });
        app.open(paths[2].clone());
        app.open(paths[0].clone());
        assert_eq!(app.tabs.len(), 3);
        assert_eq!(app.tabs.active_id(), Some(first));
        assert_eq!(app.viewer.as_ref().unwrap().reading_state(), location);
        assert_eq!(app.viewer.as_ref().unwrap().sidebar_state(), sidebar);
        app.close_tab(second); // Inactive close must not change the selected Viewer.
        assert_eq!(app.tabs.active_id(), Some(first));
        assert_eq!(app.tabs.len(), 2);
        app.close_tab(first);
        assert_eq!(app.viewer.as_ref().unwrap().path(), paths[2]);
        app.close_tab(app.tabs.active_id().unwrap());
        assert!(app.viewer.is_none());
        assert_eq!(app.tabs.len(), 0);
        assert!(app.store.state.reading(&paths[0]).is_some());
    }

    #[test]
    fn session_is_opt_in_restores_active_and_lazy_tabs_and_clears_explicitly() {
        use persistence::{ReadingState, Store};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let first = root.join("first.pdf");
        let second = root.join("second.pdf");
        for path in [&first, &second] {
            std::fs::write(path, document::tests::sample_pdf("", false)).unwrap();
        }
        let location = ReadingState {
            page: 1,
            scroll: [-17.0, -137.0],
            zoom: zoom::Zoom::Percent(4.5),
            layout: layout::LayoutMode::Facing,
            rotation: layout::Rotation::Counterclockwise,
        };
        let mut app = App::with_state(Store::temporary(directory.path()), None);
        app.open(first.clone());
        app.viewer.as_mut().unwrap().restore_reading(&location);
        app.open(second.clone());
        app.save_state(true);
        assert!(app.store.state.session.files.is_empty());
        app.store.state.restore_session = true;
        app.select_tab(app.tabs.find(&first).unwrap());
        app.save_state(true);
        drop(app);
        let mut app = App::with_state(Store::temporary(directory.path()), None);
        app.restore_session();
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.viewer.as_ref().unwrap().reading_state(), location);
        assert_eq!(app.viewer.as_ref().unwrap().path(), first);
        // The inactive restored file has not been opened. Losing it must not
        // destroy the active view or drop its recoverable tab metadata.
        std::fs::remove_file(&second).unwrap();
        let second_id = app.tabs.find(&second).unwrap();
        app.select_tab(second_id);
        assert!(app.open_error.is_some());
        assert_eq!(app.viewer.as_ref().unwrap().path(), first);
        app.close_tab(second_id);
        assert_eq!(app.tabs.len(), 1);
        app.store.state.clear_history();
        app.session_cleared = true;
        app.save_state(true);
        assert!(app.store.state.session.files.is_empty());
        assert!(app.store.state.recent.is_empty());
        let mut restart = App::with_state(Store::temporary(directory.path()), None);
        restart.restore_session();
        assert!(restart.viewer.is_none());
        assert_eq!(restart.tabs.len(), 0);
        app.store.state.restore_session = false;
        app.open(first);
        app.save_state(true);
        assert!(app.store.state.session.files.is_empty());
    }

    #[test]
    fn restored_encrypted_tab_authenticates_without_disturbing_selected_document() {
        use persistence::{ReadingState, Store};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let plain = root.join("plain.pdf");
        let locked = root.join("locked.pdf");
        std::fs::write(&plain, document::tests::sample_pdf("", false)).unwrap();
        document::tests::encrypted_fixture(
            &locked,
            "session-secret",
            mupdf::pdf::Permission::ACCESSIBILITY,
            mupdf::pdf::Encryption::Aes256,
        );
        let mut app = App::with_state(Store::temporary(directory.path()), None);
        app.store.state.restore_session = true;
        app.open(plain.clone());
        app.open(locked.clone());
        app.password_prompt
            .as_mut()
            .unwrap()
            .input
            .push_str("session-secret");
        app.submit_password();
        let location = ReadingState {
            page: 1,
            scroll: [19.0, -137.0],
            zoom: zoom::Zoom::Percent(2.75),
            layout: layout::LayoutMode::Continuous,
            rotation: layout::Rotation::Half,
        };
        app.viewer.as_mut().unwrap().restore_reading(&location);
        app.save_state(true);
        drop(app);
        let mut app = App::with_state(Store::temporary(directory.path()), None);
        app.restore_session();
        assert!(app.password_prompt.is_some());
        assert!(app.viewer.is_none());
        let preserved = app.store.state.clone();
        app.password_prompt
            .as_mut()
            .unwrap()
            .input
            .push_str("wrong-secret");
        app.submit_password();
        assert_eq!(app.store.state, preserved);
        password_frame(
            &mut app,
            &egui::Context::default(),
            vec![key_event(Key::Escape, Modifiers::NONE)],
        );
        app.select_tab(app.tabs.find(&plain).unwrap());
        let selected = app.tabs.active_id();
        app.select_tab(app.tabs.find(&locked).unwrap());
        assert_eq!(app.tabs.active_id(), selected);
        assert_eq!(app.viewer.as_ref().unwrap().path(), plain);
        app.password_prompt
            .as_mut()
            .unwrap()
            .input
            .push_str("session-secret");
        app.submit_password();
        assert_eq!(app.viewer.as_ref().unwrap().reading_state(), location);
        let count = app.tabs.len();
        app.open(plain);
        app.open(locked); // Existing unlocked tab is focused, not reopened.
        assert!(app.password_prompt.is_none());
        assert_eq!(app.tabs.len(), count);
        app.save_state(true);
        let json = std::fs::read_to_string(directory.path().join("state.json")).unwrap();
        for forbidden in ["session-secret", "wrong-secret", "password", "Chapter one"] {
            assert!(!json.contains(forbidden));
        }
    }

    #[test]
    fn tab_limit_rejects_new_files_but_allows_focusing_existing_tabs() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::with_state(persistence::Store::temporary(directory.path()), None);
        for index in 0..=persistence::MAX_SESSION {
            let path = directory.path().join(format!("{index}.pdf"));
            std::fs::write(&path, document::tests::sample_pdf("", false)).unwrap();
            app.open(path);
        }
        assert_eq!(app.tabs.len(), 16);
        assert!(app.open_error.as_ref().unwrap().contains("limit"));
        assert_eq!(
            app.viewer.as_ref().unwrap().path(),
            directory.path().join("15.pdf")
        );
        app.open(directory.path().join("0.pdf"));
        assert!(app.open_error.is_none());
        assert_eq!(app.tabs.len(), 16);
        assert_eq!(
            app.viewer.as_ref().unwrap().path(),
            directory.path().join("0.pdf")
        );
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
