//! wgpu backend for the frame plan (architecture: "渲染管线" step 3).
//!
//! [`Renderer`] executes a [`FramePlan`] into an `Rgba8Unorm` texture the size of the canvas:
//! the editor viewport's texture, or an offscreen one read back with [`Renderer::render_image`].
//! It follows the same rules as `meshling-render-cpu` (the oracle), so with `sample_count = 1`
//! the two agree within 2/255 per channel; the editor uses 4× MSAA for smooth edges.
//!
//! Per frame: every planned mesh's posed vertices and UVs go into one vertex buffer and its
//! indices into one index buffer; each pass reads its parameters from one 256-byte slot of a
//! uniform buffer with a dynamic offset (the photocraft pattern). Masks render into one R8
//! texture per slot before any draw. Textures upload once and re-upload only when their pixel
//! buffer (`Arc`) changes.

mod context;

use std::collections::HashMap;
use std::sync::Arc;

use meshling_geom::Size;
use meshling_model::{Blend, ObjId, Project};
use meshling_plan::{FramePlan, Pass, Pixels, Textures};
use meshling_rig::Pose;
use wgpu::util::DeviceExt;

pub use context::{ContextError, DeviceHealth, Fault, GpuContext};

/// Format of every colour target this renderer draws into.
pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
const UNIFORM_STRIDE: u64 = 256;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum GpuError {
    #[error("the GPU device failed: {0:?}")]
    Device(Fault),
    #[error("mesh {0} is in the plan but not in the project or pose")]
    MissingMesh(ObjId),
    #[error("texture {texture} of mesh {mesh} has no pixels")]
    MissingTexture { mesh: ObjId, texture: ObjId },
    #[error("canvas {0:?} is empty or larger than the device allows")]
    CanvasSize(Size),
    #[error("reading the frame back failed: {0}")]
    Readback(String),
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawUniform {
    canvas: [f32; 2],
    opacity: f32,
    use_mask: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum PipeKind {
    Draw(Blend),
    Mask,
}

struct GpuTexture {
    /// Identity of the uploaded pixels: the `Arc`'s address and length.
    source: (usize, usize),
    view: wgpu::TextureView,
}

/// Executes frame plans on one device. Create once per device and reuse: it caches pipelines
/// and uploaded textures.
pub struct Renderer {
    ctx: GpuContext,
    shader: wgpu::ShaderModule,
    uniform_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: HashMap<(PipeKind, u32), wgpu::RenderPipeline>,
    sampler: wgpu::Sampler,
    no_mask: wgpu::TextureView,
    textures: HashMap<ObjId, GpuTexture>,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer").field("adapter", &self.ctx.adapter).field("textures", &self.textures.len()).finish()
    }
}

/// Where one mesh's geometry sits in the frame's buffers.
#[derive(Clone, Copy)]
struct Span {
    base_vertex: i32,
    first_index: u32,
    index_count: u32,
    texture: ObjId,
}

impl Renderer {
    pub fn new(ctx: GpuContext) -> Self {
        let device = &ctx.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("meshling mesh.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mesh.wgsl").into()),
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("meshling draw uniform"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<DrawUniform>() as u64),
                },
                count: None,
            }],
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("meshling texture + mask"),
            entries: &[
                tex_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex_entry(2),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("meshling mesh"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("meshling bilinear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let no_mask = device
            .create_texture_with_data(
                &ctx.queue,
                &wgpu::TextureDescriptor {
                    label: Some("meshling no mask"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: MASK_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[255],
            )
            .create_view(&Default::default());
        Self {
            ctx,
            shader,
            uniform_layout,
            texture_layout,
            pipeline_layout,
            pipelines: HashMap::new(),
            sampler,
            no_mask,
            textures: HashMap::new(),
        }
    }

    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    fn pipeline(&mut self, kind: PipeKind, samples: u32) -> &wgpu::RenderPipeline {
        let (device, shader, layout) = (&self.ctx.device, &self.shader, &self.pipeline_layout);
        self.pipelines.entry((kind, samples)).or_insert_with(|| {
            use wgpu::{BlendComponent as C, BlendFactor as F, BlendOperation::Add as A};
            let comp = |src_factor, dst_factor| C { src_factor, dst_factor, operation: A };
            let (entry, format, blend) = match kind {
                PipeKind::Mask => {
                    let c = comp(F::One, F::OneMinusSrcAlpha);
                    ("fs_mask", MASK_FORMAT, wgpu::BlendState { color: c, alpha: c })
                }
                PipeKind::Draw(Blend::Normal) => {
                    let c = comp(F::One, F::OneMinusSrcAlpha);
                    ("fs_draw", TARGET_FORMAT, wgpu::BlendState { color: c, alpha: c })
                }
                PipeKind::Draw(Blend::Add) => (
                    "fs_draw",
                    TARGET_FORMAT,
                    wgpu::BlendState { color: comp(F::One, F::One), alpha: comp(F::Zero, F::One) },
                ),
                PipeKind::Draw(Blend::Multiply) => (
                    "fs_draw",
                    TARGET_FORMAT,
                    wgpu::BlendState { color: comp(F::Dst, F::OneMinusSrcAlpha), alpha: comp(F::Zero, F::One) },
                ),
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                    })],
                },
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    fn upload(&mut self, id: ObjId, pixels: &Pixels) {
        let source = (Arc::as_ptr(&pixels.rgba) as *const u8 as usize, pixels.rgba.len());
        if self.textures.get(&id).is_some_and(|t| t.source == source) {
            return;
        }
        let texture = self.ctx.device.create_texture_with_data(
            &self.ctx.queue,
            &wgpu::TextureDescriptor {
                label: Some("meshling layer"),
                size: wgpu::Extent3d {
                    width: pixels.size.width.max(1),
                    height: pixels.size.height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            if pixels.rgba.is_empty() { &[0, 0, 0, 0] } else { &pixels.rgba },
        );
        self.textures.insert(id, GpuTexture { source, view: texture.create_view(&Default::default()) });
    }

    /// Draw `plan` into `target` (an `Rgba8Unorm` view the size of the canvas), clearing it
    /// first. With `samples > 1`, `target` is the resolve target and a multisampled texture is
    /// drawn into.
    pub fn render_to(
        &mut self,
        project: &Project,
        pose: &Pose,
        plan: &FramePlan,
        textures: &Textures,
        target: &wgpu::TextureView,
        samples: u32,
    ) -> Result<(), GpuError> {
        if let Some(f) = self.ctx.health.fault() {
            return Err(GpuError::Device(f));
        }
        let size = plan.canvas;
        let max = self.ctx.device.limits().max_texture_dimension_2d;
        if size.width == 0 || size.height == 0 || size.width > max || size.height > max {
            return Err(GpuError::CanvasSize(size));
        }

        // Geometry of every mesh the plan touches, once each.
        let mut spans: HashMap<ObjId, Span> = HashMap::new();
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let meshes = plan.passes.iter().flat_map(|p| match p {
            Pass::Mask { meshes, .. } => meshes.clone(),
            Pass::Draw { mesh, .. } => vec![*mesh],
        });
        for id in meshes {
            if spans.contains_key(&id) {
                continue;
            }
            let mesh = project.mesh(id).ok_or(GpuError::MissingMesh(id))?;
            let posed = pose.mesh(id).ok_or(GpuError::MissingMesh(id))?;
            let pixels =
                textures.get(&mesh.texture).ok_or(GpuError::MissingTexture { mesh: id, texture: mesh.texture })?;
            self.upload(mesh.texture, pixels);
            let span = Span {
                base_vertex: vertices.len() as i32,
                first_index: indices.len() as u32,
                index_count: mesh.indices.len() as u32,
                texture: mesh.texture,
            };
            vertices.extend(
                posed.positions.iter().zip(mesh.uvs.iter()).map(|(p, uv)| Vertex { pos: [p.x, p.y], uv: [uv.x, uv.y] }),
            );
            let n = posed.positions.len().min(mesh.uvs.len()) as u32;
            // Out-of-range indices would be a validation error; drop such triangles like the CPU does.
            for tri in mesh.indices.as_chunks::<3>().0 {
                if tri.iter().all(|&i| i < n) {
                    indices.extend_from_slice(tri);
                }
            }
            spans.insert(id, Span { index_count: indices.len() as u32 - span.first_index, ..span });
        }

        // One uniform slot per pass, plus one per mask mesh.
        let canvas = [size.width as f32, size.height as f32];
        let mut uniforms: Vec<u8> = Vec::new();
        let mut push = |opacity: f32, use_mask: bool| {
            let offset = uniforms.len() as u32;
            uniforms.extend_from_slice(bytemuck::bytes_of(&DrawUniform { canvas, opacity, use_mask: use_mask as u32 }));
            uniforms.resize(uniforms.len().next_multiple_of(UNIFORM_STRIDE as usize), 0);
            offset
        };
        let mask_offset = push(1.0, false);
        let draw_offsets: Vec<u32> = plan
            .passes
            .iter()
            .map(|p| match p {
                Pass::Draw { opacity, mask, .. } => push(*opacity, mask.is_some()),
                Pass::Mask { .. } => mask_offset,
            })
            .collect();

        let device = &self.ctx.device;
        let usage = wgpu::BufferUsages::VERTEX;
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("meshling vertices"),
            contents: if vertices.is_empty() { &[0; 16] } else { bytemuck::cast_slice(&vertices) },
            usage,
        });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("meshling indices"),
            contents: if indices.is_empty() { &[0; 4] } else { bytemuck::cast_slice(&indices) },
            usage: wgpu::BufferUsages::INDEX,
        });
        let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("meshling draw uniforms"),
            contents: &uniforms,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let uniform_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("meshling draw uniforms"),
            layout: &self.uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &ubuf,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<DrawUniform>() as u64),
                }),
            }],
        });

        let extent = wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 };
        let masks: Vec<wgpu::TextureView> = (0..plan.mask_slots)
            .map(|_| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("meshling mask"),
                        size: extent,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: MASK_FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            })
            .collect();
        let msaa = (samples > 1).then(|| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("meshling msaa"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format: TARGET_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        });

        // Make sure every pipeline exists before borrowing them for the passes.
        self.pipeline(PipeKind::Mask, 1);
        for p in &plan.passes {
            if let Pass::Draw { blend, .. } = p {
                self.pipeline(PipeKind::Draw(*blend), samples);
            }
        }
        let device = &self.ctx.device;
        let group = |texture: ObjId, mask: &wgpu::TextureView| {
            let view = self.textures.get(&texture).map(|t| &t.view);
            view.map(|view| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("meshling texture + mask"),
                    layout: &self.texture_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(mask) },
                    ],
                })
            })
        };

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("meshling frame") });
        let clear =
            wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store };
        let draw_span = |rp: &mut wgpu::RenderPass<'_>, span: &Span, mask: &wgpu::TextureView, offset: u32| {
            if let Some(g) = group(span.texture, mask) {
                rp.set_bind_group(0, &uniform_group, &[offset]);
                rp.set_bind_group(1, &g, &[]);
                rp.draw_indexed(span.first_index..span.first_index + span.index_count, span.base_vertex, 0..1);
            }
        };

        for pass in &plan.passes {
            let Pass::Mask { slot, meshes } = pass else { continue };
            let Some(view) = masks.get(*slot as usize) else { continue };
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("meshling mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: clear,
                })],
                ..Default::default()
            });
            rp.set_pipeline(&self.pipelines[&(PipeKind::Mask, 1)]);
            rp.set_vertex_buffer(0, vbuf.slice(..));
            rp.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
            for id in meshes {
                if let Some(span) = spans.get(id) {
                    draw_span(&mut rp, span, &self.no_mask, mask_offset);
                }
            }
        }
        {
            let (view, resolve_target) = match &msaa {
                Some(m) => (m, Some(target)),
                None => (target, None),
            };
            let store = if msaa.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store };
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("meshling draw"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations { store, ..clear },
                })],
                ..Default::default()
            });
            rp.set_vertex_buffer(0, vbuf.slice(..));
            rp.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
            for (pass, &offset) in plan.passes.iter().zip(&draw_offsets) {
                let Pass::Draw { mesh, blend, mask, .. } = pass else { continue };
                let Some(span) = spans.get(mesh) else { continue };
                let mask_view = mask.and_then(|s| masks.get(s as usize)).unwrap_or(&self.no_mask);
                rp.set_pipeline(&self.pipelines[&(PipeKind::Draw(*blend), samples)]);
                draw_span(&mut rp, span, mask_view, offset);
            }
        }
        self.ctx.queue.submit([encoder.finish()]);
        match self.ctx.health.fault() {
            Some(f) => Err(GpuError::Device(f)),
            None => Ok(()),
        }
    }

    /// Render offscreen and read the frame back as premultiplied RGBA8 rows (top row first).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_image(
        &mut self,
        project: &Project,
        pose: &Pose,
        plan: &FramePlan,
        textures: &Textures,
        samples: u32,
    ) -> Result<Vec<u8>, GpuError> {
        let size = plan.canvas;
        if size.width == 0 || size.height == 0 {
            return Err(GpuError::CanvasSize(size));
        }
        let extent = wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 };
        let texture = self.ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("meshling offscreen"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.render_to(project, pose, plan, textures, &texture.create_view(&Default::default()), samples)?;

        let row = size.width * 4;
        let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("meshling readback"),
            size: u64::from(padded) * u64::from(size.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("meshling readback") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(size.height),
                },
            },
            extent,
        );
        self.ctx.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.ctx.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| GpuError::Readback(e.to_string()))?;
        rx.recv().map_err(|e| GpuError::Readback(e.to_string()))?.map_err(|e| GpuError::Readback(e.to_string()))?;
        let data = slice.get_mapped_range().map_err(|e| GpuError::Readback(e.to_string()))?;
        let mut out = Vec::with_capacity((row * size.height) as usize);
        for r in data.chunks(padded as usize) {
            out.extend_from_slice(&r[..row as usize]);
        }
        drop(data);
        buffer.unmap();
        match self.ctx.health.fault() {
            Some(f) => Err(GpuError::Device(f)),
            None => Ok(out),
        }
    }
}
