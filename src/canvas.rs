//! Drawing on the RGB565 framebuffer: the scaled game view, shapes and text.
//!
//! Shapes over the game view are mixed into each row before the row goes to PSRAM,
//! so the CPU never reads the framebuffer back for them.

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

/// How a shape mixes with what is under it.
#[derive(Clone, Copy)]
pub enum Paint {
    Solid(u16),
    /// Half of this color, half of the pixel under it.
    Glass(u16),
}

impl Paint {
    fn apply(self, pixels: &mut [u16]) {
        match self {
            Paint::Solid(color) => pixels.fill(color),
            Paint::Glass(color) => {
                for pixel in pixels {
                    *pixel = half_mix(*pixel, color);
                }
            }
        }
    }
}

/// A filled circle, or a ring when `inner` is above zero.
#[derive(Clone, Copy)]
pub struct Shape {
    pub x: i32,
    pub y: i32,
    pub outer: i32,
    pub inner: i32,
    pub paint: Paint,
}

impl Shape {
    /// Calls `f` with the start and the exclusive end of each span on row `y`.
    fn spans(&self, y: i32, mut f: impl FnMut(i32, i32)) {
        let dy = y - self.y;
        if dy.abs() > self.outer {
            return;
        }
        let half = isqrt(self.outer * self.outer - dy * dy);
        if dy.abs() < self.inner {
            let hole = isqrt(self.inner * self.inner - dy * dy);
            f(self.x - half, self.x - hole);
            f(self.x + hole + 1, self.x + half + 1);
        } else {
            f(self.x - half, self.x + half + 1);
        }
    }

    pub fn bounds(&self) -> Rect {
        Rect {
            x0: self.x - self.outer,
            y0: self.y - self.outer,
            x1: self.x + self.outer + 1,
            y1: self.y + self.outer + 1,
        }
    }
}

/// A rectangle on the panel. `x1` and `y1` are exclusive.
#[derive(Clone, Copy, Default)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

pub struct Canvas<'a> {
    /// The framebuffer's RGB565 pixels, row by row.
    frame: &'a mut [u16],
}

impl<'a> Canvas<'a> {
    pub fn new(frame: &'a mut [u8]) -> Self {
        // SAFETY: any two bytes make a valid `u16`.
        let (prefix, frame, _) = unsafe { frame.align_to_mut::<u16>() };
        assert!(prefix.is_empty());
        Self { frame }
    }

    /// Draws the game's 8-bit indexed picture, scaled up and centered, with the
    /// shapes on top.
    pub fn draw_view(&mut self, screen: &[u8], palette: &[u16; 256], shapes: &[Shape]) {
        let mut base = [0u16; VIEW_WIDTH];
        let mut mixed = [0u16; VIEW_WIDTH];
        for (y, line) in screen.as_chunks::<SCREENWIDTH>().0.iter().enumerate() {
            for (&index, out) in line.iter().zip(base.as_chunks_mut::<SCALE>().0) {
                out.fill(palette[usize::from(index)]);
            }
            for dy in 0..SCALE {
                let panel_y = (VIEW_Y + y * SCALE + dy) as i32;
                let mut is_mixed = false;
                for shape in shapes {
                    shape.spans(panel_y, |x0, x1| {
                        let x0 = (x0 - VIEW_X as i32).clamp(0, VIEW_WIDTH as i32) as usize;
                        let x1 = (x1 - VIEW_X as i32).clamp(0, VIEW_WIDTH as i32) as usize;
                        if x0 < x1 {
                            if !is_mixed {
                                mixed = base;
                                is_mixed = true;
                            }
                            shape.paint.apply(&mut mixed[x0..x1]);
                        }
                    });
                }
                let row = if is_mixed { &mixed } else { &base };
                self.write_row(panel_y as usize, VIEW_X, row);
            }
        }
    }

    /// Draws the parts of the shapes that are outside the game view.
    pub fn draw_around_view(&mut self, shapes: &[Shape]) {
        for shape in shapes {
            let bounds = shape.bounds();
            for y in bounds.y0..bounds.y1 {
                shape.spans(y, |x0, x1| self.span_around_view(y, x0, x1, shape.paint));
            }
        }
    }

    /// Fills the parts of the rectangle that are outside the game view.
    pub fn clear_around_view(&mut self, rect: Rect) {
        for y in rect.y0..rect.y1 {
            self.span_around_view(y, rect.x0, rect.x1, Paint::Solid(0));
        }
    }

    pub fn clear(&mut self) {
        self.frame.fill(0);
    }

    fn span_around_view(&mut self, y: i32, x0: i32, x1: i32, paint: Paint) {
        let view_rows = VIEW_Y as i32..(VIEW_Y + VIEW_HEIGHT) as i32;
        if view_rows.contains(&y) {
            self.span(y, x0, x1.min(VIEW_X as i32), paint);
            self.span(y, x0.max((VIEW_X + VIEW_WIDTH) as i32), x1, paint);
        } else {
            self.span(y, x0, x1, paint);
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
        let start = y as usize * WIDTH;
        paint.apply(&mut self.frame[start + x0..start + x1]);
    }

    fn write_row(&mut self, y: usize, x: usize, pixels: &[u16]) {
        let start = y * WIDTH + x;
        self.frame[start..start + pixels.len()].copy_from_slice(pixels);
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
                self.frame[point.y as usize * WIDTH + point.x as usize] =
                    RawU16::from(color).into_inner();
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
