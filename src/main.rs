#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod document;
#[cfg(target_os = "macos")]
mod macos;
mod renderer;
mod search;
mod sidebar;
mod viewer;

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

struct App {
    viewer: Option<Viewer>,
    open_error: Option<String>,
    renderer: Option<Renderer>,
    repaint_at: Option<Instant>,
    fatal_error: Option<anyhow::Error>,
}

impl App {
    fn new() -> Self {
        Self {
            viewer: None,
            open_error: None,
            renderer: None,
            repaint_at: None,
            fatal_error: None,
        }
    }

    fn open(&mut self, path: PathBuf) {
        match document::PdfDocument::open(path) {
            Ok(document) => {
                self.viewer = Some(Viewer::new(document));
                self.open_error = None;
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
                let output = renderer.context.run_ui(input, |ui| {
                    (open_requested, quit) = app_ui(&mut self.viewer, &mut self.open_error, ui);
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
                if quit {
                    event_loop.exit();
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
}

fn app_ui(
    viewer: &mut Option<Viewer>,
    open_error: &mut Option<String>,
    root: &mut egui::Ui,
) -> (bool, bool) {
    let ctx = root.ctx().clone();
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
    (open_requested, quit)
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
                app_ui(&mut app.viewer, &mut app.open_error, ui),
                (false, false)
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
            events: [Key::ArrowRight, Key::Plus]
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
            app_ui(&mut app.viewer, &mut app.open_error, ui);
        });
        assert!(app.viewer.as_ref().unwrap().title().ends_with("2/2 — 125%"));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("new document.pdf");
        std::fs::write(&path, document::tests::sample_pdf("", false)).unwrap();
        app.open_error = Some("previous error".into());
        app.open(path);
        assert_eq!(
            app.viewer.as_ref().unwrap().title(),
            "Review — new document.pdf — 1/2 — 100%"
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
                assert_eq!(app_ui(&mut viewer, &mut None, ui), (true, false));
            });
        }
    }
}
