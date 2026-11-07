use std::collections::HashMap;

/// Formato del lienzo: RGBA con valores sRGB (igual que las texturas de egui).
pub const CANVAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Imagen de una capa. `key` identifica la capa entre cuadros (para reusar su
/// textura) y `version` cambia cuando cambia el contenido de la imagen.
pub struct LayerSource<'a> {
    pub key: u64,
    pub version: u64,
    pub width: u32,
    pub height: u32,
    /// RGBA8, alfa no premultiplicado, filas compactas.
    pub rgba: &'a [u8],
}

/// LUT 3D de una capa. `key` identifica la LUT (para reusar su textura).
pub struct LutSource<'a> {
    pub key: u64,
    pub size: u32,
    /// `size³` colores RGBA8, rojo variando más rápido.
    pub rgba: &'a [u8],
}

/// Corrección de color de una capa (ver `snipsy_core::ColorGrade`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grade {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
    pub lut_mix: f32,
}

impl Grade {
    pub const NEUTRAL: Grade = Grade { exposure: 0.0, contrast: 1.0, saturation: 1.0, temperature: 0.0, tint: 0.0, lut_mix: 0.0 };
}

impl Default for Grade {
    fn default() -> Self {
        Self::NEUTRAL
    }
}

pub struct Layer<'a> {
    pub source: LayerSource<'a>,
    /// Esquinas en píxeles del lienzo: sup-izq, sup-der, inf-der, inf-izq.
    pub corners: [[f32; 2]; 4],
    pub opacity: f32,
    pub grade: Grade,
    pub lut: Option<LutSource<'a>>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    opacity: f32,
    /// exposición, contraste, saturación, intensidad de la LUT
    grade_a: [f32; 4],
    /// temperatura, tinte, tamaño de la LUT, 0
    grade_b: [f32; 4],
}

struct LutTexture {
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

/// Clave de la LUT neutra que usan las capas sin LUT.
const IDENTITY_LUT: u64 = u64::MAX;

struct LayerTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
    version: u64,
}

struct Canvas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

const SHADER: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) opacity: f32,
    @location(2) grade_a: vec4<f32>,
    @location(3) grade_b: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) opacity: f32,
    @location(3) grade_a: vec4<f32>,
    @location(4) grade_b: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    out.pos = vec4<f32>(pos, 0.0, 1.0);
    out.uv = uv;
    out.opacity = opacity;
    out.grade_a = grade_a;
    out.grade_b = grade_b;
    return out;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(1) @binding(0) var lut: texture_3d<f32>;
@group(1) @binding(1) var lut_samp: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let src = textureSample(tex, samp, in.uv);
    var c = src.rgb;
    let exposure = in.grade_a.x;
    let contrast = in.grade_a.y;
    let saturation = in.grade_a.z;
    let lut_mix = in.grade_a.w;
    let temperature = in.grade_b.x;
    let tint = in.grade_b.y;
    let n = max(in.grade_b.z, 2.0);

    // 1. LUT (look) con interpolación trilineal.
    let coord = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)) * ((n - 1.0) / n) + vec3<f32>(0.5 / n);
    let looked = textureSampleLevel(lut, lut_samp, coord, 0.0).rgb;
    c = mix(c, looked, lut_mix);

    // 2. Exposición y balance de blancos en luz lineal.
    var lin = pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
    lin = lin * exp2(exposure);
    lin = lin * vec3<f32>(1.0 + 0.3 * temperature, 1.0 - 0.25 * tint, 1.0 - 0.3 * temperature);
    c = pow(max(lin, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));

    // 3. Contraste (pivote en gris medio) y saturación.
    c = (c - vec3<f32>(0.5)) * contrast + vec3<f32>(0.5);
    let luma = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    c = mix(vec3<f32>(luma), c, saturation);

    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), src.a * in.opacity);
}
"#;

/// Compone capas (de abajo hacia arriba) sobre un lienzo negro en la GPU.
pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    lut_layout: wgpu::BindGroupLayout,
    luts: HashMap<u64, LutTexture>,
    canvas: Option<Canvas>,
    textures: HashMap<u64, LayerTexture>,
    vertex_buffer: Option<(wgpu::Buffer, u64)>,
}

impl Compositor {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("snipsy_compositor"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("snipsy_layer_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
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
        let lut_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("snipsy_lut_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("snipsy_compositor_layout"),
            bind_group_layouts: &[Some(&bind_layout), Some(&lut_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("snipsy_compositor"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32, 3 => Float32x4, 4 => Float32x4],
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: CANVAS_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("snipsy_layer_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mut compositor = Self {
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            bind_layout,
            sampler,
            lut_layout,
            luts: HashMap::new(),
            canvas: None,
            textures: HashMap::new(),
            vertex_buffer: None,
        };
        let identity = crate::Lut::identity(2);
        compositor.upload_lut(&LutSource { key: IDENTITY_LUT, size: identity.size, rgba: &identity.rgba });
        compositor
    }

    fn upload_lut(&mut self, src: &LutSource) {
        if self.luts.contains_key(&src.key) {
            return;
        }
        let n = src.size.max(2);
        let size = wgpu::Extent3d { width: n, height: n, depth_or_array_layers: n };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("snipsy_lut"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let expected = (n * n * n * 4) as usize;
        if src.rgba.len() >= expected {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &src.rgba[..expected],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * n), rows_per_image: Some(n) },
                size,
            );
        }
        let view = texture.create_view(&Default::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("snipsy_lut"),
            layout: &self.lut_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.luts.insert(src.key, LutTexture { _texture: texture, bind_group });
    }

    pub fn canvas_view(&self) -> Option<&wgpu::TextureView> {
        self.canvas.as_ref().map(|c| &c.view)
    }

    /// Compone las capas en un lienzo de `size`. Devuelve `true` si el lienzo
    /// se recreó (quien lo muestre debe volver a registrarlo).
    pub fn render(&mut self, size: (u32, u32), layers: &[Layer]) -> bool {
        let size = (size.0.max(1), size.1.max(1));
        let recreated = self.canvas.as_ref().is_none_or(|c| c.size != size);
        if recreated {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("snipsy_canvas"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: CANVAS_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.canvas = Some(Canvas { texture, view, size });
        }

        // Subir (o reutilizar) la textura de cada capa.
        let mut used = Vec::with_capacity(layers.len());
        let mut used_luts = vec![IDENTITY_LUT];
        for layer in layers {
            self.upload(&layer.source);
            used.push(layer.source.key);
            if let Some(lut) = &layer.lut {
                self.upload_lut(lut);
                used_luts.push(lut.key);
            }
        }
        self.textures.retain(|k, _| used.contains(k));
        self.luts.retain(|k, _| used_luts.contains(k));

        // Geometría: dos triángulos por capa, en coordenadas normalizadas.
        let to_ndc = |p: [f32; 2]| [p[0] / size.0 as f32 * 2.0 - 1.0, 1.0 - p[1] / size.1 as f32 * 2.0];
        let mut vertices: Vec<Vertex> = Vec::with_capacity(layers.len() * 6);
        for layer in layers {
            let [tl, tr, br, bl] = layer.corners.map(to_ndc);
            let o = layer.opacity.clamp(0.0, 1.0);
            let g = &layer.grade;
            let (lut_mix, lut_size) = match &layer.lut {
                Some(l) => (g.lut_mix.clamp(0.0, 1.0), l.size as f32),
                None => (0.0, 2.0),
            };
            let grade_a = [g.exposure, g.contrast, g.saturation, lut_mix];
            let grade_b = [g.temperature, g.tint, lut_size, 0.0];
            let v = |pos, uv| Vertex { pos, uv, opacity: o, grade_a, grade_b };
            vertices.extend([
                v(tl, [0.0, 0.0]),
                v(tr, [1.0, 0.0]),
                v(br, [1.0, 1.0]),
                v(tl, [0.0, 0.0]),
                v(br, [1.0, 1.0]),
                v(bl, [0.0, 1.0]),
            ]);
        }
        let bytes: &[u8] = bytemuck::cast_slice(&vertices);
        if !bytes.is_empty() {
            let needed = bytes.len() as u64;
            if self.vertex_buffer.as_ref().is_none_or(|(_, cap)| *cap < needed) {
                let cap = needed.next_power_of_two().max(4096);
                let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("snipsy_layer_vertices"),
                    size: cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.vertex_buffer = Some((buffer, cap));
            }
            self.queue.write_buffer(&self.vertex_buffer.as_ref().unwrap().0, 0, bytes);
        }

        let canvas = self.canvas.as_ref().unwrap();
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("snipsy_compose") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("snipsy_compose"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &canvas.view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            if let Some((buffer, _)) = &self.vertex_buffer {
                if !vertices.is_empty() {
                    pass.set_pipeline(&self.pipeline);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    for (i, layer) in layers.iter().enumerate() {
                        let tex = &self.textures[&layer.source.key];
                        pass.set_bind_group(0, &tex.bind_group, &[]);
                        let lut = layer.lut.as_ref().map(|l| l.key).unwrap_or(IDENTITY_LUT);
                        pass.set_bind_group(1, &self.luts[&lut].bind_group, &[]);
                        let first = (i * 6) as u32;
                        pass.draw(first..first + 6, 0..1);
                    }
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        recreated
    }

    fn upload(&mut self, src: &LayerSource) {
        let size = (src.width.max(1), src.height.max(1));
        let existing = self.textures.get(&src.key);
        if existing.is_some_and(|t| t.size == size && t.version == src.version) {
            return;
        }
        if existing.is_none_or(|t| t.size != size) {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("snipsy_layer"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("snipsy_layer"),
                layout: &self.bind_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            self.textures.insert(src.key, LayerTexture { texture, bind_group, size, version: u64::MAX });
        }
        let tex = self.textures.get_mut(&src.key).unwrap();
        let expected = (size.0 * size.1 * 4) as usize;
        if src.rgba.len() >= expected {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &src.rgba[..expected],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * size.0), rows_per_image: Some(size.1) },
                wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            );
        }
        tex.version = src.version;
    }

    /// Copia el lienzo a memoria (RGBA8 compacto). Bloquea hasta que la GPU termine.
    pub fn read_canvas(&self) -> Option<Vec<u8>> {
        let canvas = self.canvas.as_ref()?;
        let (w, h) = canvas.size;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = (w * 4).div_ceil(align) * align;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("snipsy_readback"),
            size: (padded * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("snipsy_readback") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &canvas.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        rx.recv().ok()?.ok()?;
        let data = buffer.slice(..).get_mapped_range().ok()?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h as usize {
            let start = row * padded as usize;
            out.extend_from_slice(&data[start..start + (w * 4) as usize]);
        }
        drop(data);
        buffer.unmap();
        Some(out)
    }
}
