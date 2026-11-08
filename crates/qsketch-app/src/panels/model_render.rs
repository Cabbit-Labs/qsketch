//! wgpu drawing for the Model panel. egui's render pass has no depth buffer or MSAA, so the
//! mesh is drawn into its own targets during `prepare` and copied into the panel in `paint`.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use qsketch_3d::{Mat4, Mesh, Sections, Vertex};
use wgpu::util::DeviceExt;

use crate::canvas::render::CanvasRenderer;
use crate::state::DocId;

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;
/// Close to the panel background, so the view reads as part of the app.
const CLEAR: wgpu::Color = wgpu::Color { r: 0.09, g: 0.1, b: 0.12, a: 1.0 };

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: Mat4,
    model: Mat4,
    uv_scale: [f32; 2],
    /// -1 when no section is selected.
    selected: i32,
    tint_all: u32,
}

struct GpuMesh {
    generation: u64,
    vertices: wgpu::Buffer,
    vertex_count: u32,
    sections: Option<wgpu::Buffer>,
    sections_generation: u64,
}

struct Targets {
    size: (u32, u32),
    msaa: wgpu::TextureView,
    resolve: wgpu::TextureView,
    depth: wgpu::TextureView,
    blit_bind: wgpu::BindGroup,
}

pub struct ModelRenderer {
    pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    blit_bgl: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// Shown when the panel has no document, or that document's canvas has not been drawn yet.
    blank: wgpu::TextureView,
    mesh: Option<GpuMesh>,
    targets: Option<Targets>,
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

impl ModelRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("qsketch model shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("model.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("qsketch model bgl"),
            entries: &[uniform_entry(0), texture_entry(1), sampler_entry(2)],
        });
        let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("qsketch model blit bgl"),
            entries: &[texture_entry(3), sampler_entry(4)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("qsketch model layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("qsketch model pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 4,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![3 => Uint32],
                    }),
                ],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(), // no culling: thin surfaces show both sides
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
            multiview_mask: None,
            cache: None,
        });
        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("qsketch model blit layout"),
            bind_group_layouts: &[Some(&blit_bgl)],
            immediate_size: 0,
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("qsketch model blit"),
            layout: Some(&blit_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_blit"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_blit"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("qsketch model uniforms"),
            contents: bytemuck::bytes_of(&Uniforms::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("qsketch model sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blank = device
            .create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("qsketch model blank"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[150, 150, 150, 255],
            )
            .create_view(&Default::default());
        Self { pipeline, blit_pipeline, bgl, blit_bgl, uniform, sampler, blank, mesh: None, targets: None }
    }

    fn ensure_mesh(&mut self, device: &wgpu::Device, mesh: &Mesh, generation: u64) {
        if self.mesh.as_ref().is_some_and(|m| m.generation == generation) {
            return;
        }
        // One copy of each vertex per triangle, so every triangle can carry its own section.
        let expanded: Vec<Vertex> = mesh.indices.iter().map(|&i| mesh.vertices[i as usize]).collect();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("qsketch model vertices"),
            contents: bytemuck::cast_slice(&expanded),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.mesh = Some(GpuMesh {
            generation,
            vertices,
            vertex_count: expanded.len() as u32,
            sections: None,
            sections_generation: u64::MAX,
        });
    }

    fn ensure_sections(&mut self, device: &wgpu::Device, sections: Option<&Sections>, generation: u64) {
        let Some(mesh) = &mut self.mesh else { return };
        if mesh.sections.is_some() && mesh.sections_generation == generation {
            return;
        }
        let ids: Vec<u32> = match sections {
            Some(s) => s.of_triangle.iter().flat_map(|&id| [id, id, id]).collect(),
            None => vec![0; mesh.vertex_count as usize],
        };
        mesh.sections = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("qsketch model sections"),
            contents: bytemuck::cast_slice(&ids),
            usage: wgpu::BufferUsages::VERTEX,
        }));
        mesh.sections_generation = generation;
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let texture = |label, samples, format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let attach = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let msaa = texture("qsketch model msaa", SAMPLES, COLOR_FORMAT, attach);
        let resolve = texture("qsketch model resolve", 1, COLOR_FORMAT, attach | wgpu::TextureUsages::TEXTURE_BINDING);
        let depth = texture("qsketch model depth", SAMPLES, DEPTH_FORMAT, attach);
        let blit_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("qsketch model blit bind"),
            layout: &self.blit_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&resolve) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.targets = Some(Targets { size, msaa, resolve, depth, blit_bind });
    }
}

pub struct ModelCallback {
    pub mesh: Arc<Mesh>,
    pub generation: u64,
    pub sections: Option<Arc<Sections>>,
    pub sections_generation: u64,
    pub selected: i32,
    pub tint_all: bool,
    pub view_proj: Mat4,
    pub model: Mat4,
    pub doc: Option<DocId>,
    /// Physical pixels.
    pub size: (u32, u32),
    /// The document's size, for the part of its tile-rounded texture that holds image.
    pub doc_size: (u32, u32),
}

impl CallbackTrait for ModelCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let doc = self.doc.and_then(|id| resources.get::<CanvasRenderer>()?.doc_texture(id));
        let Some(r) = resources.get_mut::<ModelRenderer>() else { return Vec::new() };
        r.ensure_mesh(device, &self.mesh, self.generation);
        r.ensure_sections(device, self.sections.as_deref(), self.sections_generation);
        r.ensure_targets(device, self.size);

        let uv_scale = match &doc {
            Some((_, w, h)) => [self.doc_size.0 as f32 / *w as f32, self.doc_size.1 as f32 / *h as f32],
            None => [1.0, 1.0],
        };
        let uniforms = Uniforms {
            view_proj: self.view_proj,
            model: self.model,
            uv_scale,
            selected: self.selected,
            tint_all: u32::from(self.tint_all),
        };
        queue.write_buffer(&r.uniform, 0, bytemuck::bytes_of(&uniforms));

        let view = doc.as_ref().map_or(&r.blank, |(view, _, _)| view);
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("qsketch model bind"),
            layout: &r.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: r.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&r.sampler) },
            ],
        });

        let (Some(targets), Some(mesh)) = (&r.targets, &r.mesh) else { return Vec::new() };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("qsketch model pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.msaa,
                depth_slice: None,
                resolve_target: Some(&targets.resolve),
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(CLEAR), store: wgpu::StoreOp::Discard },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&r.pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
        let Some(sections) = &mesh.sections else { return Vec::new() };
        pass.set_vertex_buffer(1, sections.slice(..));
        pass.draw(0..mesh.vertex_count, 0..1);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(r) = resources.get::<ModelRenderer>() else { return };
        let Some(targets) = &r.targets else { return };
        pass.set_pipeline(&r.blit_pipeline);
        pass.set_bind_group(0, &targets.blit_bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
