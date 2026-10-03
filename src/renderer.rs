use std::sync::Arc;

use anyhow::{Context, Result};
use winit::{
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoopProxy},
    window::Window,
};

use crate::AppEvent;

pub struct Renderer {
    window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pub context: egui::Context,
    input: egui_winit::State,
    painter: egui_wgpu::Renderer,
}

impl Renderer {
    pub async fn new(
        window: Arc<Window>,
        event_loop: &ActiveEventLoop,
        proxy: EventLoopProxy<AppEvent>,
    ) -> Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .context("failed to create drawing surface")?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("no compatible graphics adapter found")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Review device"),
                ..Default::default()
            })
            .await
            .context("failed to open graphics adapter")?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .unwrap_or(capabilities.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        let context = egui::Context::default();
        context.set_theme(egui::ThemePreference::System);
        let repaint_window = Arc::downgrade(&window);
        context.set_request_repaint_callback(move |request| {
            // Delayed UI repaints are scheduled from FullOutput by App.
            // Worker contexts must not keep a window alive after shutdown.
            if request.delay.is_zero()
                && let Some(window) = repaint_window.upgrade()
            {
                window.request_redraw();
            }
        });
        let mut input = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        // AccessKit must attach before the window is first made visible.
        input.init_accesskit(event_loop, &window, proxy);
        window.set_visible(true);
        let painter = egui_wgpu::Renderer::new(&device, format, Default::default());

        Ok(Self {
            window,
            instance,
            surface,
            device,
            queue,
            config,
            context,
            input,
            painter,
        })
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn on_event(&mut self, event: &WindowEvent) {
        let response = self.input.on_window_event(&self.window, event);
        if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
            self.window.request_redraw();
        }
    }

    pub fn take_input(&mut self) -> egui::RawInput {
        let mut input = self.input.take_egui_input(&self.window);
        let viewport = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        viewport.fullscreen = Some(self.window.fullscreen().is_some());
        viewport.maximized = Some(self.window.is_maximized());
        input
    }

    pub fn on_accesskit_event(&mut self, event: egui_winit::accesskit_winit::Event) {
        if event.window_id != self.window.id() {
            return;
        }
        use egui_winit::accesskit_winit::WindowEvent;
        match event.window_event {
            WindowEvent::InitialTreeRequested => self.context.enable_accesskit(),
            WindowEvent::ActionRequested(request) => {
                self.input.on_accesskit_action_request(request);
            }
            WindowEvent::AccessibilityDeactivated => self.context.disable_accesskit(),
        }
        self.window.request_redraw();
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.window.request_redraw();
    }

    pub fn render(&mut self, mut output: egui::FullOutput) -> Result<()> {
        if let Some(viewport) = output.viewport_output.get(&egui::ViewportId::ROOT) {
            // Use egui-winit's native handling (including X11's drag focus
            // guard). Close belongs to App's dirty-document protection, never
            // to the renderer. Only accept commands this single-window shell
            // emits; clipboard and surface lifecycles remain unchanged.
            egui_winit::process_viewport_commands(
                &self.context,
                &mut egui::ViewportInfo::default(),
                viewport
                    .commands
                    .iter()
                    .filter(|command| {
                        matches!(
                            command,
                            egui::ViewportCommand::Fullscreen(_)
                                | egui::ViewportCommand::Maximized(_)
                                | egui::ViewportCommand::Minimized(_)
                                | egui::ViewportCommand::StartDrag
                                | egui::ViewportCommand::BeginResize(_)
                        )
                    })
                    .cloned(),
                &self.window,
                &mut Vec::new(),
            );
            if viewport.commands.iter().any(|command| {
                matches!(
                    command,
                    egui::ViewportCommand::StartDrag | egui::ViewportCommand::BeginResize(_)
                )
            }) && let Some(pos) = self.context.input(|i| i.pointer.latest_pos())
            {
                // Native grabs can consume the button release (Wayland does).
                // End our gesture too, or the next edge drag retains the old
                // titlebar's widget ownership. stop_dragging alone leaves the
                // pointer button down; process a release through normal input.
                self.context.stop_dragging();
                let input = self.input.egui_input_mut();
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: input.modifiers,
                });
                self.window.request_redraw();
            }
        }
        output.platform_output.commands.retain(|command| {
            if let egui::OutputCommand::OpenUrl(request) = command {
                // Revalidate at the native boundary. The opener uses OS scheme
                // handlers; PDF strings are arguments/data, never shell code.
                if let Some(url) = crate::links::SafeUrl::parse(&request.url)
                    && let Err(error) = open::that_detached(url.as_str())
                {
                    eprintln!("failed to open link: {error}");
                }
                false
            } else {
                true
            }
        });
        self.input
            .handle_platform_output(&self.window, output.platform_output);
        // Upload deltas even when the surface is temporarily unavailable. egui
        // will not resend them on the next frame.
        for (id, delta) in &output.textures_delta.set {
            self.painter
                .update_texture(&self.device, &self.queue, *id, delta);
        }
        let result = self.paint(output.shapes, output.pixels_per_point);
        for id in &output.textures_delta.free {
            self.painter.free_texture(id);
        }
        result
    }

    fn paint(
        &mut self,
        shapes: Vec<egui::epaint::ClippedShape>,
        pixels_per_point: f32,
    ) -> Result<()> {
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output)
            | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
            wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .instance
                    .create_surface(self.window.clone())
                    .context("failed to recreate lost drawing surface")?;
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("graphics surface validation failed");
            }
        };
        let jobs = self.context.tessellate(shapes, pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point,
        };
        let view = output.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let commands =
            self.painter
                .update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewer pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            self.painter
                .render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        self.queue
            .submit(commands.into_iter().chain([encoder.finish()]));
        self.window.pre_present_notify();
        output.present();
        Ok(())
    }
}
