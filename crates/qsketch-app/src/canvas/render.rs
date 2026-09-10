//! wgpu renderer for document canvases. One texture per open document holding
//! the premultiplied composite; only dirty 64×64 tiles are re-uploaded.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use qsketch_core::{TILE, TILE_BYTES};
use wgpu::util::DeviceExt;

use crate::state::DocId;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Uniforms {
    pub viewport_min: [f32; 2],
    pub viewport_size: [f32; 2],
    pub doc_size: [f32; 2],
    pub tex_size: [f32; 2],
    pub center: [f32; 2],
    pub zoom: f32,
    pub rotation: f32,
    pub flip: f32,
    pub checker_size: f32,
    pub grid: f32,
    pub ppp: f32,
    pub checker_a: [f32; 4],
    pub checker_b: [f32; 4],
    pub outside: [f32; 4],
}

/// A 64×64 premultiplied RGBA8 tile to upload.
pub struct TileUpload {
    pub tx: u32,
    pub ty: u32,
    pub data: Vec<u8>,
}

struct GpuDoc {
    texture: wgpu::Texture,
    uniform: wgpu::Buffer,
    bind_nearest: wgpu::BindGroup,
    bind_linear: wgpu::BindGroup,
    tex_w: u32,
    tex_h: u32,
    /// Every tile has been uploaded at least once.
    filled: bool,
}

pub struct CanvasRenderer {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    sampler_nearest: wgpu::Sampler,
    sampler_linear: wgpu::Sampler,
    docs: HashMap<DocId, GpuDoc>,
}

impl CanvasRenderer {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("qsketch canvas shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("qsketch canvas bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("qsketch canvas layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("qsketch canvas pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
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
        let sampler = |filter: wgpu::FilterMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("qsketch canvas sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            })
        };
        Self {
            pipeline,
            bgl,
            sampler_nearest: sampler(wgpu::FilterMode::Nearest),
            sampler_linear: sampler(wgpu::FilterMode::Linear),
            docs: HashMap::new(),
        }
    }

    fn ensure_doc(&mut self, device: &wgpu::Device, id: DocId, width: u32, height: u32) -> bool {
        let tex_w = width.div_ceil(TILE as u32) * TILE as u32;
        let tex_h = height.div_ceil(TILE as u32) * TILE as u32;
        if let Some(d) = self.docs.get(&id) {
            if d.tex_w == tex_w && d.tex_h == tex_h {
                return false;
            }
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("qsketch doc texture"),
            size: wgpu::Extent3d { width: tex_w, height: tex_h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("qsketch doc uniforms"),
            contents: bytemuck::bytes_of(&Uniforms::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let make_bind = |sampler: &wgpu::Sampler| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("qsketch doc bind"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
                ],
            })
        };
        let bind_nearest = make_bind(&self.sampler_nearest);
        let bind_linear = make_bind(&self.sampler_linear);
        self.docs.insert(id, GpuDoc { texture, uniform, bind_nearest, bind_linear, tex_w, tex_h, filled: false });
        true
    }

    pub fn forget(&mut self, id: DocId) {
        self.docs.remove(&id);
    }

    /// True when the GPU holds a fully uploaded texture of this size.
    pub fn is_ready(&self, id: DocId, width: u32, height: u32) -> bool {
        let tex_w = width.div_ceil(TILE as u32) * TILE as u32;
        let tex_h = height.div_ceil(TILE as u32) * TILE as u32;
        self.docs.get(&id).is_some_and(|d| d.filled && d.tex_w == tex_w && d.tex_h == tex_h)
    }
}

/// Per-frame paint callback for one document canvas.
pub struct CanvasCallback {
    pub doc_id: DocId,
    pub width: u32,
    pub height: u32,
    pub tiles: Vec<TileUpload>,
    pub uniforms: Uniforms,
    pub linear: bool,
    /// Set when the caller wants every tile uploaded (e.g. texture was recreated).
    pub full: bool,
}

impl CallbackTrait for CanvasCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(r) = resources.get_mut::<CanvasRenderer>() else { return Vec::new() };
        r.ensure_doc(device, self.doc_id, self.width, self.height);
        let d = r.docs.get_mut(&self.doc_id).expect("doc texture");
        if self.full {
            d.filled = true;
        }
        for t in &self.tiles {
            debug_assert_eq!(t.data.len(), TILE_BYTES);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &d.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: t.tx * TILE as u32, y: t.ty * TILE as u32, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &t.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(TILE as u32 * 4),
                    rows_per_image: Some(TILE as u32),
                },
                wgpu::Extent3d { width: TILE as u32, height: TILE as u32, depth_or_array_layers: 1 },
            );
        }
        let mut u = self.uniforms;
        u.tex_size = [d.tex_w as f32, d.tex_h as f32];
        queue.write_buffer(&d.uniform, 0, bytemuck::bytes_of(&u));
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(r) = resources.get::<CanvasRenderer>() else { return };
        let Some(d) = r.docs.get(&self.doc_id) else { return };
        pass.set_pipeline(&r.pipeline);
        pass.set_bind_group(0, if self.linear { &d.bind_linear } else { &d.bind_nearest }, &[]);
        pass.draw(0..6, 0..1);
    }
}

/// True if the GPU still needs every tile of this document (no texture yet,
/// size changed, or a previous full upload never reached the GPU).
pub fn needs_full_upload(render_state: Option<&egui_wgpu::RenderState>, id: DocId, width: u32, height: u32) -> bool {
    match render_state {
        Some(rs) => !rs
            .renderer
            .read()
            .callback_resources
            .get::<CanvasRenderer>()
            .is_some_and(|r| r.is_ready(id, width, height)),
        None => true,
    }
}
