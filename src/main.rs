mod document;
mod renderer;
mod search;
mod sidebar;
mod viewer;

use std::{env, path::PathBuf, sync::Arc, time::Instant};

use anyhow::{Context, Result, bail};
use document::PdfDocument;
use renderer::Renderer;
use viewer::Viewer;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

struct App {
    viewer: Viewer,
    renderer: Option<Renderer>,
    repaint_at: Option<Instant>,
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
        self.renderer.as_ref().unwrap().window().request_redraw();
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
            WindowEvent::RedrawRequested => {
                let input = renderer.take_input();
                let output = renderer.context.run_ui(input, |ui| self.viewer.ui(ui));
                let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
                self.repaint_at = Instant::now().checked_add(delay);
                renderer.window().set_title(&self.viewer.title());
                if let Err(error) = renderer.render(output) {
                    eprintln!("failed to draw frame: {error:#}");
                }
                if self.viewer.quit {
                    event_loop.exit();
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
    event_loop.run_app(&mut App {
        viewer: Viewer::new(document),
        renderer: None,
        repaint_at: None,
    })?;
    Ok(())
}
