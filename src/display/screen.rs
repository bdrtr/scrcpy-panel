use anyhow::{Context, Result};
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use sdl2::render::{Canvas, Texture, TextureCreator};
use sdl2::video::{Window, WindowContext};
use sdl2::sys;
use std::os::raw::{c_int, c_void};
use crate::media::decoder::DecodedFrame;

/// Client-side display orientation (0, 90, 180, 270 with optional horizontal flip)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Normal,     // 0°
    Rot90,      // 90° CW
    Rot180,     // 180°
    Rot270,     // 270° CW (= 90° CCW)
}

impl Orientation {
    /// Angle in degrees (clockwise) for SDL_RenderCopyEx
    pub fn angle(self) -> f64 {
        match self {
            Orientation::Normal => 0.0,
            Orientation::Rot90  => 90.0,
            Orientation::Rot180 => 180.0,
            Orientation::Rot270 => 270.0,
        }
    }

    /// Does this orientation swap width and height?
    pub fn swaps_dimensions(self) -> bool {
        matches!(self, Orientation::Rot90 | Orientation::Rot270)
    }

    /// Cycle to the next rotation (CW)
    pub fn rotate_cw(self) -> Self {
        match self {
            Orientation::Normal => Orientation::Rot90,
            Orientation::Rot90  => Orientation::Rot180,
            Orientation::Rot180 => Orientation::Rot270,
            Orientation::Rot270 => Orientation::Normal,
        }
    }

    /// Cycle to the previous rotation (CCW)
    pub fn rotate_ccw(self) -> Self {
        match self {
            Orientation::Normal => Orientation::Rot270,
            Orientation::Rot90  => Orientation::Normal,
            Orientation::Rot180 => Orientation::Rot90,
            Orientation::Rot270 => Orientation::Rot180,
        }
    }
}

// OpenGL constants (same on all platforms)
const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
const GL_LINEAR_MIPMAP_LINEAR: i32 = 0x2703;
const GL_TEXTURE_LOD_BIAS: u32 = 0x8501;

type GlTexParameteriFunc = unsafe extern "C" fn(u32, u32, i32);
type GlTexParameterfFunc = unsafe extern "C" fn(u32, u32, f32);
type GlGenerateMipmapFunc = unsafe extern "C" fn(u32);

/// Display margins to keep from screen edges
const DISPLAY_MARGINS: u32 = 96;

// =====================================================================
// Windows continuous resize workaround
// On Windows (and macOS), SDL blocks the event loop during window drag.
// SDL_AddEventWatch fires a callback during the modal loop, allowing
// us to re-render the texture in real-time while resizing.
// =====================================================================

/// Data passed to the resize event watch callback
struct ResizeWatchData {
    renderer: *mut sys::SDL_Renderer,
    texture: *mut sys::SDL_Texture,
    frame_width: u32,
    frame_height: u32,
}

/// C-compatible callback for SDL_AddEventWatch
unsafe extern "C" fn resize_event_watch(
    userdata: *mut c_void,
    event: *mut sys::SDL_Event,
) -> c_int {
    let data = &*(userdata as *const ResizeWatchData);
    let event = &*event;

    if event.type_ == sys::SDL_EventType::SDL_WINDOWEVENT as u32 {
        let win_event = event.window;
        if win_event.event == sys::SDL_WindowEventID::SDL_WINDOWEVENT_RESIZED as u8
            || win_event.event == sys::SDL_WindowEventID::SDL_WINDOWEVENT_SIZE_CHANGED as u8
        {
            let win_w = win_event.data1 as u32;
            let win_h = win_event.data2 as u32;

            if data.frame_width > 0 && data.frame_height > 0 && win_w > 0 && win_h > 0 {
                // Calculate content rect preserving aspect ratio
                let dst = content_rect_raw(win_w, win_h, data.frame_width, data.frame_height);

                sys::SDL_RenderClear(data.renderer);
                let dst_rect = sys::SDL_Rect {
                    x: dst.0, y: dst.1, w: dst.2 as c_int, h: dst.3 as c_int,
                };
                sys::SDL_RenderCopy(
                    data.renderer,
                    data.texture,
                    std::ptr::null(),
                    &dst_rect,
                );
                sys::SDL_RenderPresent(data.renderer);
            }
        }
    }
    0
}

/// Calculate content rect (standalone, for use in the C callback)
fn content_rect_raw(win_w: u32, win_h: u32, frame_w: u32, frame_h: u32) -> (i32, i32, u32, u32) {
    let keep_width = (frame_w as u64) * (win_h as u64)
        > (frame_h as u64) * (win_w as u64);

    let (w, h) = if keep_width {
        let h = (frame_h as u64) * (win_w as u64) / (frame_w as u64);
        (win_w, h as u32)
    } else {
        let w = (frame_w as u64) * (win_h as u64) / (frame_h as u64);
        (w as u32, win_h)
    };

    let x = ((win_w - w) / 2) as i32;
    let y = ((win_h - h) / 2) as i32;
    (x, y, w, h)
}

/// The screen manages the SDL window and renders video frames.
pub struct Screen {
    pub canvas: Canvas<Window>,
    _texture_creator: Box<TextureCreator<WindowContext>>,
    texture: Option<Texture<'static>>,
    pub frame_width: u32,
    pub frame_height: u32,
    pub fullscreen: bool,
    /// Resize watch data (heap-allocated so the pointer stays stable)
    resize_watch: Option<Box<ResizeWatchData>>,
    /// OpenGL mipmap support
    mipmaps: bool,
    gl_tex_parameteri: Option<GlTexParameteriFunc>,
    gl_tex_parameterf: Option<GlTexParameterfFunc>,
    gl_generate_mipmap: Option<GlGenerateMipmapFunc>,
    /// Client-side display orientation
    pub orientation: Orientation,
}

impl Screen {
    /// Create a new screen with an SDL window
    pub fn new(
        sdl_video: &sdl2::VideoSubsystem,
        title: &str,
        frame_width: u32,
        frame_height: u32,
        fullscreen: bool,
        _always_on_top: bool,
        borderless: bool,
        window_x: Option<i16>,
        window_y: Option<i16>,
        window_width: Option<u16>,
        window_height: Option<u16>,
    ) -> Result<Self> {
        // Use explicit size if provided, else auto-calculate
        let (win_w, win_h) = match (window_width, window_height) {
            (Some(w), Some(h)) => (w as u32, h as u32),
            _ => Self::optimal_size(frame_width, frame_height),
        };

        let mut window_builder = sdl_video.window(title, win_w, win_h);

        // Position: explicit or centered
        match (window_x, window_y) {
            (Some(x), Some(y)) => { window_builder.position(x as i32, y as i32); }
            _ => { window_builder.position_centered(); }
        }

        window_builder
            .resizable()
            .allow_highdpi();

        if borderless {
            window_builder.borderless();
        }

        let window = window_builder
            .build()
            .context("Failed to create SDL window")?;

        // Apply always-on-top if requested (SDL2 raw API)
        if _always_on_top {
            unsafe {
                sys::SDL_SetWindowAlwaysOnTop(
                    window.raw(),
                    sdl2::sys::SDL_bool::SDL_TRUE,
                );
            }
            log::info!("Window set to always-on-top");
        }
        // Set window icon (procedurally generated 32x32 phone icon)
        {
            let icon_size: u32 = 32;
            let mut pixels = vec![0u8; (icon_size * icon_size * 4) as usize];
            for y in 0..icon_size {
                for x in 0..icon_size {
                    let idx = ((y * icon_size + x) * 4) as usize;
                    // Phone outline: green rectangle with rounded feel
                    let in_body = x >= 6 && x < 26 && y >= 2 && y < 30;
                    let in_screen = x >= 8 && x < 24 && y >= 5 && y < 25;
                    let in_button = x >= 13 && x < 19 && y >= 26 && y < 28;
                    if in_screen {
                        // White screen
                        pixels[idx] = 255; pixels[idx+1] = 255;
                        pixels[idx+2] = 255; pixels[idx+3] = 255;
                    } else if in_button {
                        // Home button (light gray)
                        pixels[idx] = 200; pixels[idx+1] = 200;
                        pixels[idx+2] = 200; pixels[idx+3] = 255;
                    } else if in_body {
                        // Green body (#4CAF50)
                        pixels[idx] = 76; pixels[idx+1] = 175;
                        pixels[idx+2] = 80; pixels[idx+3] = 255;
                    }
                    // else: transparent (0,0,0,0)
                }
            }
            unsafe {
                let surface = sys::SDL_CreateRGBSurfaceFrom(
                    pixels.as_ptr() as *mut std::ffi::c_void,
                    icon_size as i32, icon_size as i32,
                    32, (icon_size * 4) as i32,
                    0x000000FF, 0x0000FF00, 0x00FF0000, 0xFF000000,
                );
                if !surface.is_null() {
                    sys::SDL_SetWindowIcon(window.raw(), surface);
                    sys::SDL_FreeSurface(surface);
                }
            }
        }

        let canvas = window
            .into_canvas()
            .accelerated()
            .build()
            .context("Failed to create SDL canvas")?;

        let texture_creator = Box::new(canvas.texture_creator());

        let mut screen = Self {
            canvas,
            _texture_creator: texture_creator,
            texture: None,
            frame_width,
            frame_height,
            fullscreen,
            resize_watch: None,
            mipmaps: false,
            gl_tex_parameteri: None,
            gl_tex_parameterf: None,
            gl_generate_mipmap: None,
            orientation: Orientation::Normal,
        };

        // Detect OpenGL renderer and enable mipmaps (matches C display.c)
        screen.init_opengl_mipmaps();

        screen.create_texture(frame_width, frame_height)?;
        screen.register_resize_watch();

        if fullscreen {
            screen.toggle_fullscreen();
        }

        Ok(screen)
    }

    /// Try to enable OpenGL mipmaps (trilinear filtering) for better downscaling
    fn init_opengl_mipmaps(&mut self) {
        unsafe {
            let mut info: sys::SDL_RendererInfo = std::mem::zeroed();
            if sys::SDL_GetRendererInfo(self.canvas.raw(), &mut info) != 0 {
                return;
            }
            let name = if info.name.is_null() {
                return;
            } else {
                std::ffi::CStr::from_ptr(info.name).to_string_lossy()
            };

            log::info!("Renderer: {}", name);

            if !name.starts_with("opengl") {
                log::info!("Mipmaps disabled (not an OpenGL renderer)");
                return;
            }

            // Load GL functions via SDL
            let tex_pi = sys::SDL_GL_GetProcAddress(
                b"glTexParameteri\0".as_ptr() as *const _
            );
            let tex_pf = sys::SDL_GL_GetProcAddress(
                b"glTexParameterf\0".as_ptr() as *const _
            );
            let gen_mm = sys::SDL_GL_GetProcAddress(
                b"glGenerateMipmap\0".as_ptr() as *const _
            );

            if tex_pi.is_null() || tex_pf.is_null() || gen_mm.is_null() {
                log::info!("Mipmaps disabled (GL functions not available)");
                return;
            }

            self.gl_tex_parameteri = Some(std::mem::transmute(tex_pi));
            self.gl_tex_parameterf = Some(std::mem::transmute(tex_pf));
            self.gl_generate_mipmap = Some(std::mem::transmute(gen_mm));
            self.mipmaps = true;
            log::info!("Trilinear filtering enabled");
        }
    }

    /// Register the SDL event watch for continuous resize rendering
    fn register_resize_watch(&mut self) {
        // Get raw pointers to the renderer and texture
        let renderer = unsafe { self.canvas.raw() };
        let texture_raw = match self.texture {
            Some(ref t) => unsafe { t.raw() },
            None => return,
        };

        let watch_data = Box::new(ResizeWatchData {
            renderer,
            texture: texture_raw,
            frame_width: self.frame_width,
            frame_height: self.frame_height,
        });

        // Remove any existing watch
        self.unregister_resize_watch();

        let data_ptr = &*watch_data as *const ResizeWatchData as *mut c_void;
        unsafe {
            sys::SDL_AddEventWatch(Some(resize_event_watch), data_ptr);
        }

        self.resize_watch = Some(watch_data);
    }

    /// Remove the event watch
    fn unregister_resize_watch(&mut self) {
        if let Some(ref watch_data) = self.resize_watch {
            let data_ptr = &**watch_data as *const ResizeWatchData as *mut c_void;
            unsafe {
                sys::SDL_DelEventWatch(Some(resize_event_watch), data_ptr);
            }
        }
        self.resize_watch = None;
    }

    /// Update the resize watch data (after texture recreation)
    fn update_resize_watch(&mut self) {
        if self.resize_watch.is_some() {
            self.register_resize_watch();
        }
    }

    /// Calculate optimal window size maintaining aspect ratio
    fn optimal_size(content_w: u32, content_h: u32) -> (u32, u32) {
        let max_w = 1920u32.saturating_sub(DISPLAY_MARGINS);
        let max_h = 1080u32.saturating_sub(DISPLAY_MARGINS);

        let mut w = content_w;
        let mut h = content_h;

        if w > max_w {
            h = h * max_w / w;
            w = max_w;
        }
        if h > max_h {
            w = w * max_h / h;
            h = max_h;
        }

        (w.max(1), h.max(1))
    }

    /// Create or recreate the YUV texture (only called on size change)
    fn create_texture(&mut self, width: u32, height: u32) -> Result<()> {
        // Remove event watch before destroying texture
        self.unregister_resize_watch();

        // Drop old texture first
        self.texture = None;

        let texture = self._texture_creator
            .create_texture_streaming(PixelFormatEnum::IYUV, width, height)
            .context("Failed to create YUV texture")?;

        // SAFETY: _texture_creator is Box'd (stable address) and outlives this texture
        let texture: Texture<'static> = unsafe { std::mem::transmute(texture) };

        // Enable trilinear filtering if mipmaps are supported
        if self.mipmaps {
            unsafe {
                sys::SDL_GL_BindTexture(texture.raw(), std::ptr::null_mut(), std::ptr::null_mut());
                if let Some(tex_pi) = self.gl_tex_parameteri {
                    tex_pi(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR_MIPMAP_LINEAR);
                }
                if let Some(tex_pf) = self.gl_tex_parameterf {
                    tex_pf(GL_TEXTURE_2D, GL_TEXTURE_LOD_BIAS, -1.0);
                }
                sys::SDL_GL_UnbindTexture(texture.raw());
            }
        }

        self.texture = Some(texture);
        self.frame_width = width;
        self.frame_height = height;
        Ok(())
    }

    /// Update the texture with a decoded YUV frame and render it
    pub fn render_frame(&mut self, frame: &DecodedFrame) -> Result<()> {
        // Recreate texture only if frame size changed
        if frame.width != self.frame_width || frame.height != self.frame_height {
            log::info!("Frame size changed: {}x{} → {}x{}",
                self.frame_width, self.frame_height, frame.width, frame.height);
            self.create_texture(frame.width, frame.height)?;
        }

        if let Some(ref mut texture) = self.texture {
            // Update YUV texture planes (single GPU upload, no allocation)
            texture.update_yuv(
                None,
                &frame.data[0], frame.linesize[0],
                &frame.data[1], frame.linesize[1],
                &frame.data[2], frame.linesize[2],
            ).context("Failed to update texture")?;

            // Regenerate mipmaps after texture update (matches C display.c)
            if self.mipmaps {
                unsafe {
                    sys::SDL_GL_BindTexture(texture.raw(), std::ptr::null_mut(), std::ptr::null_mut());
                    if let Some(gen_mm) = self.gl_generate_mipmap {
                        gen_mm(GL_TEXTURE_2D);
                    }
                    sys::SDL_GL_UnbindTexture(texture.raw());
                }
            }

            // Calculate destination rect maintaining aspect ratio
            let (win_w, win_h) = self.canvas.output_size()
                .map_err(|e| anyhow::anyhow!("Failed to get canvas size: {}", e))?;

            // Account for rotation: if 90° or 270°, use swapped frame dims for layout
            let (layout_w, layout_h) = if self.orientation.swaps_dimensions() {
                (self.frame_height, self.frame_width)
            } else {
                (self.frame_width, self.frame_height)
            };
            let dst_rect = Self::content_rect(win_w, win_h, layout_w, layout_h);

            self.canvas.clear();

            if self.orientation == Orientation::Normal {
                // Fast path: no rotation
                self.canvas.copy(texture, None, Some(dst_rect))
                    .map_err(|e| anyhow::anyhow!("Failed to copy texture: {}", e))?;
            } else {
                // Rotated rendering: use SDL_RenderCopyEx
                let angle = self.orientation.angle();

                let render_rect = if self.orientation.swaps_dimensions() {
                    // When rotated 90/270, SDL rotates around center, so we need
                    // to swap the dst rect dimensions and offset
                    Rect::new(
                        dst_rect.x() + (dst_rect.width() as i32 - dst_rect.height() as i32) / 2,
                        dst_rect.y() + (dst_rect.height() as i32 - dst_rect.width() as i32) / 2,
                        dst_rect.height(),
                        dst_rect.width(),
                    )
                } else {
                    dst_rect
                };

                self.canvas.copy_ex(
                    texture, None, Some(render_rect),
                    angle, None, false, false,
                ).map_err(|e| anyhow::anyhow!("Failed to render rotated: {}", e))?;
            }

            self.canvas.present();
        }

        // Re-register event watch with latest texture data
        self.update_resize_watch();

        Ok(())
    }

    /// Calculate the content rectangle preserving aspect ratio within the window
    pub fn content_rect(win_w: u32, win_h: u32, frame_w: u32, frame_h: u32) -> Rect {
        let (x, y, w, h) = content_rect_raw(win_w, win_h, frame_w, frame_h);
        Rect::new(x, y, w, h)
    }

    /// Toggle fullscreen mode
    pub fn toggle_fullscreen(&mut self) {
        use sdl2::video::FullscreenType;
        let new_mode = if self.fullscreen {
            FullscreenType::Off
        } else {
            FullscreenType::Desktop
        };
        let _ = self.canvas.window_mut().set_fullscreen(new_mode);
        self.fullscreen = !self.fullscreen;
    }

    /// Resize window to fit the frame while preserving aspect ratio (Mod+w)
    pub fn resize_to_fit(&mut self) {
        if self.fullscreen { return; }
        let (w, h) = Self::optimal_size(self.frame_width, self.frame_height);
        self.canvas.window_mut().set_size(w, h).ok();
        log::info!("Resized window to fit: {}x{}", w, h);
    }

    /// Resize window to pixel-perfect 1:1 frame size (Mod+g)
    pub fn resize_to_pixel_perfect(&mut self) {
        if self.fullscreen { return; }
        self.canvas.window_mut()
            .set_size(self.frame_width, self.frame_height).ok();
        log::info!("Resized to pixel-perfect: {}x{}",
            self.frame_width, self.frame_height);
    }

    /// Convert window pixel coordinates to frame coordinates
    pub fn window_to_frame_coords(&self, win_x: i32, win_y: i32) -> (u32, u32) {
        let (win_w, win_h) = self.canvas.output_size().unwrap_or((1, 1));
        let rect = Self::content_rect(win_w, win_h, self.frame_width, self.frame_height);

        let x = ((win_x - rect.x()) as f64 * self.frame_width as f64 / rect.width() as f64)
            .clamp(0.0, self.frame_width as f64 - 1.0) as u32;
        let y = ((win_y - rect.y()) as f64 * self.frame_height as f64 / rect.height() as f64)
            .clamp(0.0, self.frame_height as f64 - 1.0) as u32;

        (x, y)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // Remove event watch before destroying texture/renderer
        self.unregister_resize_watch();
        // Drop texture before texture_creator
        self.texture = None;
    }
}
