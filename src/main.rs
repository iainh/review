#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod document;
#[cfg(target_os = "macos")]
mod macos;
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
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};
use zeroize::{Zeroize, Zeroizing};

struct PasswordPrompt {
    path: PathBuf,
    input: Zeroizing<String>,
    incorrect: bool,
    focus: bool,
}

struct App {
    viewer: Option<Viewer>,
    open_error: Option<String>,
    password_prompt: Option<PasswordPrompt>,
    renderer: Option<Renderer>,
    repaint_at: Option<Instant>,
    fatal_error: Option<anyhow::Error>,
}

impl App {
    fn new() -> Self {
        Self {
            viewer: None,
            open_error: None,
            password_prompt: None,
            renderer: None,
            repaint_at: None,
            fatal_error: None,
        }
    }

    fn open(&mut self, path: PathBuf) {
        self.password_prompt = None;
        self.open_error = None;
        match document::PdfDocument::open_with_password(&path, None) {
            Ok(Some(document)) => {
                self.viewer = Some(Viewer::new(document));
            }
            Ok(None) => {
                self.password_prompt = Some(PasswordPrompt {
                    path,
                    input: Zeroizing::new(String::new()),
                    incorrect: false,
                    focus: true,
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
            Ok(Some(document)) => self.viewer = Some(Viewer::new(document)),
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
}

impl ApplicationHandler<PathBuf> for App {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, path: PathBuf) {
        self.open(path);
        if let Some(renderer) = &self.renderer {
            renderer.window().focus_window();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() || self.fatal_error.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Review")
            .with_inner_size(winit::dpi::LogicalSize::new(960, 720));
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
            pollster::block_on(Renderer::new(window)).context("failed to initialize graphics")
        })();
        match renderer {
            Ok(renderer) => {
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
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => renderer.resize(size),
            WindowEvent::DroppedFile(path) => self.open(path),
            WindowEvent::RedrawRequested => {
                let input = renderer.take_input();
                let mut open_requested = false;
                let mut quit = false;
                let mut submit_password = false;
                let output = renderer.context.run_ui(input, |ui| {
                    (open_requested, quit, submit_password) = app_ui(
                        &mut self.viewer,
                        &mut self.open_error,
                        &mut self.password_prompt,
                        ui,
                    );
                });
                let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
                self.repaint_at = Instant::now().checked_add(delay);
                renderer.window().set_title(
                    &self
                        .viewer
                        .as_ref()
                        .map_or_else(|| "Review".into(), Viewer::title),
                );
                if let Err(error) = renderer.render(output) {
                    eprintln!("failed to draw frame: {error:#}");
                }
                if let Some(viewer) = &mut self.viewer {
                    viewer.print_if_requested(renderer.window());
                }
                if quit {
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
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(deadline) = self.repaint_at {
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
        // Drop/join egui's clipboard worker before winit destroys the display.
        self.renderer = None;
    }
}

fn app_ui(
    viewer: &mut Option<Viewer>,
    open_error: &mut Option<String>,
    password_prompt: &mut Option<PasswordPrompt>,
    root: &mut egui::Ui,
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
            ui.label("Password");
            let mut edit = egui::TextEdit::singleline(&mut *prompt.input)
                .id(egui::Id::new("pdf_password"))
                .password(true)
                .desired_width(f32::INFINITY)
                .show(ui);
            // Password fields must not retain plaintext undo history in egui.
            edit.state.clear_undoer();
            edit.state.store(&ctx, edit.response.id);
            let field = edit.response;
            if prompt.focus {
                field.request_focus();
                prompt.focus = false;
            }
            if prompt.incorrect {
                ui.colored_label(egui::Color32::LIGHT_RED, "Incorrect password. Try again.");
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
    let mut open_requested = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::O));
    if open_error.is_some()
        && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
    {
        *open_error = None;
    }
    let quit = if let Some(viewer) = viewer {
        viewer.ui(root, &mut open_requested);
        viewer.quit
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
        ctx.input(|input| input.key_pressed(Key::Q) || input.key_pressed(Key::Escape))
    };
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
    let event_loop = EventLoop::<PathBuf>::with_user_event()
        .build()
        .context("failed to create event loop")?;
    #[cfg(target_os = "macos")]
    let _open_documents = macos::OpenDocuments::install(event_loop.create_proxy());
    let mut app = App::new();
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
        let mut app = App::new();
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
        let mut app = App::new();
        app.password_prompt = Some(PasswordPrompt {
            path: "locked.pdf".into(),
            input: Zeroizing::new("sensitive-text".into()),
            incorrect: false,
            focus: true,
        });
        let ctx = egui::Context::default();
        password_frame(&mut app, &ctx, vec![]);
        let (_, output) = password_frame(&mut app, &ctx, vec![]);
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
        let mut app = App::new();
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
        let mut app = App::new();
        app.viewer = Some(Viewer::new(document::tests::sample_document()));
        app.open("/missing/review-no-such-file.pdf".into());
        assert!(
            app.open_error
                .as_ref()
                .unwrap()
                .contains("failed to open PDF")
        );
        assert!(app.viewer.as_ref().unwrap().title().contains("sample.pdf"));
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
                    ui
                ),
                (false, false, false)
            );
        });
        assert!(app.open_error.is_none());
    }

    #[test]
    fn successful_open_replaces_document_and_resets_view() {
        let mut app = App::new();
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
            );
        });
        assert!(app.viewer.as_ref().unwrap().title().ends_with("2/2 — 125%"));
        let directory = tempfile::tempdir().unwrap();
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
                    app_ui(&mut viewer, &mut None, &mut None, ui),
                    (true, false, false)
                );
            });
        }
    }
}
