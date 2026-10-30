//! Drawing primitives for panel rows: the scaled game view, round shapes and text
//! masks. All of them work on one RGB565 row at a time.

use alloc::{vec, vec::Vec};

use embedded_graphics::{
    Pixel,
    mono_font::{MonoTextStyle, ascii::FONT_10X20},
    pixelcolor::BinaryColor,
    prelude::*,
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use esp_hal::ram;
use neurodoom::{SCREENHEIGHT, SCREENWIDTH};

use crate::display::{HEIGHT, WIDTH};

/// Each game pixel covers 2x2 panel pixels.
pub const SCALE: usize = 2;
pub const VIEW_WIDTH: usize = SCREENWIDTH * SCALE;
pub const VIEW_HEIGHT: usize = SCREENHEIGHT * SCALE;
/// The top left corner of the centered game view.
pub const VIEW_X: usize = (WIDTH - VIEW_WIDTH) / 2;
pub const VIEW_Y: usize = (HEIGHT - VIEW_HEIGHT) / 2;

/// Draws one row of the game view, scaled up, with black on both sides.
#[ram]
pub fn draw_view_row(screen_row: &[u8], palette: &[u16; 256], row: &mut [u16]) {
    row[..VIEW_X].fill(0);
    row[VIEW_X + VIEW_WIDTH..].fill(0);
    let view = &mut row[VIEW_X..VIEW_X + VIEW_WIDTH];
    for (&index, out) in screen_row.iter().zip(view.as_chunks_mut::<SCALE>().0) {
        out.fill(palette[usize::from(index)]);
    }
}

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
    /// Paints the shape's part of panel row `y`.
    #[ram]
    pub fn draw_row(&self, y: i32, row: &mut [u16]) {
        let dy = y - self.y;
        if dy.abs() > self.outer {
            return;
        }
        let half = isqrt(self.outer * self.outer - dy * dy);
        if dy.abs() < self.inner {
            let hole = isqrt(self.inner * self.inner - dy * dy);
            self.span(row, self.x - half, self.x - hole);
            self.span(row, self.x + hole + 1, self.x + half + 1);
        } else {
            self.span(row, self.x - half, self.x + half + 1);
        }
    }

    fn span(&self, row: &mut [u16], x0: i32, x1: i32) {
        let x0 = x0.clamp(0, WIDTH as i32) as usize;
        let x1 = x1.clamp(0, WIDTH as i32) as usize;
        if x0 < x1 {
            self.paint.apply(&mut row[x0..x1]);
        }
    }
}

/// A one-color picture with one bit per pixel, for text.
pub struct Mask {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    words_per_row: usize,
    bits: Vec<u32>,
    color: u16,
}

impl Mask {
    /// An empty mask that covers the given panel area.
    pub fn new(x: i32, y: i32, width: i32, height: i32, color: u16) -> Self {
        let words_per_row = (width as usize).div_ceil(32);
        Self {
            x,
            y,
            width,
            height,
            words_per_row,
            bits: vec![0; words_per_row * height as usize],
            color,
        }
    }

    /// A mask with one line of text, centered on the point.
    pub fn text(text: &str, x: i32, y: i32, color: u16) -> Self {
        let size = FONT_10X20.character_size;
        let (width, height) = (text.len() as i32 * size.width as i32, size.height as i32);
        let mut mask = Self::new(
            x - width / 2 - 1,
            y - height / 2 - 1,
            width + 2,
            height + 2,
            color,
        );
        mask.write(text, x, y, Alignment::Center);
        mask
    }

    /// Writes one line of text, aligned on `x` and centered on `y`.
    pub fn write(&mut self, text: &str, x: i32, y: i32, alignment: Alignment) {
        let style = MonoTextStyle::new(&FONT_10X20, BinaryColor::On);
        let text_style = TextStyleBuilder::new()
            .alignment(alignment)
            .baseline(Baseline::Middle)
            .build();
        let _ = Text::with_text_style(text, Point::new(x, y), style, text_style).draw(self);
    }

    pub fn clear(&mut self) {
        self.bits.fill(0);
    }

    /// Paints the mask's part of panel row `y`.
    #[ram]
    pub fn draw_row(&self, y: i32, row: &mut [u16]) {
        let line = y - self.y;
        if !(0..self.height).contains(&line) {
            return;
        }
        let words = &self.bits[line as usize * self.words_per_row..][..self.words_per_row];
        for (index, &word) in words.iter().enumerate() {
            let mut word = word;
            while word != 0 {
                let x = self.x + (index * 32) as i32 + word.trailing_zeros() as i32;
                if (0..WIDTH as i32).contains(&x) {
                    row[x as usize] = self.color;
                }
                word &= word - 1;
            }
        }
    }
}

/// Draws in panel coordinates. Pixels outside the mask are dropped.
impl DrawTarget for Mask {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            let (x, y) = (point.x - self.x, point.y - self.y);
            if color.is_on() && (0..self.width).contains(&x) && (0..self.height).contains(&y) {
                let index = y as usize * self.words_per_row + x as usize / 32;
                self.bits[index] |= 1 << (x % 32);
            }
        }
        Ok(())
    }
}

impl OriginDimensions for Mask {
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

#[inline]
pub fn isqrt(value: i32) -> i32 {
    if value <= 0 {
        return 0;
    }
    // A power of two above the root, so that Newton's method needs few steps.
    let mut root = 1 << (32 - value.leading_zeros()).div_ceil(2);
    loop {
        let next = (root + value / root) / 2;
        if next >= root {
            return root;
        }
        root = next;
    }
}
