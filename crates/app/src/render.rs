//! GPU rendering: strokes as cached meshes in local coordinates + per-instance transform.
//!
//! Precision: the per-stroke transform (`offset`, `scale`) is computed on the CPU from exact
//! coordinates, relative to the camera. When a stroke is viewed far more magnified than it was
//! drawn (`scale` large), f32 vertex math would wobble, so such strokes are transformed in f64
//! on the CPU and clipped to a guard band before being uploaded (see [`push_cpu_path`]).

use canvas_core::document::VisibleStroke;
use canvas_core::mesh::{MAX_LOD, Mesh, lod_for, tessellate, tessellate_part};
use canvas_core::stroke::{Stroke, StrokeId};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const MSAA: u32 = 4;
/// Up to this many binary orders of magnification the GPU f32 path is used.
const GPU_MAX_REL: f64 = 6.0;
const CHUNK_VERTICES: u64 = 1 << 20;
/// Bytes per vertex: local x, y and ink intensity.
const VERTEX_SIZE: u64 = 12;
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Stencil8;
const CHUNK_INDICES: u64 = 3 << 20;
/// Meshes unused for this many frames are evicted.
const EVICT_FRAMES: u64 = 20_000;
/// Time budget per frame for tessellating meshes that are not cached yet.
const TESSELLATION_BUDGET: Duration = Duration::from_millis(6);
/// Strokes with no mesh at all and at most this many points are tessellated right away (a
/// stroke just drawn must never blink out); everything else goes to the worker threads.
const SYNC_POINTS: usize = 1500;
/// A cached mesh up to this many LODs finer than needed is used as is (no re-tessellation
/// when zooming out; finer geometry just costs a few more triangles).
const LOD_SLACK: u32 = 2;

/// Background tessellation: worker threads turn strokes into meshes; the render thread only
/// uploads finished meshes, so zooming over a big drawing never stalls a frame.
struct Workers {
    jobs: std::sync::mpsc::Sender<(u64, StrokeId, u32, Stroke)>,
    results: std::sync::mpsc::Receiver<(u64, StrokeId, u32, Mesh)>,
    pending: std::collections::HashSet<(StrokeId, u32)>,
    /// Bumped when the document changes completely: results for older jobs are dropped.
    generation: u64,
}

impl Workers {
    fn new() -> Self {
        let (jobs, job_rx) = std::sync::mpsc::channel::<(u64, StrokeId, u32, Stroke)>();
        let (res_tx, results) = std::sync::mpsc::channel();
        let job_rx = Arc::new(std::sync::Mutex::new(job_rx));
        let n = std::thread::available_parallelism().map_or(2, |n| n.get()).saturating_sub(1).clamp(1, 4);
        for i in 0..n {
            let rx = job_rx.clone();
            let tx = res_tx.clone();
            let _ = std::thread::Builder::new().name(format!("tessellate-{i}")).spawn(move || {
                loop {
                    let job = rx.lock().ok().and_then(|r| r.recv().ok());
                    let Some((generation, id, lod, stroke)) = job else { break };
                    if tx.send((generation, id, lod, tessellate(&stroke, lod))).is_err() {
                        break;
                    }
                }
            });
        }
        Self { jobs, results, pending: Default::default(), generation: 0 }
    }

    fn request(&mut self, stroke: &Stroke, lod: u32) {
        if self.pending.len() < 20_000 && self.pending.insert((stroke.id, lod)) {
            let _ = self.jobs.send((self.generation, stroke.id, lod, stroke.clone()));
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    offset: [f32; 2],
    /// Scale × (cos, sin) of the view rotation: screen = offset + R·local·scale.
    rs: [f32; 2],
    color: [f32; 4],
    /// Paper grain: strength, 1 / cell size (local units), unused, unused.
    params: [f32; 4],
}

const INSTANCE_ATTRS: [wgpu::VertexAttribute; 4] = [
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 1 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 8, shader_location: 2 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 3 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 4 },
];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

struct Chunk {
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    v_cap: u64,
    i_cap: u64,
    v_used: u64,
    i_used: u64,
    live: usize,
}

#[derive(Clone, Copy)]
struct CachedMesh {
    chunk: usize,
    base_vertex: i32,
    first_index: u32,
    index_count: u32,
    last_used: u64,
}

enum Draw {
    Cached { chunk: usize, base_vertex: i32, first_index: u32, count: u32, instance: u32 },
    Dynamic { base_vertex: i32, first_index: u32, count: u32, instance: u32 },
}

pub struct Gpu {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
}

impl Gpu {
    pub async fn new(instance: &wgpu::Instance, surface: wgpu::Surface<'static>, size: [u32; 2]) -> Self {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .expect("no suitable GPU adapter");
        log::info!("GPU: {:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("device"),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .expect("failed to create device");
        let caps = surface.get_capabilities(&adapter);
        // Prefer a non-sRGB format: colours are blended in gamma space like in paint apps.
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        // Mailbox on desktop for the lowest pen latency; vsync (Fifo) on phones, where
        // rendering faster than the display only drains the battery.
        // Mailbox on desktop for the lowest pen latency; vsync (Fifo) on phones, where
        // rendering faster than the display only drains the battery.
        let present_mode = if !cfg!(target_os = "android") && caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size[0].max(1),
            height: size[1].max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);
        Self { surface, device, queue, config }
    }

    pub fn resize(&mut self, size: [u32; 2]) {
        if size[0] == 0 || size[1] == 0 {
            return;
        }
        self.config.width = size[0];
        self.config.height = size[1];
        self.surface.configure(&self.device, &self.config);
    }
}

/// CPU time spent in each phase of the last frame (seconds).
#[derive(Default, Clone, Copy, Debug)]
pub struct Timings {
    pub acquire: f64,
    pub build: f64,
    pub submit: f64,
    pub present: f64,
    pub triangles: usize,
    /// Part of `build` spent on the stroke being drawn.
    pub active: f64,
}

/// Where a frame goes.
#[derive(Clone, Copy)]
pub enum Target<'a> {
    Surface(&'a Gpu),
    View {
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        format: wgpu::TextureFormat,
        size: [u32; 2],
        view: &'a wgpu::TextureView,
    },
}

impl Target<'_> {
    pub fn device_queue_format(&self) -> (&wgpu::Device, &wgpu::Queue, wgpu::TextureFormat) {
        match self {
            Target::Surface(g) => (&g.device, &g.queue, g.config.format),
            Target::View { device, queue, format, .. } => (device, queue, *format),
        }
    }
}

/// One reference photo for this frame.
pub struct ImageDraw<'a> {
    pub id: u64,
    /// Screen corners for texture coordinates (0,0), (1,0), (1,1), (0,1).
    pub corners: [[f64; 2]; 4],
    pub opacity: f32,
    /// Decoded pixels (used once to create the texture).
    pub rgba: &'a image::RgbaImage,
}

struct ImageTexture {
    bind_group: wgpu::BindGroup,
    last_used: u64,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ImageVertex {
    pos: [f32; 2],
    uv: [f32; 2],
    opacity: f32,
}

/// The stroke in progress: already tessellated parts plus the points after them.
#[derive(Clone)]
pub struct ActiveDraw<'a> {
    pub stroke: &'a Stroke,
    /// Position of its layer.
    pub layer: u16,
    pub offset: [f64; 2],
    pub scale: f64,
    pub frozen: &'a [Mesh],
    /// First point of the live tail (the last frozen part ends here too).
    pub tail_from: usize,
    /// Extra point after the last one, drawn only (input extrapolated to hide latency).
    pub predicted: Option<canvas_core::stroke::StrokePoint>,
}

/// What the app wants drawn this frame.
pub struct FrameInput<'a> {
    pub visible: &'a [VisibleStroke],
    pub doc: &'a canvas_core::document::Document,
    /// Stroke being drawn right now (not in the document yet) with its transform.
    pub active: Option<ActiveDraw<'a>>,
    /// Draw the scene cache first, with its corners at these screen points
    /// (texture corners (0,0), (1,0), (1,1), (0,1)).
    pub scene: Option<[[f64; 2]; 4]>,
    /// Reference photos to draw below the strokes, bottom first.
    pub images: &'a [ImageDraw<'a>],
    /// Grid lines (from, to, strong) in screen px, drawn on the background.
    pub grid: &'a [([f64; 2], [f64; 2], bool)],
    /// Strokes added this frame (just drawn): tessellated right away, whatever the budget, so
    /// a finished stroke never blinks out.
    pub urgent: &'a [StrokeId],
    /// Opacity of each layer, by layer position (`VisibleStroke::layer`); missing = 1.
    pub layer_opacity: &'a [f32],
    pub log_zoom: f64,
    /// (cos, sin) of the view rotation.
    pub rotation: [f64; 2],
    pub background: [f32; 4],
    pub egui: Option<(&'a [egui::ClippedPrimitive], f32)>,
    /// egui texture updates; always consumed, even when the frame is skipped.
    pub textures: egui::TexturesDelta,
}

struct SceneCache {
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    size: [u32; 2],
}

/// Image id used for the scene cache quad.
const SCENE_CACHE_ID: u64 = u64::MAX;
/// Image id used for the grid lines.
const GRID_ID: u64 = u64::MAX - 1;

/// Per-size render targets: strokes are drawn with MSAA into a transparent layer (so the
/// eraser can clear pixels), which is then put over the background on the screen.
struct Targets {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    stencil: wgpu::TextureView,
    layer: wgpu::TextureView,
    composite: wgpu::BindGroup,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Opaque,
    Blend,
    Erase,
}

pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    pipeline_opaque: wgpu::RenderPipeline,
    /// Eraser strokes: clear what is under them in the stroke layer.
    pipeline_erase: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    composite_bgl: wgpu::BindGroupLayout,
    /// Layer opacities for the composite shader (64 × vec4).
    layer_opacity_buf: wgpu::Buffer,
    image_pipeline: wgpu::RenderPipeline,
    image_bgl: wgpu::BindGroupLayout,
    image_sampler: wgpu::Sampler,
    image_textures: HashMap<u64, ImageTexture>,
    /// A 1×1 grey texture: grid lines are drawn as thin image quads.
    grid_bind_group: wgpu::BindGroup,
    grid_texture: wgpu::Texture,
    grid_ready: bool,
    image_vbuf: wgpu::Buffer,
    image_v_cap: u64,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    targets: Option<Targets>,
    /// Targets sized for the scene cache (kept separately so neither size thrashes).
    cache_targets: Option<Targets>,
    /// Last fully rendered scene (background, photos, layers; no UI), shown transformed while
    /// the view moves so gestures cost the same however big the drawing is.
    scene_cache: Option<SceneCache>,
    chunks: Vec<Chunk>,
    meshes: HashMap<(StrokeId, u32), CachedMesh>,
    workers: Workers,
    cpu_meshes: HashMap<(StrokeId, u32), (Arc<Mesh>, u64)>,
    instance_buf: wgpu::Buffer,
    instance_cap: u64,
    dyn_vbuf: wgpu::Buffer,
    dyn_ibuf: wgpu::Buffer,
    dyn_v_cap: u64,
    dyn_i_cap: u64,
    frame: u64,
    pub egui: egui_wgpu::Renderer,
    srgb: bool,
    /// Set when some meshes were postponed because of the tessellation budget.
    pub needs_another_frame: bool,
    /// CPU time of the last frame's phases (seconds), for profiling.
    pub timings: Timings,
    /// Tessellate everything this frame (offscreen renders that must be complete).
    unlimited_budget: bool,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("stroke"),
            source: wgpu::ShaderSource::Wgsl(include_str!("stroke.wgsl").into()),
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pl"),
            bind_group_layouts: &[Some(&bgl)],
            ..Default::default()
        });
        // Two variants: opaque strokes are drawn without blending, which lets tile-based GPUs
        // (Mali Forward Pixel Kill, PowerVR HSR) skip fragments hidden by later strokes.
        // Translucent strokes use the stencil buffer so that a stroke overlapping itself (joins,
        // frozen parts while drawing) is blended only once per pixel. Each translucent stroke
        // gets its own reference value; a pixel already at that value is skipped.
        let no_stencil = wgpu::DepthStencilState {
            format: STENCIL_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: wgpu::StencilState::default(),
            bias: Default::default(),
        };
        let once = wgpu::StencilFaceState {
            compare: wgpu::CompareFunction::NotEqual,
            fail_op: wgpu::StencilOperation::Keep,
            depth_fail_op: wgpu::StencilOperation::Keep,
            pass_op: wgpu::StencilOperation::Replace,
        };
        let stencil_once = wgpu::DepthStencilState {
            stencil: wgpu::StencilState { front: once, back: once, read_mask: 0xff, write_mask: 0xff },
            ..no_stencil.clone()
        };
        let make_pipeline = |label, blend, ds: &wgpu::DepthStencilState| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: VERTEX_SIZE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &INSTANCE_ATTRS,
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(ds.clone()),
            multisample: wgpu::MultisampleState { count: MSAA, ..Default::default() },
            multiview_mask: None,
            cache: None,
        });
        let pipeline = make_pipeline("stroke", Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), &stencil_once);
        let pipeline_opaque = make_pipeline("stroke opaque", None, &no_stencil);
        // dst = dst · (1 − coverage): an eraser of alpha 1 removes everything under it.
        let erase = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline_erase = make_pipeline("stroke erase", Some(wgpu::BlendState { color: erase, alpha: erase }), &no_stencil);

        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let composite_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite pl"),
            bind_group_layouts: &[Some(&composite_bgl)],
            ..Default::default()
        });
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"),
            layout: Some(&composite_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let image_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image"),
            source: wgpu::ShaderSource::Wgsl(include_str!("image.wgsl").into()),
        });
        let image_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let image_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("image pl"),
            bind_group_layouts: &[Some(&bgl), Some(&image_bgl)],
            ..Default::default()
        });
        let image_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("image"),
            layout: Some(&image_layout),
            vertex: wgpu::VertexState {
                module: &image_shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<ImageVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &image_shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let image_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("image"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let grid_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grid"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let grid_view = grid_tex.create_view(&Default::default());
        let grid_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grid"),
            layout: &image_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&grid_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&image_sampler) },
            ],
        });
        // (The texel is written on the first frame, when a queue is at hand.)
        // egui is drawn in the (single-sampled) screen pass; it antialiases by itself.
        let egui = egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions { msaa_samples: 1, ..Default::default() });
        let mk = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
        };
        let vu = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST;
        let iu = wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST;
        Self {
            pipeline,
            pipeline_opaque,
            pipeline_erase,
            composite_pipeline,
            composite_bgl,
            layer_opacity_buf: mk("layer opacity", 64 * 16, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST),
            image_pipeline,
            image_bgl,
            image_sampler,
            image_textures: HashMap::new(),
            grid_bind_group,
            grid_texture: grid_tex,
            grid_ready: false,
            image_vbuf: mk("image v", 256 * std::mem::size_of::<ImageVertex>() as u64, vu),
            image_v_cap: 256,
            uniform_buf,
            bind_group,
            targets: None,
            cache_targets: None,
            scene_cache: None,
            chunks: Vec::new(),
            meshes: HashMap::new(),
            workers: Workers::new(),
            cpu_meshes: HashMap::new(),
            instance_buf: mk("instances", 1024 * std::mem::size_of::<Instance>() as u64, vu),
            instance_cap: 1024,
            dyn_vbuf: mk("dyn v", 65536 * VERTEX_SIZE, vu),
            dyn_ibuf: mk("dyn i", 65536 * 4, iu),
            dyn_v_cap: 65536,
            dyn_i_cap: 65536,
            frame: 0,
            egui,
            srgb: format.is_srgb(),
            needs_another_frame: false,
            timings: Timings::default(),
            unlimited_budget: false,
        }
    }

    fn color(&self, c: [u8; 4]) -> [f32; 4] {
        let conv = |v: u8| {
            let v = v as f32 / 255.0;
            if self.srgb { if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) } } else { v }
        };
        let a = c[3] as f32 / 255.0;
        // Premultiplied alpha.
        [conv(c[0]) * a, conv(c[1]) * a, conv(c[2]) * a, a]
    }

    /// Uploads a photo with a full mip chain (so it doesn't shimmer when zoomed out).
    fn create_image_texture(&self, device: &wgpu::Device, queue: &wgpu::Queue, rgba: &image::RgbaImage) -> wgpu::BindGroup {
        let max = device.limits().max_texture_dimension_2d;
        let mut level = if rgba.width() > max || rgba.height() > max {
            let k = max as f32 / rgba.width().max(rgba.height()) as f32;
            image::imageops::resize(rgba, ((rgba.width() as f32 * k) as u32).max(1), ((rgba.height() as f32 * k) as u32).max(1), image::imageops::FilterType::Triangle)
        } else {
            rgba.clone()
        };
        let (w, h) = level.dimensions();
        let mips = 32 - w.max(h).leading_zeros();
        let format = if self.srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("photo"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for mip in 0..mips {
            let (lw, lh) = level.dimensions();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: mip, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                level.as_raw(),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(lw * 4), rows_per_image: Some(lh) },
                wgpu::Extent3d { width: lw, height: lh, depth_or_array_layers: 1 },
            );
            if mip + 1 < mips {
                level = image::imageops::resize(&level, (lw / 2).max(1), (lh / 2).max(1), image::imageops::FilterType::Triangle);
            }
        }
        let view = tex.create_view(&Default::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("photo"),
            layout: &self.image_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.image_sampler) },
            ],
        })
    }

    /// Instance colour, grain parameters and how to draw it. Erasers draw with alpha 1 and
    /// clear the layer.
    fn paint(&self, s: &Stroke) -> ([f32; 4], [f32; 4], Kind) {
        if s.is_eraser() {
            return ([0.0, 0.0, 0.0, 1.0], [0.0; 4], Kind::Erase);
        }
        let mut c = s.color;
        c[3] = (c[3] as f32 * s.brush.opacity()).round() as u8;
        let color = self.color(c);
        let params = match s.brush.grain() {
            Some((strength, cell)) => [strength, 1.0 / cell, 0.0, 0.0],
            None => [0.0; 4],
        };
        let kind = if color[3] >= 1.0 && s.brush.is_solid() { Kind::Opaque } else { Kind::Blend };
        (color, params, kind)
    }

    fn upload_mesh(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Mesh) -> Option<(usize, i32, u32)> {
        let nv = mesh.vertices.len() as u64;
        let ni = mesh.indices.len() as u64;
        if ni == 0 {
            return None;
        }
        let pos = self.chunks.iter().position(|c| c.v_cap - c.v_used >= nv && c.i_cap - c.i_used >= ni);
        let ci = match pos {
            Some(i) => i,
            None => {
                let v_cap = CHUNK_VERTICES.max(nv);
                let i_cap = CHUNK_INDICES.max(ni);
                let chunk = Chunk {
                    vbuf: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("mesh v"),
                        size: v_cap * VERTEX_SIZE,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    ibuf: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("mesh i"),
                        size: i_cap * 4,
                        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    v_cap,
                    i_cap,
                    v_used: 0,
                    i_used: 0,
                    live: 0,
                };
                self.chunks.push(chunk);
                self.chunks.len() - 1
            }
        };
        let c = &mut self.chunks[ci];
        queue.write_buffer(&c.vbuf, c.v_used * VERTEX_SIZE, bytemuck::cast_slice(&mesh.vertices));
        // Index buffer writes must be 4-byte aligned in size: u32 indices always are.
        queue.write_buffer(&c.ibuf, c.i_used * 4, bytemuck::cast_slice(&mesh.indices));
        let r = (ci, c.v_used as i32, c.i_used as u32);
        c.v_used += nv;
        c.i_used += ni;
        c.live += 1;
        Some(r)
    }

    fn evict(&mut self) {
        let frame = self.frame;
        self.image_textures.retain(|_, t| frame - t.last_used < EVICT_FRAMES);
        let chunks = &mut self.chunks;
        self.meshes.retain(|_, m| {
            let keep = frame - m.last_used < EVICT_FRAMES;
            if !keep {
                chunks[m.chunk].live -= 1;
            }
            keep
        });
        for c in chunks.iter_mut() {
            if c.live == 0 {
                c.v_used = 0;
                c.i_used = 0;
            }
        }
        self.cpu_meshes.retain(|_, (_, used)| frame - *used < EVICT_FRAMES);
    }

    /// Forget every cached mesh of a stroke (e.g. after it changed).
    pub fn clear_cache(&mut self) {
        self.image_textures.clear();
        self.workers.generation += 1;
        self.workers.pending.clear();
        self.meshes.clear();
        self.cpu_meshes.clear();
        for c in &mut self.chunks {
            c.live = 0;
            c.v_used = 0;
            c.i_used = 0;
        }
    }

    fn apply_textures(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, t: &mut egui::TexturesDelta) {
        for (id, deltas) in &t.set {
            for delta in deltas {
                self.egui.update_texture(device, queue, *id, delta);
            }
        }
        for id in &t.free {
            self.egui.free_texture(id);
        }
        t.clear();
    }

    pub fn render_target(&mut self, target: Target, input: FrameInput) {
        match target {
            Target::Surface(gpu) => self.render(gpu, input),
            Target::View { device, queue, format, size, view } => self.render_to(device, queue, format, size, view, input),
        }
    }

    /// Renders the scene (no UI) into the scene cache texture of `size`.
    pub fn render_scene_cache(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, size: [u32; 2], input: FrameInput) {
        if self.scene_cache.as_ref().is_none_or(|c| c.size != size) {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("scene cache"),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scene cache"),
                layout: &self.image_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.image_sampler) },
                ],
            });
            self.scene_cache = Some(SceneCache { view, bind_group, size });
        }
        let view = self.scene_cache.as_ref().expect("created").view.clone();
        std::mem::swap(&mut self.targets, &mut self.cache_targets);
        self.render_to(device, queue, format, size, &view, input);
        std::mem::swap(&mut self.targets, &mut self.cache_targets);
    }

    /// Renders a complete frame offscreen and returns straight RGBA pixels (row-major).
    /// `size[0]` must be a multiple of 64 (texture copy row alignment).
    pub fn render_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        size: [u32; 2],
        input: FrameInput,
    ) -> Vec<u8> {
        debug_assert!(size[0].is_multiple_of(64));
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        self.unlimited_budget = true;
        // The main targets are sized for the window; keep them for the next frame.
        // Own targets for the export size; the window's stay for the next frame.
        let window_targets = self.targets.take();
        self.render_to(device, queue, format, size, &view, input);
        self.targets = window_targets;
        self.unlimited_budget = false;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (size[0] * size[1] * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size[0] * 4), rows_per_image: Some(size[1]) },
            },
            wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        if let Err(e) = device.poll(wgpu::PollType::wait_indefinitely()) {
            log::warn!("offscreen render: {e}");
            return Vec::new();
        }
        let Ok(mapped) = slice.get_mapped_range() else { return Vec::new() };
        let mut data = mapped.to_vec();
        drop(mapped);
        buf.unmap();
        if matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb) {
            for px in data.as_chunks_mut::<4>().0 {
                px.swap(0, 2);
            }
        }
        data
    }

    /// Renders one frame to the window surface and presents it.
    pub fn render(&mut self, gpu: &Gpu, mut input: FrameInput) {
        let t0 = Instant::now();
        let acquired = gpu.surface.get_current_texture();
        self.timings.acquire = t0.elapsed().as_secs_f64();
        let frame = match acquired {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            other => {
                if matches!(other, wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost) {
                    gpu.surface.configure(&gpu.device, &gpu.config);
                }
                self.apply_textures(&gpu.device, &gpu.queue, &mut input.textures);
                self.needs_another_frame = true;
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let size = [gpu.config.width, gpu.config.height];
        self.render_to(&gpu.device, &gpu.queue, gpu.config.format, size, &view, input);
        let t1 = Instant::now();
        gpu.queue.present(frame);
        self.timings.present = t1.elapsed().as_secs_f64();
    }

    /// Renders one frame into `view` (any render-attachment texture of `format`).
    pub fn render_to(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        size: [u32; 2],
        view: &wgpu::TextureView,
        mut input: FrameInput,
    ) {
        self.frame += 1;
        self.apply_textures(device, queue, &mut input.textures);
        self.needs_another_frame = false;
        // Meshes finished in the background since the last frame.
        while let Ok((generation, id, lod, mesh)) = self.workers.results.try_recv() {
            self.workers.pending.remove(&(id, lod));
            if generation != self.workers.generation || self.meshes.contains_key(&(id, lod)) {
                continue;
            }
            if let Some((chunk, base_vertex, first_index)) = self.upload_mesh(device, queue, &mesh) {
                self.meshes.insert((id, lod), CachedMesh { chunk, base_vertex, first_index, index_count: mesh.indices.len() as u32, last_used: self.frame });
            }
        }
        if !self.workers.pending.is_empty() {
            self.needs_another_frame = true;
        }
        let vp = [size[0] as f64, size[1] as f64];
        let rot = input.rotation;
        let rs = |scale: f64| [(scale * rot[0]) as f32, (scale * rot[1]) as f32];

        let mut instances: Vec<Instance> = Vec::with_capacity(input.visible.len() + 1);
        let mut kinds: Vec<Kind> = Vec::with_capacity(input.visible.len() + 1);
        // Layer position of each draw.
        let mut draw_layers: Vec<u16> = Vec::with_capacity(input.visible.len() + 1);
        let mut requests: Vec<(f64, StrokeId, u32)> = Vec::new();
        let mut draws: Vec<Draw> = Vec::with_capacity(input.visible.len() + 1);
        let mut dyn_v: Vec<[f32; 3]> = Vec::new();
        let mut dyn_i: Vec<u32> = Vec::new();
        let started = Instant::now();

        for v in input.visible {
            let Some(stroke) = input.doc.get(v.id) else { continue };
            let rel = input.log_zoom - v.level as f64;
            let lod = lod_for(v.level, input.log_zoom);
            let (color, grain, kind) = self.paint(stroke);
            if rel <= GPU_MAX_REL {
                // Exact LOD, or a slightly finer one already cached.
                let mut use_lod = (lod..=(lod + LOD_SLACK).min(MAX_LOD)).find(|l| self.meshes.contains_key(&(v.id, *l)));
                if use_lod.is_none() {
                    // Closest cached LOD, finer ones first: shown until the right one is ready.
                    let fallback = (0..=MAX_LOD).filter(|l| self.meshes.contains_key(&(v.id, *l))).min_by_key(|l| (*l < lod, l.abs_diff(lod)));
                    let sync = self.unlimited_budget
                        || input.urgent.contains(&v.id)
                        || (fallback.is_none() && stroke.points.len() <= SYNC_POINTS && started.elapsed() < TESSELLATION_BUDGET);
                    if sync {
                        let mesh = tessellate(stroke, lod);
                        if let Some((chunk, base_vertex, first_index)) = self.upload_mesh(device, queue, &mesh) {
                            let m = CachedMesh { chunk, base_vertex, first_index, index_count: mesh.indices.len() as u32, last_used: self.frame };
                            self.meshes.insert((v.id, lod), m);
                            use_lod = Some(lod);
                        }
                    } else {
                        // In the background; meanwhile show whatever LOD there is.
                        let b = stroke.cache.bbox;
                        let area = ((b[2] - b[0]) as f64 * (b[3] - b[1]) as f64) * v.scale * v.scale;
                        requests.push((area, v.id, lod));
                        self.needs_another_frame = true;
                        use_lod = fallback;
                    }
                }
                let cached = use_lod.and_then(|l| {
                    let m = self.meshes.get_mut(&(v.id, l))?;
                    m.last_used = self.frame;
                    Some(*m)
                });
                if let Some(m) = cached {
                    let instance = instances.len() as u32;
                    instances.push(Instance { offset: f32x2(v.offset), rs: rs(v.scale), color, params: grain });
                    kinds.push(kind);
                    draw_layers.push(v.layer);
                    draws.push(Draw::Cached {
                        chunk: m.chunk,
                        base_vertex: m.base_vertex,
                        first_index: m.first_index,
                        count: m.index_count,
                        instance,
                    });
                }
            } else {
                let key = (v.id, lod);
                let mesh = match self.cpu_meshes.get_mut(&key) {
                    Some((m, used)) => {
                        *used = self.frame;
                        m.clone()
                    }
                    None => {
                        let m = Arc::new(tessellate(stroke, lod));
                        self.cpu_meshes.insert(key, (m.clone(), self.frame));
                        m
                    }
                };
                let base = dyn_v.len();
                let first = dyn_i.len();
                push_cpu_path(&mesh, v.offset, v.scale, rot, vp, &mut dyn_v, &mut dyn_i);
                if dyn_i.len() > first {
                    let instance = instances.len() as u32;
                    // Vertices are already in screen space here, so there are no local coordinates
                    // for the grain; at this magnification it is faded out anyway.
                    instances.push(Instance { offset: [0.0, 0.0], rs: [1.0, 0.0], color, params: [0.0; 4] });
                    kinds.push(kind);
                    draw_layers.push(v.layer);
                    draws.push(Draw::Dynamic {
                        base_vertex: base as i32,
                        first_index: first as u32,
                        count: (dyn_i.len() - first) as u32,
                        instance,
                    });
                    // Indices were written relative to `base`.
                }
            }
        }

        // Background jobs, biggest on screen first (they matter most when zooming out).
        requests.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, id, lod) in requests {
            if let Some(stroke) = input.doc.get(id) {
                self.workers.request(stroke, lod);
            }
        }

        let t_active = Instant::now();
        if let Some(a) = input.active {
            let lod = lod_for(a.stroke.level, input.log_zoom).min(2);
            let from = a.tail_from.min(a.stroke.points.len().saturating_sub(1));
            let tail = match a.predicted {
                Some(p) => {
                    // The tail plus the predicted tip, as a throwaway stroke.
                    let s = a.stroke;
                    let mut t = Stroke::new(s.id, s.level, s.anchor.clone(), s.color, s.width);
                    t.brush = s.brush;
                    t.flat_caps = [s.flat_caps[0] && from == 0, false];
                    t.points = s.points[from..].to_vec();
                    t.points.push(p);
                    t.refresh_cache();
                    tessellate_part(&t, 0..t.points.len(), lod)
                }
                None => tessellate_part(a.stroke, from..a.stroke.points.len(), lod),
            };
            let base = dyn_v.len();
            let first = dyn_i.len();
            for mesh in a.frozen.iter().chain(std::iter::once(&tail)) {
                let b = (dyn_v.len() - base) as u32;
                dyn_v.extend_from_slice(&mesh.vertices);
                dyn_i.extend(mesh.indices.iter().map(|i| i + b));
            }
            if dyn_i.len() > first {
                let (color, params, kind) = self.paint(a.stroke);
                let instance = instances.len() as u32;
                instances.push(Instance { offset: f32x2(a.offset), rs: rs(a.scale), color, params });
                kinds.push(kind);
                draw_layers.push(a.layer);
                draws.push(Draw::Dynamic {
                    base_vertex: base as i32,
                    first_index: first as u32,
                    count: (dyn_i.len() - first) as u32,
                    instance,
                });
            }
        }

        self.timings.active = t_active.elapsed().as_secs_f64();

        // Photos: corners in f64, clipped to a guard band, then f32 (exact at any zoom).
        let mut image_v: Vec<ImageVertex> = Vec::new();
        let mut image_draws: Vec<(u64, u32, u32)> = Vec::new();
        let guard = [-vp[0], -vp[1], 2.0 * vp[0], 2.0 * vp[1]];
        // Grid lines first (they lie on the background).
        if !input.grid.is_empty() {
            if !self.grid_ready {
                queue.write_texture(
                    self.grid_texture.as_image_copy(),
                    &[90, 90, 100, 255],
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
                    wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                );
                self.grid_ready = true;
            }
            let first = image_v.len() as u32;
            let half = (vp[0].max(vp[1]) / 1400.0).max(0.6);
            for (a, b, strong) in input.grid {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l = dx.hypot(dy).max(1e-9);
                let n = [-dy / l * half, dx / l * half];
                let quad = [[a[0] + n[0], a[1] + n[1], 0.5, 0.5], [b[0] + n[0], b[1] + n[1], 0.5, 0.5], [b[0] - n[0], b[1] - n[1], 0.5, 0.5], [a[0] - n[0], a[1] - n[1], 0.5, 0.5]];
                let poly = clip_polygon4(&quad, guard);
                let opacity = if *strong { 0.32 } else { 0.13 };
                for k in 1..poly.len().saturating_sub(1) {
                    for q in [poly[0], poly[k], poly[k + 1]] {
                        image_v.push(ImageVertex { pos: [q[0] as f32, q[1] as f32], uv: [0.5, 0.5], opacity });
                    }
                }
            }
            image_draws.push((GRID_ID, first, image_v.len() as u32));
        }
        if let (Some(c), true) = (input.scene, self.scene_cache.is_some()) {
            let quad = [[c[0][0], c[0][1], 0.0, 0.0], [c[1][0], c[1][1], 1.0, 0.0], [c[2][0], c[2][1], 1.0, 1.0], [c[3][0], c[3][1], 0.0, 1.0]];
            let poly = clip_polygon4(&quad, guard);
            if poly.len() >= 3 {
                let first = image_v.len() as u32;
                for k in 1..poly.len() - 1 {
                    for q in [poly[0], poly[k], poly[k + 1]] {
                        image_v.push(ImageVertex { pos: [q[0] as f32, q[1] as f32], uv: [q[2] as f32, q[3] as f32], opacity: 1.0 });
                    }
                }
                image_draws.push((SCENE_CACHE_ID, first, image_v.len() as u32));
            }
        }
        for img in input.images {
            let c = img.corners;
            let (x0, x1) = (c.iter().map(|p| p[0]).fold(f64::MAX, f64::min), c.iter().map(|p| p[0]).fold(f64::MIN, f64::max));
            let (y0, y1) = (c.iter().map(|p| p[1]).fold(f64::MAX, f64::min), c.iter().map(|p| p[1]).fold(f64::MIN, f64::max));
            if x1 < 0.0 || y1 < 0.0 || x0 > vp[0] || y0 > vp[1] || (x1 - x0) * (y1 - y0) < 1.0 || img.opacity <= 0.0 {
                continue;
            }
            let quad = [[c[0][0], c[0][1], 0.0, 0.0], [c[1][0], c[1][1], 1.0, 0.0], [c[2][0], c[2][1], 1.0, 1.0], [c[3][0], c[3][1], 0.0, 1.0]];
            let poly = clip_polygon4(&quad, guard);
            if poly.len() < 3 {
                continue;
            }
            if !self.image_textures.contains_key(&img.id) {
                let bind_group = self.create_image_texture(device, queue, img.rgba);
                self.image_textures.insert(img.id, ImageTexture { bind_group, last_used: self.frame });
            }
            if let Some(t) = self.image_textures.get_mut(&img.id) {
                t.last_used = self.frame;
            }
            let first = image_v.len() as u32;
            for k in 1..poly.len() - 1 {
                for q in [poly[0], poly[k], poly[k + 1]] {
                    image_v.push(ImageVertex { pos: [q[0] as f32, q[1] as f32], uv: [q[2] as f32, q[3] as f32], opacity: img.opacity });
                }
            }
            image_draws.push((img.id, first, image_v.len() as u32));
        }
        // Upload per-frame data, growing buffers when needed.
        let grow = |buf: &mut wgpu::Buffer, cap: &mut u64, need: u64, elem: u64, usage, label| {
            if need > *cap {
                *cap = need.next_power_of_two();
                *buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: *cap * elem,
                    usage,
                    mapped_at_creation: false,
                });
            }
        };
        let vu = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST;
        let iu = wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST;
        grow(&mut self.instance_buf, &mut self.instance_cap, instances.len() as u64, std::mem::size_of::<Instance>() as u64, vu, "instances");
        grow(&mut self.dyn_vbuf, &mut self.dyn_v_cap, dyn_v.len() as u64, VERTEX_SIZE, vu, "dyn v");
        grow(&mut self.image_vbuf, &mut self.image_v_cap, image_v.len() as u64, std::mem::size_of::<ImageVertex>() as u64, vu, "image v");
        if !image_v.is_empty() {
            queue.write_buffer(&self.image_vbuf, 0, bytemuck::cast_slice(&image_v));
        }
        grow(&mut self.dyn_ibuf, &mut self.dyn_i_cap, dyn_i.len() as u64, 4, iu, "dyn i");
        if !instances.is_empty() {
            queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&instances));
        }
        if !dyn_v.is_empty() {
            queue.write_buffer(&self.dyn_vbuf, 0, bytemuck::cast_slice(&dyn_v));
            queue.write_buffer(&self.dyn_ibuf, 0, bytemuck::cast_slice(&dyn_i));
        }
        queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&Uniforms { viewport: [size[0] as f32, size[1] as f32], _pad: [0.0; 2] }),
        );

        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            let mk = |label, samples, usage, format| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: samples,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            let msaa = mk("msaa", MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT, format);
            let stencil = mk("stencil", MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT, STENCIL_FORMAT);
            let layer = mk("layer", 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, format);
            let composite = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("composite"),
                layout: &self.composite_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&layer) },
                    wgpu::BindGroupEntry { binding: 1, resource: self.layer_opacity_buf.as_entire_binding() },
                ],
            });
            self.targets = Some(Targets { size, msaa, stencil, layer, composite });
        }
        let targets = self.targets.as_ref().expect("just created");

        self.timings.build = started.elapsed().as_secs_f64();
        self.timings.triangles = draws
            .iter()
            .map(|d| match *d {
                Draw::Cached { count, .. } | Draw::Dynamic { count, .. } => count as usize / 3,
            })
            .sum();
        let t_submit = Instant::now();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        let mut egui_cmds = Vec::new();
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size,
            pixels_per_point: input.egui.map(|e| e.1).unwrap_or(1.0),
        };
        if let Some((prims, _)) = input.egui {
            egui_cmds = self.egui.update_buffers(device, queue, &mut encoder, prims, &screen);
        }

        // Layer opacities for the composite shader.
        let mut ops = [[0.0f32; 4]; 64];
        for (i, o) in ops.iter_mut().enumerate() {
            o[0] = input.layer_opacity.get(i).copied().unwrap_or(1.0);
        }
        queue.write_buffer(&self.layer_opacity_buf, 0, bytemuck::cast_slice(&ops));
        // Draw order: layer by layer (bottom first), drawing order within a layer.
        let mut order: Vec<usize> = (0..draws.len()).collect();
        order.sort_by_key(|&i| draw_layers[i]);

        // 1. Background and photos.
        {
            let bg = input.background;
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("background"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r: bg[0] as f64, g: bg[1] as f64, b: bg[2] as f64, a: bg[3] as f64 }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            if !image_draws.is_empty() {
                pass.set_pipeline(&self.image_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.image_vbuf.slice(..));
                for (id, a, b) in &image_draws {
                    let bg = if *id == SCENE_CACHE_ID {
                        &self.scene_cache.as_ref().expect("checked").bind_group
                    } else if *id == GRID_ID {
                        &self.grid_bind_group
                    } else {
                        &self.image_textures[id].bind_group
                    };
                    pass.set_bind_group(1, bg, &[]);
                    pass.draw(*a..*b, 0..1);
                }
            }
        }

        // 2. Each layer: its strokes into the transparent layer texture (MSAA, resolved), where
        // its erasers clear pixels; then that texture over the screen with the layer's opacity.
        let mut i = 0;
        while i < order.len() {
            let layer = draw_layers[order[i]];
            let mut j = i;
            while j < order.len() && draw_layers[order[j]] == layer {
                j += 1;
            }
            {
                let mut pass = encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("strokes"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: if MSAA > 1 { &targets.msaa } else { &targets.layer },
                            depth_slice: None,
                            resolve_target: if MSAA > 1 { Some(&targets.layer) } else { None },
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: if MSAA > 1 { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &targets.stencil,
                            depth_ops: None,
                            stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
                        }),
                        ..Default::default()
                    })
                    .forget_lifetime();
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(1, self.instance_buf.slice(..));
                let mut stencil_ref = 0u32;
                let mut bound: Option<usize> = None; // usize::MAX = dynamic buffers
                let mut kind_bound: Option<Kind> = None;
                for &k in &order[i..j] {
                    let d = &draws[k];
                    let (Draw::Cached { instance, .. } | Draw::Dynamic { instance, .. }) = *d;
                    let kind = kinds[instance as usize];
                    if kind_bound != Some(kind) {
                        pass.set_pipeline(match kind {
                            Kind::Opaque => &self.pipeline_opaque,
                            Kind::Blend => &self.pipeline,
                            Kind::Erase => &self.pipeline_erase,
                        });
                        kind_bound = Some(kind);
                    }
                    if kind == Kind::Blend {
                        // 1..=255, cycling. A repeat can only clash with a stroke 255 translucent
                        // strokes earlier at the same pixel that nothing translucent covered since.
                        stencil_ref = stencil_ref % 255 + 1;
                        pass.set_stencil_reference(stencil_ref);
                    }
                    match *d {
                        Draw::Cached { chunk, base_vertex, first_index, count, instance } => {
                            if bound != Some(chunk) {
                                pass.set_vertex_buffer(0, self.chunks[chunk].vbuf.slice(..));
                                pass.set_index_buffer(self.chunks[chunk].ibuf.slice(..), wgpu::IndexFormat::Uint32);
                                bound = Some(chunk);
                            }
                            pass.draw_indexed(first_index..first_index + count, base_vertex, instance..instance + 1);
                        }
                        Draw::Dynamic { base_vertex, first_index, count, instance } => {
                            if bound != Some(usize::MAX) {
                                pass.set_vertex_buffer(0, self.dyn_vbuf.slice(..));
                                pass.set_index_buffer(self.dyn_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                                bound = Some(usize::MAX);
                            }
                            pass.draw_indexed(first_index..first_index + count, base_vertex, instance..instance + 1);
                        }
                    }
                }
            }
            {
                let mut pass = encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("composite"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                        })],
                        ..Default::default()
                    })
                    .forget_lifetime();
                pass.set_pipeline(&self.composite_pipeline);
                pass.set_bind_group(0, &targets.composite, &[]);
                let l = layer as u32;
                pass.draw(0..3, l..l + 1);
            }
            i = j;
        }

        // 3. The UI on top.
        if let Some((prims, _)) = input.egui {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.egui.render(&mut pass, prims, &screen);
        }
        queue.submit(egui_cmds.into_iter().chain(std::iter::once(encoder.finish())));
        self.timings.submit = t_submit.elapsed().as_secs_f64();
        if self.frame.is_multiple_of(120) {
            self.evict();
        }
    }
}

fn f32x2(v: [f64; 2]) -> [f32; 2] {
    [v[0] as f32, v[1] as f32]
}

/// Transforms a mesh to screen space in f64 and clips it to a guard band around the viewport.
/// Output vertices are in screen pixels; indices are relative to the first vertex pushed.
fn push_cpu_path(
    mesh: &Mesh,
    offset: [f64; 2],
    scale: f64,
    rot: [f64; 2],
    vp: [f64; 2],
    out_v: &mut Vec<[f32; 3]>,
    out_i: &mut Vec<u32>,
) {
    let base = out_v.len() as u32;
    let guard = [-vp[0], -vp[1], 2.0 * vp[0], 2.0 * vp[1]];
    let tf = |i: u32| {
        let v = mesh.vertices[i as usize];
        let r = canvas_core::camera::rotate([v[0] as f64 * scale, v[1] as f64 * scale], rot);
        [offset[0] + r[0], offset[1] + r[1], v[2] as f64]
    };
    for tri in mesh.indices.as_chunks::<3>().0 {
        let p = [tf(tri[0]), tf(tri[1]), tf(tri[2])];
        let outside = |axis: usize, lo: bool| {
            p.iter().all(|q| if lo { q[axis] < guard[axis] } else { q[axis] > guard[axis + 2] })
        };
        if outside(0, true) || outside(0, false) || outside(1, true) || outside(1, false) {
            continue;
        }
        let inside = p.iter().all(|q| q[0] >= guard[0] && q[0] <= guard[2] && q[1] >= guard[1] && q[1] <= guard[3]);
        let poly: Vec<[f64; 3]> = if inside { p.to_vec() } else { clip_polygon(&p, guard) };
        if poly.len() < 3 {
            continue;
        }
        let first = out_v.len() as u32 - base;
        out_v.extend(poly.iter().map(|q| [q[0] as f32, q[1] as f32, q[2] as f32]));
        for k in 1..poly.len() as u32 - 1 {
            out_i.extend_from_slice(&[first, first + k, first + k + 1]);
        }
    }
}

/// [`clip_polygon`] for vertices with two attributes (x, y, u, v).
fn clip_polygon4(poly: &[[f64; 4]], r: [f64; 4]) -> Vec<[f64; 4]> {
    let mut out = poly.to_vec();
    for (axis, bound, greater) in [(0, r[0], true), (0, r[2], false), (1, r[1], true), (1, r[3], false)] {
        if out.is_empty() {
            break;
        }
        let input = std::mem::take(&mut out);
        let ins = |p: &[f64; 4]| if greater { p[axis] >= bound } else { p[axis] <= bound };
        for i in 0..input.len() {
            let a = input[i];
            let b = input[(i + 1) % input.len()];
            let (ia, ib) = (ins(&a), ins(&b));
            if ia {
                out.push(a);
            }
            if ia != ib {
                let t = (bound - a[axis]) / (b[axis] - a[axis]);
                out.push(std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t));
            }
        }
    }
    out
}

/// Sutherland–Hodgman clipping of a convex polygon against an axis-aligned rectangle
/// (x, y; the third component is an attribute, interpolated along cut edges).
fn clip_polygon(poly: &[[f64; 3]], r: [f64; 4]) -> Vec<[f64; 3]> {
    let mut out = poly.to_vec();
    // (axis, bound, keep_greater)
    for (axis, bound, greater) in [(0, r[0], true), (0, r[2], false), (1, r[1], true), (1, r[3], false)] {
        if out.is_empty() {
            break;
        }
        let input = std::mem::take(&mut out);
        let ins = |p: &[f64; 3]| if greater { p[axis] >= bound } else { p[axis] <= bound };
        for i in 0..input.len() {
            let a = input[i];
            let b = input[(i + 1) % input.len()];
            let (ia, ib) = (ins(&a), ins(&b));
            if ia {
                out.push(a);
            }
            if ia != ib {
                let t = (bound - a[axis]) / (b[axis] - a[axis]);
                out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_triangle_to_rect() {
        let tri = [[-10.0, 5.0, 1.0], [20.0, 5.0, 1.0], [5.0, 30.0, 0.0]];
        let out = clip_polygon(&tri, [0.0, 0.0, 10.0, 10.0]);
        assert!(out.len() >= 3);
        for p in out {
            assert!((0.0..=1.0).contains(&p[2]));
            assert!(p[0] >= -1e-9 && p[0] <= 10.0 + 1e-9 && p[1] >= -1e-9 && p[1] <= 10.0 + 1e-9);
        }
    }
}
