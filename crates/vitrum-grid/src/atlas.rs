//! The glyph atlas: one `R8Unorm` texture holding every rasterised glyph the
//! renderer has drawn, plus the shelf packer that places them.
//!
//! Coverage is 8 bits per pixel, not RGBA, because a monochrome mask is all a
//! terminal needs and it is a quarter of the bandwidth and memory. A
//! 2048 x 2048 atlas is 4 MiB and holds several thousand glyph boxes at typical
//! terminal sizes.
//!
//! When the atlas fills, it is reset rather than grown: the packer rewinds, the
//! entry map is emptied, and the generation counter bumps. The renderer watches
//! the generation and rebuilds the whole instance buffer when it changes,
//! because every cached atlas coordinate just became meaningless. A second
//! reset inside one frame means the frame genuinely needs more glyphs than the
//! atlas can hold, and that is reported instead of thrashing forever.

use std::collections::HashMap;

use crate::font::{FontStack, FontStyle, RasterGlyph};

/// ASCII is the overwhelming majority of terminal text, so those entries live
/// in a directly indexed table instead of behind a hash.
const ASCII_SLOTS: usize = 128;

/// The ASCII table is indexed by `FontStyle as usize`, so it must stay exactly
/// as wide as the variant list. Deriving the width means adding a variant is a
/// compile-time widening rather than a runtime out-of-bounds panic.
const STYLE_SLOTS: usize = FontStyle::ALL.len();

/// One pixel of empty space is kept around every glyph so a rounding error in
/// the shader can never sample a neighbour's ink.
const PADDING: u32 = 1;

/// Default atlas edge length. `wgpu`'s downlevel limit for a 2D texture is
/// 2048, so this is the largest square guaranteed available everywhere.
pub const DEFAULT_ATLAS_DIM: u32 = 2048;

/// Identity of a cached glyph bitmap.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GlyphKey {
    /// The character drawn.
    pub ch: char,
    /// Which face variant drew it.
    pub style: FontStyle,
}

/// Where a glyph lives in the atlas and how it sits inside its cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct AtlasEntry {
    /// Left edge in atlas pixels.
    pub x: u16,
    /// Top edge in atlas pixels.
    pub y: u16,
    /// Width in pixels. Zero means the glyph has no ink.
    pub w: u16,
    /// Height in pixels. Zero means the glyph has no ink.
    pub h: u16,
    /// Pixels from the cell's left edge to the bitmap's left edge.
    pub left: i16,
    /// Pixels from the cell's top edge to the bitmap's top edge.
    pub top: i16,
}

impl AtlasEntry {
    /// The entry used for a blank cell: nothing to sample.
    pub const BLANK: Self = Self {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        left: 0,
        top: 0,
    };

    /// True when the entry has no pixels.
    #[must_use]
    pub const fn is_blank(self) -> bool {
        self.w == 0 || self.h == 0
    }
}

/// Why a glyph could not be placed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AtlasError {
    /// The glyph is larger than the whole atlas. Only reachable with an
    /// enormous font size against a small atlas.
    GlyphTooLarge {
        /// Glyph width including padding.
        width: u32,
        /// Glyph height including padding.
        height: u32,
        /// Atlas edge length.
        dim: u32,
    },
    /// One frame needed more distinct glyphs than the atlas can hold, so a
    /// reset would immediately be followed by another. Raise the atlas
    /// dimension or lower the font size.
    Exhausted {
        /// Atlas edge length.
        dim: u32,
        /// How many glyphs were resident when the second reset was needed.
        resident: usize,
    },
}

impl core::fmt::Display for AtlasError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::GlyphTooLarge { width, height, dim } => write!(
                f,
                "glyph of {width}x{height} px does not fit a {dim}x{dim} atlas; \
                 reduce the font size or enlarge the atlas"
            ),
            Self::Exhausted { dim, resident } => write!(
                f,
                "one frame needed more glyphs than a {dim}x{dim} atlas holds \
                 ({resident} resident at the second reset); enlarge the atlas"
            ),
        }
    }
}

impl core::error::Error for AtlasError {}

/// Shelf-packed glyph cache backed by a single GPU texture.
pub struct GlyphAtlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    dim: u32,
    /// Next free x on the current shelf.
    cursor_x: u32,
    /// Top y of the current shelf.
    shelf_y: u32,
    /// Height of the current shelf.
    shelf_h: u32,
    entries: HashMap<GlyphKey, AtlasEntry>,
    ascii_entries: [[Option<AtlasEntry>; STYLE_SLOTS]; ASCII_SLOTS],
    generation: u64,
    resets_this_frame: u32,
}

impl core::fmt::Debug for GlyphAtlas {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GlyphAtlas")
            .field("dim", &self.dim)
            .field("resident", &self.resident())
            .field("generation", &self.generation)
            .finish()
    }
}

impl GlyphAtlas {
    /// Create a `dim` x `dim` coverage atlas.
    ///
    /// `dim` is clamped to the device's maximum 2D texture dimension and to at
    /// least 256, so a caller cannot accidentally request a texture the adapter
    /// will refuse.
    #[must_use]
    pub fn new(device: &wgpu::Device, dim: u32) -> Self {
        let dim = dim.clamp(256, device.limits().max_texture_dimension_2d);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vitrum-grid.glyph-atlas"),
            size: wgpu::Extent3d {
                width: dim,
                height: dim,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            dim,
            cursor_x: 0,
            shelf_y: 0,
            shelf_h: 0,
            entries: HashMap::new(),
            ascii_entries: [[None; STYLE_SLOTS]; ASCII_SLOTS],
            generation: 0,
            resets_this_frame: 0,
        }
    }

    /// The texture view the render pipeline binds.
    #[must_use]
    pub const fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Atlas edge length in pixels, after clamping.
    #[must_use]
    pub const fn dim(&self) -> u32 {
        self.dim
    }

    /// How many glyphs are currently placed.
    #[must_use]
    pub fn resident(&self) -> usize {
        let ascii = self
            .ascii_entries
            .iter()
            .flat_map(|styles| styles.iter())
            .filter(|entry| entry.is_some())
            .count();
        ascii + self.entries.len()
    }

    /// Bumped every time the atlas is reset. The renderer compares this against
    /// the value it saw last frame; a change means every cached coordinate is
    /// stale and the instance buffer has to be rebuilt.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Look up a glyph without inserting it.
    #[must_use]
    pub fn get(&self, key: GlyphKey) -> Option<AtlasEntry> {
        let val = key.ch as u32;
        if (val as usize) < ASCII_SLOTS {
            self.ascii_entries[val as usize][key.style as usize]
        } else {
            self.entries.get(&key).copied()
        }
    }

    /// Reset the per-frame reset counter. The renderer calls this once at the
    /// top of each frame.
    pub const fn begin_frame(&mut self) {
        self.resets_this_frame = 0;
    }

    /// Return the entry for `key`, rasterising and uploading it if this is the
    /// first time it has been seen.
    ///
    /// # Errors
    ///
    /// [`AtlasError::GlyphTooLarge`] when a single glyph cannot fit the atlas,
    /// and [`AtlasError::Exhausted`] when one frame needs more glyphs than the
    /// atlas holds.
    pub fn get_or_insert(
        &mut self,
        queue: &wgpu::Queue,
        fonts: &mut FontStack,
        key: GlyphKey,
    ) -> Result<AtlasEntry, AtlasError> {
        let val = key.ch as u32;
        if (val as usize) < ASCII_SLOTS {
            if let Some(entry) = self.ascii_entries[val as usize][key.style as usize] {
                return Ok(entry);
            }
        } else if let Some(entry) = self.entries.get(&key) {
            return Ok(*entry);
        }
        let glyph = fonts.rasterize(key.ch, key.style);
        self.insert_glyph(queue, key, &glyph)
    }

    /// Place an already-rasterised glyph. Exposed so a caller can pre-warm the
    /// atlas with a known character set before the first frame.
    ///
    /// # Errors
    ///
    /// Same conditions as [`GlyphAtlas::get_or_insert`].
    pub fn insert_glyph(
        &mut self,
        queue: &wgpu::Queue,
        key: GlyphKey,
        glyph: &RasterGlyph,
    ) -> Result<AtlasEntry, AtlasError> {
        if glyph.is_blank() {
            let val = key.ch as u32;
            if (val as usize) < ASCII_SLOTS {
                self.ascii_entries[val as usize][key.style as usize] = Some(AtlasEntry::BLANK);
            } else {
                self.entries.insert(key, AtlasEntry::BLANK);
            }
            return Ok(AtlasEntry::BLANK);
        }

        let need_w = glyph.width + PADDING * 2;
        let need_h = glyph.height + PADDING * 2;
        if need_w > self.dim || need_h > self.dim {
            return Err(AtlasError::GlyphTooLarge {
                width: need_w,
                height: need_h,
                dim: self.dim,
            });
        }

        let (x, y) = match self.allocate(need_w, need_h) {
            Some(slot) => slot,
            None => {
                if self.resets_this_frame >= 1 {
                    return Err(AtlasError::Exhausted {
                        dim: self.dim,
                        resident: self.resident(),
                    });
                }
                self.reset();
                // The size check above already proved this glyph fits an empty
                // atlas, so the retry cannot fail.
                self.allocate(need_w, need_h)
                    .expect("glyph smaller than the atlas must fit an empty atlas")
            }
        };

        let gx = x + PADDING;
        let gy = y + PADDING;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: gx, y: gy, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &glyph.coverage,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(glyph.width),
                rows_per_image: Some(glyph.height),
            },
            wgpu::Extent3d {
                width: glyph.width,
                height: glyph.height,
                depth_or_array_layers: 1,
            },
        );

        let entry = AtlasEntry {
            x: gx as u16,
            y: gy as u16,
            w: glyph.width as u16,
            h: glyph.height as u16,
            left: glyph.left.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
            top: glyph.top.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
        };
        let val = key.ch as u32;
        if (val as usize) < ASCII_SLOTS {
            self.ascii_entries[val as usize][key.style as usize] = Some(entry);
        } else {
            self.entries.insert(key, entry);
        }
        Ok(entry)
    }

    /// Reserve a `w` x `h` box, opening a new shelf when the current one is
    /// full. Returns `None` when the atlas has no vertical room left.
    fn allocate(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.cursor_x + w > self.dim {
            // Close the current shelf and start one below it.
            self.shelf_y += self.shelf_h;
            self.shelf_h = 0;
            self.cursor_x = 0;
        }
        let shelf_h = self.shelf_h.max(h);
        if self.shelf_y + shelf_h > self.dim {
            return None;
        }
        let slot = (self.cursor_x, self.shelf_y);
        self.cursor_x += w;
        self.shelf_h = shelf_h;
        Some(slot)
    }

    /// Rewind the packer and forget every entry.
    ///
    /// The texture keeps its old pixels; that is safe because nothing samples
    /// outside a live entry's rectangle, and overwriting 4 MiB just to look
    /// tidy would be a needless upload.
    fn reset(&mut self) {
        self.entries.clear();
        self.ascii_entries = [[None; STYLE_SLOTS]; ASCII_SLOTS];
        self.cursor_x = 0;
        self.shelf_y = 0;
        self.shelf_h = 0;
        self.generation += 1;
        self.resets_this_frame += 1;
    }
}
