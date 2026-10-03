mod document;
mod renderer;

use std::{env, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail};
use document::PdfDocument;
use renderer::Renderer;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

struct App {
    document: Option<PdfDocument>,
    renderer: Option<Renderer>,
    zoom: f32,
}

impl App {
    fn new(document: PdfDocument) -> Self {
        Self {
            document: Some(document),
            renderer: None,
            zoom: 1.0,
        }
    }

    fn refresh_page(&mut self) -> Result<()> {
        let renderer = self.renderer.as_mut().context("renderer is not ready")?;
        let document = self.document.as_ref().context("document is not open")?;
        let image = document.render_current(renderer.viewport(), self.zoom)?;
        renderer.set_page(image);
        renderer.window().set_title(&format!(
            "Review — {} — {}/{} — {:.0}%",
            document.name(),
            document.current_page() + 1,
            document.page_count(),
            self.zoom * 100.0
        ));
        renderer.window().request_redraw();
        Ok(())
    }

    fn change_page(&mut self, delta: i32) {
        if self
            .document
            .as_mut()
            .is_some_and(|document| document.change_page(delta))
        {
            self.report_refresh_error();
        }
    }

    fn change_zoom(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(0.25, 4.0);
        self.report_refresh_error();
    }

    fn report_refresh_error(&mut self) {
        if let Err(error) = self.refresh_page() {
            eprintln!("failed to render page: {error:#}");
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title("Review")
            .with_inner_size(winit::dpi::LogicalSize::new(960, 720));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .expect("failed to create window"),
        );
        self.renderer =
            Some(pollster::block_on(Renderer::new(window)).expect("failed to initialize graphics"));
        self.report_refresh_error();
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

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                renderer.resize(size);
                self.report_refresh_error();
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = renderer.render() {
                    eprintln!("failed to draw frame: {error:#}");
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Escape | KeyCode::KeyQ) => event_loop.exit(),
                    PhysicalKey::Code(KeyCode::ArrowLeft | KeyCode::PageUp) => self.change_page(-1),
                    PhysicalKey::Code(KeyCode::ArrowRight | KeyCode::PageDown) => {
                        self.change_page(1)
                    }
                    PhysicalKey::Code(KeyCode::Equal | KeyCode::NumpadAdd) => {
                        self.change_zoom(1.25)
                    }
                    PhysicalKey::Code(KeyCode::Minus | KeyCode::NumpadSubtract) => {
                        self.change_zoom(0.8)
                    }
                    PhysicalKey::Code(KeyCode::Digit0 | KeyCode::Numpad0) => {
                        self.zoom = 1.0;
                        self.report_refresh_error();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn main() -> Result<()> {
    let mut args = env::args_os();
    let executable = args.next().unwrap_or_default();
    let Some(path) = args.next().map(PathBuf::from) else {
        bail!(
            "usage: {} <document.pdf>",
            PathBuf::from(executable).display()
        );
    };
    if args.next().is_some() {
        bail!("Review opens one PDF at a time");
    }

    let document = PdfDocument::open(path)?;
    let event_loop = EventLoop::new().context("failed to create event loop")?;
    event_loop.run_app(&mut App::new(document))?;
    Ok(())
}
