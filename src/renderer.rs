use std::sync::Arc;

use anyhow::{Context, Result};
use wgpu::util::DeviceExt;
use winit::{dpi::PhysicalSize, window::Window};

use crate::document::PageImage;

const SHADER: &str = r#"
struct Quad {
    scale: vec2<f32>,
    _padding: vec2<f32>,
};

@group(0) @binding(0) var page: texture_2d<f32>;
@group(0) @binding(1) var page_sampler: sampler;
@group(0) @binding(2) var<uniform> quad: Quad;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let positions = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    );
    let uvs = array<vec2<f32>, 6>(
        vec2(0.0, 1.0), vec2(1.0, 1.0), vec2(0.0, 0.0),
        vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(1.0, 0.0),
    );
    var output: VertexOutput;
    output.position = vec4(positions[index] * quad.scale, 0.0, 1.0);
    output.uv = uvs[index];
    return output;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(page, page_sampler, input.uv);
}
"#;

pub struct Renderer {
    window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    page: Option<GpuPage>,
}

struct GpuPage {
    bind_group: wgpu::BindGroup,
    _texture: wgpu::Texture,
    _uniform: wgpu::Buffer,
}

impl Renderer {
    pub async fn new(window: Arc<Window>) -> Result<Self> {
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
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .context("failed to open graphics adapter")?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
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

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("page layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("page pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("page shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("page pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Ok(Self {
            window,
            instance,
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_group_layout,
            page: None,
        })
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn viewport(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn set_page(&mut self, image: PageImage) {
        let size = wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rendered PDF page"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width * 4),
                rows_per_image: Some(image.height),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("page sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let quad = [
            image.width as f32 / self.config.width as f32,
            image.height as f32 / self.config.height as f32,
            0.0,
            0.0,
        ];
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("page dimensions"),
                contents: bytemuck::cast_slice(&quad),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("page resources"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        self.page = Some(GpuPage {
            bind_group,
            _texture: texture,
            _uniform: uniform,
        });
    }

    pub fn render(&mut self) -> Result<()> {
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output)
            | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
            wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .instance
                    .create_surface(self.window.clone())
                    .context("failed to recreate drawing surface")?;
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("graphics surface validation failed");
            }
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("page pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.09,
                            g: 0.10,
                            b: 0.11,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(page) = &self.page {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &page.bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        output.present();
        Ok(())
    }
}
