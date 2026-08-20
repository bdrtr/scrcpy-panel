//! YUV420P planes to an RGBA texture, converted on the card.
//!
//! This is the other half of the frame path. swscale converting to packed RGB
//! costs 0.59 ms a frame at 1080x2400, and Slint then carrying eight megabytes
//! of it to the card costs 3.90 — so the conversion is the small half and the
//! traffic is the large one. A YUV420P frame is 3.9 MB where the RGB is 7.8, and
//! the card can do the arithmetic for nothing, so uploading the planes and
//! converting here takes both.
//!
//! The horizontal mirror of `--display-orientation=flipN` comes along for the
//! ride: it is a sign on a texture coordinate here, where on the CPU it was the
//! one pass over the frame that could not be avoided.
//!
//! Behind the `wgpu` feature, and not wired into the client: measured, it comes
//! to 1.25 ms a frame against 1.42 for simply handing Slint an RGBA buffer,
//! which needs no card, no unstable API and no renderer switch. 0.17 ms is not
//! worth those. `examples/frame_cost.rs` is what uses it, and what found that
//! out; the README's roadmap has the ledger.

// Nothing in the client calls this — see above. It is kept because it is the
// measurement roadmap item 7 turns on, and because it is where the conversion
// would go if a machine ever came along where the ledger reads the other way.
#![allow(dead_code)]

use slint::wgpu_29::wgpu;

/// How many output textures are kept in turn. Slint holds the one it was given
/// until it has drawn it, so the next frame must not be written into that one.
const RING: usize = 3;

/// The conversion, and everything it needs that outlives a frame.
pub struct YuvToRgb {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    flip_buffer: wgpu::Buffer,
    /// The three plane textures and the bind group over them, for one size.
    planes: Option<Planes>,
    /// Output textures in turn, and where in the turn we are.
    out: Vec<wgpu::Texture>,
    next: usize,
    size: (u32, u32),
}

struct Planes {
    y: wgpu::Texture,
    u: wgpu::Texture,
    v: wgpu::Texture,
    bind: wgpu::BindGroup,
}

/// BT.601, limited range — the arithmetic swscale does for these frames, and
/// the reason `the_shader_and_swscale_agree` can compare the two at all.
const SHADER: &str = r#"
struct Uniforms {
    // 1.0 or -1.0: the horizontal mirror, done here rather than in a pass over
    // the frame on the CPU. A uniform block is rounded up to sixteen bytes
    // whatever is in it, which is what `flip_buffer` is sized to.
    flip: f32,
};

@group(0) @binding(0) var y_plane: texture_2d<f32>;
@group(0) @binding(1) var u_plane: texture_2d<f32>;
@group(0) @binding(2) var v_plane: texture_2d<f32>;
@group(0) @binding(3) var plane_sampler: sampler;
@group(0) @binding(4) var<uniform> uniforms: Uniforms;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> VertexOut {
    // One triangle large enough to cover the target, which is cheaper than two
    // and has no seam down the diagonal.
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    var out: VertexOut;
    out.uv = vec2<f32>(select(x, 1.0 - x, uniforms.flip < 0.0), y);
    out.position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

@fragment
fn fragment_main(in: VertexOut) -> @location(0) vec4<f32> {
    let y = textureSample(y_plane, plane_sampler, in.uv).r * 255.0;
    let u = textureSample(u_plane, plane_sampler, in.uv).r * 255.0;
    let v = textureSample(v_plane, plane_sampler, in.uv).r * 255.0;
    let luma = (y - 16.0) * 1.1643836;
    let cb = u - 128.0;
    let cr = v - 128.0;
    let r = luma + 1.5960268 * cr;
    let g = luma - 0.3917623 * cb - 0.8129676 * cr;
    let b = luma + 2.0172321 * cb;
    return vec4<f32>(
        clamp(r / 255.0, 0.0, 1.0),
        clamp(g / 255.0, 0.0, 1.0),
        clamp(b / 255.0, 0.0, 1.0),
        1.0,
    );
}
"#;

impl YuvToRgb {
    /// Build the pipeline on the device Slint is drawing with, which is the
    /// only device whose textures it will accept.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv420p → rgba"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let plane = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv planes"),
            entries: &[
                plane(0),
                plane(1),
                plane(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("yuv → rgba"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("yuv → rgba"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Nearest, not linear: swscale's own path gives a 2x2 block of pixels
        // the one chroma sample it has, and the point of comparing the two is
        // that they are doing the same arithmetic.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("planes"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let flip_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flip"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            bind_layout,
            sampler,
            flip_buffer,
            planes: None,
            out: Vec::new(),
            next: 0,
            size: (0, 0),
        }
    }

    /// Upload one frame's planes and convert them, giving back the texture as a
    /// Slint image.
    ///
    /// `strides` are the planes' own, which a decoder pads: a 1080-wide frame
    /// arrives with a 1088-byte luma row, and the upload is told so rather than
    /// the rows being packed down first.
    pub fn convert(
        &mut self,
        planes: [&[u8]; 3],
        strides: [usize; 3],
        width: u32,
        height: u32,
        flip: bool,
    ) -> anyhow::Result<slint::Image> {
        if self.size != (width, height) {
            self.build_for(width, height);
        }
        let chroma = (width.div_ceil(2), height.div_ceil(2));
        let sizes = [(width, height), chroma, chroma];
        let textures = {
            let ready = self.planes.as_ref().expect("planes were just built");
            [&ready.y, &ready.u, &ready.v]
        };
        for (index, texture) in textures.iter().enumerate() {
            let (plane_width, plane_height) = sizes[index];
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                planes[index],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(strides[index] as u32),
                    rows_per_image: Some(plane_height),
                },
                wgpu::Extent3d {
                    width: plane_width,
                    height: plane_height,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.queue.write_buffer(
            &self.flip_buffer,
            0,
            bytemuck_f32(if flip { -1.0 } else { 1.0 }).as_slice(),
        );

        let target = &self.out[self.next];
        self.next = (self.next + 1) % RING;
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("yuv") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("yuv → rgba"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.planes.as_ref().expect("planes").bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        slint::Image::try_from(target.clone())
            .map_err(|e| anyhow::anyhow!("Slint would not take the texture: {e:?}"))
    }

    /// The textures for one frame size. Rebuilt when the device rotates.
    fn build_for(&mut self, width: u32, height: u32) {
        let make = |label: &str, width: u32, height: u32, usage: wgpu::TextureUsages, format| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let plane_usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
        let y = make("y", width, height, plane_usage, wgpu::TextureFormat::R8Unorm);
        let u = make(
            "u",
            chroma_width,
            chroma_height,
            plane_usage,
            wgpu::TextureFormat::R8Unorm,
        );
        let v = make(
            "v",
            chroma_width,
            chroma_height,
            plane_usage,
            wgpu::TextureFormat::R8Unorm,
        );
        let view = |texture: &wgpu::Texture| texture.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yuv planes"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view(&y)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view(&u)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view(&v)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.flip_buffer.as_entire_binding(),
                },
            ],
        });
        self.planes = Some(Planes { y, u, v, bind });

        // Slint keeps the texture it was handed until it has drawn it, so the
        // next frame goes into the next one of these.
        let out_usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        self.out = (0..RING)
            .map(|_| make("rgba", width, height, out_usage, wgpu::TextureFormat::Rgba8Unorm))
            .collect();
        self.next = 0;
        self.size = (width, height);
    }

    /// The last converted frame's pixels, read back off the card. For the test
    /// that holds the shader to what swscale does; nothing in a session does
    /// this, because reading back is the cost the whole thing exists to avoid.
    pub fn read_back(&self, width: u32, height: u32) -> Vec<u8> {
        let previous = (self.next + RING - 1) % RING;
        let row = (width as usize * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("read back"),
            size: (row * height as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.out[previous],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row as u32),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let mapped = buffer.slice(..).get_mapped_range();
        let mut out = Vec::with_capacity(width as usize * height as usize * 4);
        for line in 0..height as usize {
            out.extend_from_slice(&mapped[line * row..][..width as usize * 4]);
        }
        drop(mapped);
        buffer.unmap();
        out
    }
}

/// One float as the sixteen bytes the uniform block wants.
fn bytemuck_f32(value: f32) -> Vec<u8> {
    let mut bytes = value.to_ne_bytes().to_vec();
    bytes.resize(16, 0);
    bytes
}
