//! Drawing on the RGB565 framebuffer: the scaled game view, shapes and text.

use embedded_graphics::{
    Pixel,
    pixelcolor::{Rgb565, raw::RawU16},
    prelude::*,
};
use neurodoom::{SCREENHEIGHT, SCREENWIDTH};

use crate::display::{HEIGHT, WIDTH};

/// Each game pixel covers 2x2 panel pixels.
pub const SCALE: usize = 2;
pub const VIEW_WIDTH: usize = SCREENWIDTH * SCALE;
pub const VIEW_HEIGHT: usize = SCREENHEIGHT * SCALE;
/// The top left corner of the centered game view.
pub const VIEW_X: usize = (WIDTH - VIEW_WIDTH) / 2;
pub const VIEW_Y: usize = (HEIGHT - VIEW_HEIGHT) / 2;

/// A rectangle on the panel. `x1` and `y1` are exclusive.
#[derive(Clone, Copy, Default)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub fn around(x: i32, y: i32, radius: i32) -> Self {
        Self {
            x0: x - radius,
            y0: y - radius,
            x1: x + radius + 1,
            y1: y + radius + 1,
        }
    }
}

/// How a shape mixes with what is under it.
#[derive(Clone, Copy)]
pub enum Paint {
    Solid(u16),
    /// Half of this color, half of the pixel under it.
    Glass(u16),
}

pub struct Canvas<'a> {
    frame: &'a mut [u8],
}

impl<'a> Canvas<'a> {
    pub fn new(frame: &'a mut [u8]) -> Self {
        Self { frame }
    }

    /// Draws the game's 8-bit indexed picture, scaled up and centered.
    pub fn draw_view(&mut self, screen: &[u8], palette: &[u16; 256]) {
        let mut row = [0u8; VIEW_WIDTH * 2];
        for (y, line) in screen.chunks_exact(SCREENWIDTH).enumerate() {
            for (&index, out) in line.iter().zip(row.chunks_exact_mut(SCALE * 2)) {
                let [lo, hi] = palette[usize::from(index)].to_le_bytes();
                out.copy_from_slice(&[lo, hi, lo, hi]);
            }
            for dy in 0..SCALE {
                let start = ((VIEW_Y + y * SCALE + dy) * WIDTH + VIEW_X) * 2;
                self.frame[start..start + row.len()].copy_from_slice(&row);
            }
        }
    }

    pub fn fill_rect(&mut self, rect: Rect, color: u16) {
        for y in rect.y0..rect.y1 {
            self.span(y, rect.x0, rect.x1, Paint::Solid(color));
        }
    }

    pub fn fill_circle(&mut self, cx: i32, cy: i32, radius: i32, paint: Paint) {
        for dy in -radius..=radius {
            let half = isqrt(radius * radius - dy * dy);
            self.span(cy + dy, cx - half, cx + half + 1, paint);
        }
    }

    /// A circle outline between the two radii.
    pub fn ring(&mut self, cx: i32, cy: i32, outer: i32, inner: i32, paint: Paint) {
        for dy in -outer..=outer {
            let half = isqrt(outer * outer - dy * dy);
            if dy.abs() < inner {
                let hole = isqrt(inner * inner - dy * dy);
                self.span(cy + dy, cx - half, cx - hole, paint);
                self.span(cy + dy, cx + hole + 1, cx + half + 1, paint);
            } else {
                self.span(cy + dy, cx - half, cx + half + 1, paint);
            }
        }
    }

    /// Paints row `y` from `x0` up to `x1`, clipped to the panel.
    fn span(&mut self, y: i32, x0: i32, x1: i32, paint: Paint) {
        if !(0..HEIGHT as i32).contains(&y) {
            return;
        }
        let x0 = x0.clamp(0, WIDTH as i32) as usize;
        let x1 = x1.clamp(0, WIDTH as i32) as usize;
        if x0 >= x1 {
            return;
        }
        let row = y as usize * WIDTH * 2;
        let pixels = &mut self.frame[row + x0 * 2..row + x1 * 2];
        match paint {
            Paint::Solid(color) => {
                let bytes = color.to_le_bytes();
                for pixel in pixels.chunks_exact_mut(2) {
                    pixel.copy_from_slice(&bytes);
                }
            }
            Paint::Glass(color) => {
                for pixel in pixels.chunks_exact_mut(2) {
                    let under = u16::from_le_bytes([pixel[0], pixel[1]]);
                    pixel.copy_from_slice(&half_mix(under, color).to_le_bytes());
                }
            }
        }
    }
}

impl DrawTarget for Canvas<'_> {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if (0..WIDTH as i32).contains(&point.x) && (0..HEIGHT as i32).contains(&point.y) {
                let index = (point.y as usize * WIDTH + point.x as usize) * 2;
                let raw = RawU16::from(color).into_inner();
                self.frame[index..index + 2].copy_from_slice(&raw.to_le_bytes());
            }
        }
        Ok(())
    }
}

impl OriginDimensions for Canvas<'_> {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

/// The RGB565 value of an 8-bit-per-channel color.
pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

/// The average of two RGB565 colors.
fn half_mix(a: u16, b: u16) -> u16 {
    ((a & 0xf7de) >> 1) + ((b & 0xf7de) >> 1)
}

pub fn isqrt(value: i32) -> i32 {
    if value <= 0 {
        return 0;
    }
    let mut root = value;
    let mut next = (root + 1) / 2;
    while next < root {
        root = next;
        next = (root + value / root) / 2;
    }
    root
}
