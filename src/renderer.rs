use std::collections::HashMap;
use std::sync::Arc;

use crossfont::{
    BitmapBuffer, FontDesc, FontKey, GlyphKey, Rasterize, Rasterizer, Size, Slant, Style, Weight,
};
use wgpu::util::DeviceExt;

use helix_tui::buffer::Cell;
use helix_view::graphics::{Color, CursorKind, Modifier};

/// ANSI 256-color palette (first 16 colors — standard terminal colors).
/// Extended 216-color cube + 24 grayscale are computed algorithmically.
const ANSI_COLORS: [[u8; 3]; 16] = [
    [0, 0, 0],       // Black
    [205, 49, 49],   // Red
    [13, 188, 121],  // Green
    [229, 229, 16],  // Yellow
    [36, 114, 200],  // Blue
    [188, 63, 188],  // Magenta
    [17, 168, 205],  // Cyan
    [229, 229, 229], // White (light gray)
    [102, 102, 102], // Bright black (dark gray)
    [241, 76, 76],   // Bright red
    [35, 209, 139],  // Bright green
    [245, 245, 67],  // Bright yellow
    [59, 142, 234],  // Bright blue
    [214, 112, 214], // Bright magenta
    [41, 184, 219],  // Bright cyan
    [255, 255, 255], // Bright white
];

fn color_to_rgba(color: Color, default: [f32; 4]) -> [f32; 4] {
    match color {
        Color::Rgb(r, g, b) => [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0],
        Color::Indexed(n) => {
            let (r, g, b) = if n < 16 {
                let c = ANSI_COLORS[n as usize];
                (c[0], c[1], c[2])
            } else if n < 232 {
                // 216-color cube: 6x6x6
                let n = n - 16;
                let b = (n % 6) * 51;
                let g = ((n / 6) % 6) * 51;
                let r = (n / 36) * 51;
                (r, g, b)
            } else {
                // 24 grayscale
                let v = 8 + (n - 232) * 10;
                (v, v, v)
            };
            [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
        }
        Color::Black => [0.0, 0.0, 0.0, 1.0],
        Color::Red => color_to_rgba(Color::Indexed(1), default),
        Color::Green => color_to_rgba(Color::Indexed(2), default),
        Color::Yellow => color_to_rgba(Color::Indexed(3), default),
        Color::Blue => color_to_rgba(Color::Indexed(4), default),
        Color::Magenta => color_to_rgba(Color::Indexed(5), default),
        Color::Cyan => color_to_rgba(Color::Indexed(6), default),
        Color::Gray => color_to_rgba(Color::Indexed(7), default),
        Color::LightRed => color_to_rgba(Color::Indexed(9), default),
        Color::LightGreen => color_to_rgba(Color::Indexed(10), default),
        Color::LightYellow => color_to_rgba(Color::Indexed(11), default),
        Color::LightBlue => color_to_rgba(Color::Indexed(12), default),
        Color::LightMagenta => color_to_rgba(Color::Indexed(13), default),
        Color::LightCyan => color_to_rgba(Color::Indexed(14), default),
        Color::LightGray => color_to_rgba(Color::Indexed(7), default),
        Color::White => [1.0, 1.0, 1.0, 1.0],
        Color::Reset => default,
    }
}

#[derive(Clone, Copy)]
struct AtlasEntry {
    uv: [f32; 4], // u0, v0, u1, v1
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

pub struct GlyphAtlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    cache: HashMap<GlyphKey, AtlasEntry>,
    atlas_width: u32,
    atlas_height: u32,
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
    rasterizer: Rasterizer,
    regular_key: FontKey,
    bold_key: Option<FontKey>,
    italic_key: Option<FontKey>,
    bold_italic_key: Option<FontKey>,
}

impl GlyphAtlas {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, font_size: f32) -> Self {
        let atlas_width = 2048u32;
        let atlas_height = 2048u32;

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph_atlas"),
            size: wgpu::Extent3d {
                width: atlas_width,
                height: atlas_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Clear atlas to zero
        let zeros = vec![0u8; (atlas_width * atlas_height) as usize];
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &zeros,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas_width),
                rows_per_image: Some(atlas_height),
            },
            wgpu::Extent3d {
                width: atlas_width,
                height: atlas_height,
                depth_or_array_layers: 1,
            },
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let mut rasterizer = Rasterizer::new().expect("failed to create rasterizer");

        let size = Size::new(font_size);
        let regular_desc = FontDesc::new(
            "monospace",
            Style::Description {
                slant: Slant::Normal,
                weight: Weight::Normal,
            },
        );
        let regular_key = rasterizer
            .load_font(&regular_desc, size)
            .expect("failed to load regular font");

        let bold_desc = FontDesc::new(
            "monospace",
            Style::Description {
                slant: Slant::Normal,
                weight: Weight::Bold,
            },
        );
        let bold_key = rasterizer.load_font(&bold_desc, size).ok();

        let italic_desc = FontDesc::new(
            "monospace",
            Style::Description {
                slant: Slant::Italic,
                weight: Weight::Normal,
            },
        );
        let italic_key = rasterizer.load_font(&italic_desc, size).ok();

        let bold_italic_desc = FontDesc::new(
            "monospace",
            Style::Description {
                slant: Slant::Italic,
                weight: Weight::Bold,
            },
        );
        let bold_italic_key = rasterizer.load_font(&bold_italic_desc, size).ok();

        GlyphAtlas {
            texture,
            view,
            sampler,
            cache: HashMap::new(),
            atlas_width,
            atlas_height,
            cursor_x: 0,
            cursor_y: 0,
            row_height: 0,
            rasterizer,
            regular_key,
            bold_key,
            italic_key,
            bold_italic_key,
        }
    }

    fn font_key_for_modifier(&self, modifier: Modifier) -> FontKey {
        let bold = modifier.contains(Modifier::BOLD);
        let italic = modifier.contains(Modifier::ITALIC);
        match (bold, italic) {
            (true, true) => self.bold_italic_key.unwrap_or(self.regular_key),
            (true, false) => self.bold_key.unwrap_or(self.regular_key),
            (false, true) => self.italic_key.unwrap_or(self.regular_key),
            (false, false) => self.regular_key,
        }
    }

    fn rasterize_and_upload(&mut self, queue: &wgpu::Queue, glyph_key: GlyphKey) -> AtlasEntry {
        let glyph = self
            .rasterizer
            .get_glyph(glyph_key)
            .expect("failed to rasterize glyph");

        let w = glyph.width as u32;
        let h = glyph.height as u32;

        if w == 0 || h == 0 {
            return AtlasEntry {
                uv: [0.0, 0.0, 0.0, 0.0],
                left: 0.0,
                top: 0.0,
                width: 0.0,
                height: 0.0,
            };
        }

        // Advance to next row if needed
        if self.cursor_x + w > self.atlas_width {
            self.cursor_x = 0;
            self.cursor_y += self.row_height + 1;
            self.row_height = 0;
        }

        let x = self.cursor_x;
        let y = self.cursor_y;

        // Convert bitmap to single-channel
        let alpha_data: Vec<u8> = match &glyph.buffer {
            BitmapBuffer::Rgb(data) => {
                // Use luminance as alpha
                data.chunks(3).map(|rgb| rgb[0]).collect()
            }
            BitmapBuffer::Rgba(data) => data.chunks(4).map(|rgba| rgba[3]).collect(),
        };

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &alpha_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );

        let entry = AtlasEntry {
            uv: [
                x as f32 / self.atlas_width as f32,
                y as f32 / self.atlas_height as f32,
                (x + w) as f32 / self.atlas_width as f32,
                (y + h) as f32 / self.atlas_height as f32,
            ],
            left: glyph.left as f32,
            top: glyph.top as f32,
            width: w as f32,
            height: h as f32,
        };

        self.cursor_x += w + 1;
        self.row_height = self.row_height.max(h);

        entry
    }

    fn get_or_insert(&mut self, queue: &wgpu::Queue, glyph_key: GlyphKey) -> AtlasEntry {
        if let Some(&entry) = self.cache.get(&glyph_key) {
            return entry;
        }
        let entry = self.rasterize_and_upload(queue, glyph_key);
        self.cache.insert(glyph_key, entry);
        entry
    }
}

// Per-cell instance data sent to GPU
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BgInstance {
    pos: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GlyphInstance {
    pos: [f32; 2],
    size: [f32; 2],
    uv: [f32; 4],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    screen_size: [f32; 2],
    _pad: [f32; 2],
}

pub struct Renderer {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub atlas: GlyphAtlas,
    bg_pipeline: wgpu::RenderPipeline,
    glyph_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    atlas_bind_group: wgpu::BindGroup,
    pub cell_width: f32,
    pub cell_height: f32,
    pub default_fg: [f32; 4],
    pub default_bg: [f32; 4],
}

impl Renderer {
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
        font_size: f32,
    ) -> Self {
        let atlas = GlyphAtlas::new(&device, &queue, font_size);

        // Get cell metrics from font
        let metrics = atlas
            .rasterizer
            .metrics(atlas.regular_key, Size::new(font_size))
            .expect("failed to get font metrics");
        let cell_width = metrics.average_advance as f32;
        let cell_height = metrics.line_height as f32;

        // Uniforms
        let uniforms = Uniforms {
            screen_size: [config.width as f32, config.height as f32],
            _pad: [0.0; 2],
        };
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("uniform_bgl"),
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

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniform_bg"),
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Atlas bind group
        let atlas_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("atlas_bgl"),
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
                ],
            });

        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas_bg"),
            layout: &atlas_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&atlas.sampler),
                },
            ],
        });

        // Shader modules
        let bg_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bg_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/bg.wgsl").into()),
        });

        let glyph_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glyph_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/glyph.wgsl").into()),
        });

        let surface_format = config.format;

        // Background pipeline
        let bg_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bg_pipeline_layout"),
            bind_group_layouts: &[&uniform_bind_group_layout],
            push_constant_ranges: &[],
        });

        let bg_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bg_pipeline"),
            layout: Some(&bg_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &bg_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<BgInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2, // pos
                        1 => Float32x2, // size
                        2 => Float32x4, // color
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &bg_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Glyph pipeline
        let glyph_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("glyph_pipeline_layout"),
                bind_group_layouts: &[&uniform_bind_group_layout, &atlas_bind_group_layout],
                push_constant_ranges: &[],
            });

        let glyph_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glyph_pipeline"),
            layout: Some(&glyph_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &glyph_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GlyphInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2, // pos
                        1 => Float32x2, // size
                        2 => Float32x4, // uv
                        3 => Float32x4, // color
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &glyph_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let default_fg = [0.85, 0.85, 0.85, 1.0];
        let default_bg = [0.1, 0.1, 0.1, 1.0];

        Renderer {
            device,
            queue,
            surface,
            config,
            atlas,
            bg_pipeline,
            glyph_pipeline,
            uniform_buffer,
            uniform_bind_group,
            atlas_bind_group,
            cell_width,
            cell_height,
            default_fg,
            default_bg,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);

        let uniforms = Uniforms {
            screen_size: [width as f32, height as f32],
            _pad: [0.0; 2],
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    pub fn render_grid(
        &mut self,
        grid: &[Cell],
        cols: u16,
        rows: u16,
        cursor_pos: Option<(u16, u16)>,
        cursor_kind: CursorKind,
    ) {
        let output = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(e) => {
                eprintln!("Surface error: {e}");
                return;
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut bg_instances: Vec<BgInstance> = Vec::with_capacity((cols * rows) as usize);
        let mut glyph_instances: Vec<GlyphInstance> = Vec::new();

        for row in 0..rows {
            for col in 0..cols {
                let idx = (row as usize) * (cols as usize) + (col as usize);
                let cell = &grid[idx];

                let px = col as f32 * self.cell_width;
                let py = row as f32 * self.cell_height;

                let modifier = cell.modifier;
                let reversed = modifier.contains(Modifier::REVERSED);
                let hidden = modifier.contains(Modifier::HIDDEN);
                let dim = modifier.contains(Modifier::DIM);

                let mut fg = color_to_rgba(cell.fg, self.default_fg);
                let mut bg = color_to_rgba(cell.bg, self.default_bg);

                if reversed {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if hidden {
                    fg = bg;
                }
                if dim {
                    fg[3] *= 0.5;
                }

                // Background
                bg_instances.push(BgInstance {
                    pos: [px, py],
                    size: [self.cell_width, self.cell_height],
                    color: bg,
                });

                // Glyph
                let ch = cell.symbol.chars().next().unwrap_or(' ');
                if ch != ' ' && !ch.is_control() {
                    let font_key = self.atlas.font_key_for_modifier(modifier);
                    let glyph_key = GlyphKey {
                        font_key,
                        character: ch,
                        size: Size::new(0.0), // uses loaded size
                    };

                    let entry = self.atlas.get_or_insert(&self.queue, glyph_key);
                    if entry.width > 0.0 {
                        let gx = px + entry.left;
                        let gy = py + (self.cell_height - entry.top);

                        glyph_instances.push(GlyphInstance {
                            pos: [gx, gy],
                            size: [entry.width, entry.height],
                            uv: entry.uv,
                            color: fg,
                        });
                    }
                }

                // Cursor
                if let Some((cx, cy)) = cursor_pos {
                    if col == cx && row == cy && cursor_kind != CursorKind::Hidden {
                        let (cw, ch) = match cursor_kind {
                            CursorKind::Block => (self.cell_width, self.cell_height),
                            CursorKind::Bar => (2.0, self.cell_height),
                            CursorKind::Underline => (self.cell_width, 2.0),
                            CursorKind::Hidden => unreachable!(),
                        };
                        let cy_offset = if cursor_kind == CursorKind::Underline {
                            self.cell_height - 2.0
                        } else {
                            0.0
                        };
                        bg_instances.push(BgInstance {
                            pos: [px, py + cy_offset],
                            size: [cw, ch],
                            color: self.default_fg, // cursor uses fg color
                        });
                    }
                }
            }
        }

        let bg_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bg_instances"),
                contents: bytemuck::cast_slice(&bg_instances),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let glyph_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("glyph_instances"),
                contents: bytemuck::cast_slice(&glyph_instances),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render_encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: self.default_bg[0] as f64,
                            g: self.default_bg[1] as f64,
                            b: self.default_bg[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            // Draw backgrounds
            pass.set_pipeline(&self.bg_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, bg_buffer.slice(..));
            pass.draw(0..4, 0..bg_instances.len() as u32);

            // Draw glyphs
            if !glyph_instances.is_empty() {
                pass.set_pipeline(&self.glyph_pipeline);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_bind_group(1, &self.atlas_bind_group, &[]);
                pass.set_vertex_buffer(0, glyph_buffer.slice(..));
                pass.draw(0..4, 0..glyph_instances.len() as u32);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }
}
