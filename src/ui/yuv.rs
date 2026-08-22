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
//! ride: it is the column read back to front here — `width - 1 - x`, in the
//! same fragment that does the colour — where on the CPU it was the one pass
//! over the frame that could not be avoided.
//!
//! Behind the `wgpu` feature, and not wired into the client: it comes to 1.25 ms
//! a frame, and simply handing Slint an RGBA buffer costs 0.91 on the renderer
//! the client actually uses — which needs no card, no unstable API and no
//! renderer switch. The shader is the dearer of the two by 0.34 ms, not the
//! cheaper: it beats an RGBA upload on the WGPU renderer by 1.14, and getting
//! onto that renderer costs more than that. `examples/frame_cost.rs` is what
//! uses it, and what found that out; the README has the ledger.

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

/// BT.601, limited range — the arithmetic swscale does for these frames, which
/// is what lets `the_shader_and_swscale_agree_where_swscale_replicates` compare
/// the two at all.
const SHADER: &str = r#"
struct Uniforms {
    // 1.0 or -1.0: the horizontal mirror, done here rather than in a pass over
    // the frame on the CPU. A uniform block is rounded up to sixteen bytes
    // whatever is in it, which is what `flip_buffer` is sized to.
    flip: f32,
    // The picture's width in luma pixels, which is what the mirror needs to
    // turn a column into the one across from it now that the coordinate it
    // works in is a pixel rather than a fraction.
    width: u32,
};

@group(0) @binding(0) var y_plane: texture_2d<f32>;
@group(0) @binding(1) var u_plane: texture_2d<f32>;
@group(0) @binding(2) var v_plane: texture_2d<f32>;
@group(0) @binding(3) var<uniform> uniforms: Uniforms;

@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // One triangle large enough to cover the target, which is cheaper than two
    // and has no seam down the diagonal.
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

@fragment
fn fragment_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // Loaded by pixel rather than sampled by fraction, and this is the whole of
    // why. A normalised coordinate is the *luma* plane's; the chroma planes are
    // ceil(w/2) by ceil(h/2), which at an even size covers exactly the same
    // picture and at an odd one covers half a chroma texel more. The same
    // coordinate then lands one texel to the side for every other column from
    // the middle of the picture onwards — a wrong colour, not a rounding
    // difference. At 1081x2400 that read 16.5 of 255 against swscale where
    // 1080x2400 read 0.70, and it is the width that shows it cleanly: at an odd
    // *height* swscale is not doing this arithmetic at all, so it cannot say
    // who is right. 4:2:0 means floor(x/2), floor(y/2), and a load says that at
    // every size — which is what `the_shader_draws_what_four_two_zero_means`
    // holds it to, against a reference of that definition rather than against
    // another implementation of it.
    var pixel = vec2<u32>(position.xy);
    if uniforms.flip < 0.0 {
        pixel.x = uniforms.width - 1u - pixel.x;
    }
    let chroma = pixel >> vec2<u32>(1u);

    let y = textureLoad(y_plane, pixel, 0).r * 255.0;
    let u = textureLoad(u_plane, chroma, 0).r * 255.0;
    let v = textureLoad(v_plane, chroma, 0).r * 255.0;
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
                // Nothing filters these: the fragment shader loads the texel it
                // wants by index, because a 4:2:0 chroma plane is not a
                // resampling of the picture but one sample per two-by-two
                // block, and floor(x/2) is what says so.
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
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
            uniforms(if flip { -1.0 } else { 1.0 }, width).as_slice(),
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
            .map(|_| {
                make(
                    "rgba",
                    width,
                    height,
                    out_usage,
                    wgpu::TextureFormat::Rgba8Unorm,
                )
            })
            .collect();
        self.next = 0;
        self.size = (width, height);
    }

    /// The last converted frame's pixels, read back off the card. For the two
    /// tests at the end of this file; nothing in a session does it, because
    /// reading back is the cost the whole thing exists to avoid.
    ///
    /// A texture-to-buffer copy wants its rows a multiple of 256 bytes, so the
    /// rows come back padded and are cut down to the picture here — every
    /// caller wants them packed.
    ///
    /// It reads the texture `convert` wrote last, which is the one before
    /// `next`. Called before the first `convert` of a size it would read one
    /// that `build_for` had only just created and nothing had drawn into; every
    /// caller converts first, and there is nothing sensible for it to return if
    /// one did not.
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

/// The mirror and the width, as the sixteen bytes the uniform block wants.
fn uniforms(flip: f32, width: u32) -> Vec<u8> {
    let mut bytes = flip.to_ne_bytes().to_vec();
    bytes.extend_from_slice(&width.to_ne_bytes());
    bytes.resize(16, 0);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device of this file's own. The client's comes from inside a Slint
    /// window, which a test has no business opening.
    fn a_card() -> Option<(wgpu::Device, wgpu::Queue)> {
        // No window and no surface: this only ever renders to a texture. Three
        // things about Slint's wgpu 29 re-exports differ from the wgpu docs and
        // each cost a compile: `InstanceDescriptor` has no `Default`,
        // `Instance::new` takes it by value, and `enumerate_adapters` returns a
        // future rather than a `Vec`.
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
            .into_iter()
            .next()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
    }

    /// A frame of made-up planes, at ffmpeg's own strides so that the padding
    /// the upload is told about is really there. High-frequency noise on
    /// purpose: a smooth picture hides an off-by-one in the chroma index, which
    /// is the whole of what these tests are for.
    fn a_frame(width: u32, height: u32) -> ffmpeg_next::frame::Video {
        ffmpeg_next::init().expect("ffmpeg");
        let mut frame =
            ffmpeg_next::frame::Video::new(ffmpeg_next::format::Pixel::YUV420P, width, height);
        for plane in 0..frame.planes() {
            for (i, byte) in frame.data_mut(plane).iter_mut().enumerate() {
                *byte = (i.wrapping_mul(37 + plane).wrapping_add(plane * 7) % 251) as u8;
            }
        }
        frame
    }

    /// What the shader is meant to draw, worked out on the CPU from the same
    /// definition: 4:2:0 is one chroma sample per two-by-two block, so pixel
    /// (x, y) takes the sample at (x/2, y/2), and the matrix is BT.601 limited
    /// range. Deliberately not swscale — see the second test for why swscale
    /// cannot be the reference at every size.
    ///
    /// What this cannot do is arbitrate the definition, since it restates it.
    /// It settles everything around it, which is where the faults were and
    /// where they would be again: the plane strides, the index into each plane,
    /// the mirror, the matrix, and the upload of three differently-sized
    /// textures. That floor(x/2), floor(y/2) is the right rule is settled
    /// elsewhere — it is what swscale itself does at every size where it is
    /// converting rather than rescaling.
    fn the_definition(
        frame: &ffmpeg_next::frame::Video,
        width: u32,
        height: u32,
        flip: bool,
    ) -> Vec<u8> {
        let planes = [frame.data(0), frame.data(1), frame.data(2)];
        let strides = [frame.stride(0), frame.stride(1), frame.stride(2)];
        let mut out = Vec::with_capacity(width as usize * height as usize * 4);
        for row in 0..height as usize {
            for column in 0..width as usize {
                let source = if flip {
                    width as usize - 1 - column
                } else {
                    column
                };
                let y = planes[0][row * strides[0] + source] as f64;
                let (chroma_x, chroma_y) = (source / 2, row / 2);
                let u = planes[1][chroma_y * strides[1] + chroma_x] as f64;
                let v = planes[2][chroma_y * strides[2] + chroma_x] as f64;
                let luma = (y - 16.0) * 1.1643836;
                let (cb, cr) = (u - 128.0, v - 128.0);
                for channel in [
                    luma + 1.5960268 * cr,
                    luma - 0.3917623 * cb - 0.8129676 * cr,
                    luma + 2.0172321 * cb,
                ] {
                    out.push(channel.clamp(0.0, 255.0).round() as u8);
                }
                out.push(255);
            }
        }
        out
    }

    /// The same frame through swscale, packed RGB24, and how many of its
    /// columns are real.
    ///
    /// swscale does not always fill the row. With the destination row exactly
    /// the picture — which is what this does, and what the decoder does — it
    /// leaves the last `width % 16` columns untouched whenever that is between
    /// one and seven: 641 loses one, 68 loses four, 1079 loses seven, and 1080,
    /// 1081 and 1082 lose none. Swept against libswscale directly over every
    /// width from 8 to 400 and every even one to 1600 with no exception — but
    /// that sweep only ever asked one converter. The rule belongs to the x86
    /// SIMD path this machine dispatches to, not to libswscale: with
    /// `av_force_cpu_flags(0)` the same sweep says odd widths lose one and even
    /// widths lose none, which agrees at 640, 641, 65 and 1080 and disagrees at
    /// 68, 1079 and 1081.
    ///
    /// Which is the whole argument for the sentinel. There is no formula worth
    /// trusting here, so the reference measures how much of itself is real
    /// rather than predicting it; give the row 32 bytes of slack and every
    /// converter fills every column.
    fn through_swscale(
        frame: &ffmpeg_next::frame::Video,
        width: u32,
        height: u32,
    ) -> (Vec<u8>, usize) {
        const SENTINEL: u8 = 0xAB;
        let row_bytes = width as usize * 3;
        // swscale fills its rows out past the picture — the decoder's own tests
        // measure how far — so the buffer is longer than the picture and cut
        // back afterwards.
        let mut out = vec![SENTINEL; row_bytes * height as usize + 4096];
        let mut scaler = ffmpeg_next::software::scaling::Context::get(
            ffmpeg_next::format::Pixel::YUV420P,
            width,
            height,
            ffmpeg_next::format::Pixel::RGB24,
            width,
            height,
            ffmpeg_next::software::scaling::Flags::BILINEAR,
        )
        .expect("a scaler");
        unsafe {
            let mut planes: [*mut u8; 4] = [
                out.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ];
            let strides: [i32; 4] = [row_bytes as i32, 0, 0, 0];
            ffmpeg_next::ffi::sws_scale(
                scaler.as_mut_ptr(),
                (*frame.as_ptr()).data.as_ptr() as *const *const u8,
                (*frame.as_ptr()).linesize.as_ptr(),
                0,
                height as i32,
                planes.as_mut_ptr(),
                strides.as_ptr(),
            );
        }
        out.truncate(row_bytes * height as usize);

        // The written columns are the ones where any row is not the sentinel.
        let written = (0..width as usize)
            .rev()
            .find(|column| {
                (0..height as usize)
                    .any(|row| out[row * row_bytes + column * 3..][..3] != [SENTINEL; 3])
            })
            .map_or(0, |column| column + 1);
        (out, written)
    }

    /// The card's picture against a reference, as a mean, a worst and where the
    /// worst was. `bytes` is the reference's bytes a pixel — four for our own,
    /// three for swscale's packed RGB — and `columns` how many of them to
    /// believe, which is not always all of them.
    fn difference(
        card: &[u8],
        reference: &[u8],
        bytes: usize,
        width: u32,
        columns: usize,
        height: u32,
    ) -> (f64, u8, (u32, u32)) {
        let mut sum = 0u64;
        let mut worst = 0u8;
        let mut worst_at = (0, 0);
        let mut counted = 0u64;
        for row in 0..height as usize {
            for column in 0..columns {
                let a = &card[(row * width as usize + column) * 4..][..3];
                let b = &reference[(row * width as usize + column) * bytes..][..3];
                for channel in 0..3 {
                    let difference = a[channel].abs_diff(b[channel]);
                    sum += difference as u64;
                    if difference > worst {
                        worst = difference;
                        worst_at = (column as u32, row as u32);
                    }
                    counted += 1;
                }
            }
        }
        (sum as f64 / counted as f64, worst, worst_at)
    }

    /// Every size the sizes below are chosen from, and why: `--crop`,
    /// `--max-size` and a device that picks its own display size all produce
    /// odd ones, and the drift they used to cause was invisible at 1080x2400.
    const SIZES: [(u32, u32); 8] = [
        (640, 480),
        (641, 480),
        (640, 481),
        (641, 481),
        (65, 64),
        (64, 65),
        (1080, 2400),
        (1081, 2399),
    ];

    /// The test this file's comments had been citing since it was written, and
    /// which did not exist. It holds the shader to the definition of 4:2:0
    /// rather than to another implementation of it, so a wrong matrix, a plane
    /// read at the wrong stride, a chroma sample taken from the wrong column or
    /// row, and a mirror that mirrors the wrong thing would each fail it — at
    /// every size, not only the even ones.
    ///
    /// The tolerance is for arithmetic, not for indexing: the card works in f32
    /// and this works in f64, so a channel either side of a rounding boundary
    /// can differ by one. An index that is wrong by one is worth far more than
    /// that on this frame, which is noise on purpose.
    ///
    /// Needs a card: `cargo test --release --features wgpu the_shader`. Skips
    /// itself where there is no adapter rather than failing.
    #[test]
    fn the_shader_draws_what_four_two_zero_means() {
        let Some((device, queue)) = a_card() else {
            eprintln!("no wgpu adapter on this machine, so the shader is unchecked");
            return;
        };
        let mut converter = YuvToRgb::new(&device, &queue);

        for (width, height) in SIZES {
            let frame = a_frame(width, height);
            let planes: [&[u8]; 3] = [frame.data(0), frame.data(1), frame.data(2)];
            let strides = [frame.stride(0), frame.stride(1), frame.stride(2)];
            for flip in [false, true] {
                converter
                    .convert(planes, strides, width, height, flip)
                    .expect("a converted frame");
                let from_card = converter.read_back(width, height);
                let wanted = the_definition(&frame, width, height, flip);

                let (mean, worst, at) =
                    difference(&from_card, &wanted, 4, width, width as usize, height);
                assert!(
                    mean < 0.5 && worst <= 2,
                    "{width}x{height} flip={flip}: mean {mean:.3} of 255, worst {worst} at {at:?}"
                );
            }
        }
    }

    /// And the cross-check against another implementation, which is worth
    /// having because it is the one the client actually ships — but only where
    /// the two are doing the same thing.
    ///
    /// They are not, at an odd height. swscale converts YUV420P to RGB24 with
    /// an unscaled converter that replicates each chroma row down two output
    /// rows — the same thing the shader does — only while the chroma plane's
    /// height goes into the picture's exactly twice. At an odd height it does
    /// not: ceil(h/2) rows have to reach h, which is not a doubling, so swscale
    /// builds a vertical scaler and *blends* two chroma rows instead. Measured
    /// against libswscale directly, with a flat luma and a chroma plane whose
    /// rows are labelled: at 8x10 every output row comes back a pure chroma row
    /// (weights 0.00 and 0.99), and at 8x11 the same probe reads weights of
    /// 0.293, 0.849, 0.575, 0.043 — two rows mixed. 1080x2399 and 640x481 read
    /// 0.238 and 0.732 alternating. SWS_POINT does not put it back either; it
    /// picks the nearest row of a rescaled grid rather than floor(y/2).
    ///
    /// So an odd height compares two different pictures, and on this
    /// deliberately noisy frame that difference is worth a mean of 37.8 of 255.
    /// Neither is wrong — floor(y/2) is what 4:2:0 means and what swscale
    /// itself does whenever the ratio is two — so the test says what it can say
    /// honestly, which is that they agree wherever they are doing the same
    /// arithmetic. The first test is the one that covers the rest.
    #[test]
    fn the_shader_and_swscale_agree_where_swscale_replicates() {
        let Some((device, queue)) = a_card() else {
            eprintln!("no wgpu adapter on this machine, so the shader is unchecked");
            return;
        };
        let mut converter = YuvToRgb::new(&device, &queue);

        for (width, height) in SIZES.into_iter().filter(|(_, h)| h % 2 == 0) {
            let frame = a_frame(width, height);
            let planes: [&[u8]; 3] = [frame.data(0), frame.data(1), frame.data(2)];
            let strides = [frame.stride(0), frame.stride(1), frame.stride(2)];
            converter
                .convert(planes, strides, width, height, false)
                .expect("a converted frame");
            let from_card = converter.read_back(width, height);
            let (from_swscale, written) = through_swscale(&frame, width, height);

            // What this host's swscale loses, reported and not asserted. The
            // rule is not libswscale's, it is the x86 SIMD converter's: with
            // `av_force_cpu_flags(0)` the losses change shape entirely — 1081
            // goes from all of its columns to 1080, 68 from 64 to all 68 — so
            // pinning it here would fail on a machine that dispatches
            // elsewhere, for a reason with nothing to do with the shader. What
            // the comparison uses is the count measured just above, which is
            // right whichever converter ran.
            let lost = width as usize % 16;
            let expected = if width >= 8 && (1..8).contains(&lost) {
                width as usize - lost
            } else {
                width as usize
            };
            if written != expected {
                eprintln!(
                    "{width}x{height}: swscale filled {written} columns where this \
                     machine's SIMD path fills {expected} — a different converter, \
                     not a fault"
                );
            }

            let (mean, worst, at) =
                difference(&from_card, &from_swscale, 3, width, written, height);
            assert!(
                mean < 1.5 && worst <= 16,
                "{width}x{height}: over the {written} columns swscale wrote, \
                 mean {mean:.3} of 255, worst {worst} at {at:?}"
            );
        }
    }
}
