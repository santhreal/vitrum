//! Offscreen render target with pixel readback.
//!
//! This is the correctness harness for the renderer: draw a grid into a texture
//! nobody is looking at, copy it back to host memory, and assert on exact byte
//! values. No window, no swapchain, no compositor, so the same assertions run
//! on a developer machine and on a headless box.
//!
//! The target keeps its contents between renders. That is deliberate: it is how
//! a caller proves the no-change path really did nothing, by rendering an
//! unchanged frame and finding the previous pixels still there.

use crate::cell::Rgba;

/// Byte alignment `copy_texture_to_buffer` requires for each row.
const COPY_ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// An offscreen colour target plus the staging buffer used to read it back.
#[derive(Debug)]
pub struct HeadlessTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
}

impl HeadlessTarget {
    /// Texture format of the target.
    ///
    /// Non-sRGB on purpose. Terminal colours are already sRGB-encoded byte
    /// values, so writing them through a linear-to-sRGB conversion would change
    /// them. With this format a cell painted `#3366cc` reads back as exactly
    /// `0x33, 0x66, 0xcc`.
    pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// Allocate a `width` x `height` target.
    ///
    /// # Panics
    ///
    /// Panics when either dimension is zero, which would produce a texture no
    /// render pass can use.
    #[must_use]
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        assert!(
            width > 0 && height > 0,
            "headless target must have a non-zero size, got {width}x{height}"
        );
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vitrum-grid.headless-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: Self::FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let unpadded = width * 4;
        let padded_bytes_per_row = unpadded.div_ceil(COPY_ROW_ALIGNMENT) * COPY_ROW_ALIGNMENT;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vitrum-grid.headless-readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            texture,
            view,
            readback,
            width,
            height,
            padded_bytes_per_row,
        }
    }

    /// The view a render pass writes into.
    #[must_use]
    pub const fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Target width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Target height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Copy the texture back to host memory and return it as tightly packed
    /// RGBA8.
    ///
    /// Blocks until the GPU finishes, so the returned image reflects every
    /// command submitted before the call.
    ///
    /// # Panics
    ///
    /// Panics if the device is lost or the buffer mapping fails, both of which
    /// mean the GPU is in an unusable state and no assertion afterwards would
    /// be meaningful.
    #[must_use]
    pub fn read(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Image {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vitrum-grid.headless-readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            // A closed channel means the caller vanished; nothing to report to.
            let _ = tx.send(result);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device lost while waiting for headless readback");
        rx.recv()
            .expect("readback callback never ran despite a blocking poll")
            .expect("failed to map the headless readback buffer");

        let stride = self.padded_bytes_per_row as usize;
        let row_bytes = self.width as usize * 4;
        let mut pixels = vec![0u8; row_bytes * self.height as usize];
        {
            let mapped = slice.get_mapped_range();
            for y in 0..self.height as usize {
                let src = y * stride;
                pixels[y * row_bytes..(y + 1) * row_bytes]
                    .copy_from_slice(&mapped[src..src + row_bytes]);
            }
        }
        self.readback.unmap();

        Image {
            width: self.width,
            height: self.height,
            pixels,
        }
    }
}

/// Tightly packed RGBA8 pixels read back from the GPU.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Image {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Image {
    /// Image width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Image height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Raw `width * height * 4` bytes, row-major, RGBA order.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.pixels
    }

    /// The pixel at `(x, y)`.
    ///
    /// # Panics
    ///
    /// Panics when the coordinate is outside the image, because an out-of-range
    /// probe in a test is a broken test, not a runtime condition.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Rgba {
        assert!(
            x < self.width && y < self.height,
            "pixel ({x}, {y}) is outside a {}x{} image",
            self.width,
            self.height
        );
        let i = (y as usize * self.width as usize + x as usize) * 4;
        Rgba::from_bytes([
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ])
    }

    /// How many pixels exactly equal `color`.
    #[must_use]
    pub fn count(&self, color: Rgba) -> usize {
        let want = color.to_bytes();
        self.pixels.chunks_exact(4).filter(|px| *px == want).count()
    }

    /// Every distinct colour in the image with its pixel count, most frequent
    /// first. Useful for pinning down what a render actually produced when an
    /// exact-pixel assertion fails.
    #[must_use]
    pub fn palette(&self) -> Vec<(Rgba, usize)> {
        let mut counts: std::collections::HashMap<[u8; 4], usize> =
            std::collections::HashMap::new();
        for px in self.pixels.chunks_exact(4) {
            *counts.entry([px[0], px[1], px[2], px[3]]).or_insert(0) += 1;
        }
        let mut out: Vec<(Rgba, usize)> = counts
            .into_iter()
            .map(|(bytes, n)| (Rgba::from_bytes(bytes), n))
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_bytes().cmp(&b.0.to_bytes())));
        out
    }
}
