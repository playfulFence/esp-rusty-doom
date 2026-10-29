//! The Goodix touch controller (GT911 or GT1151) on the I2C bus, with multi-touch.

use esp_hal::{
    Async,
    i2c::master::{Config, I2c},
    time::Rate,
};

use crate::{
    TouchResources,
    display::{HEIGHT, WIDTH},
};

/// The status register. Bit 7 tells that new data is ready, bits 0 to 3 count the
/// touch points. Writing 0 acknowledges the data.
const STATUS: [u8; 2] = [0x81, 0x4e];
/// The first touch point. Each point takes 8 bytes: track ID, X, Y and size (16-bit
/// little-endian each), and a reserved byte.
const POINTS: [u8; 2] = [0x81, 0x4f];
const POINT_BYTES: usize = 8;
pub const MAX_POINTS: usize = 5;

#[derive(Clone, Copy, Default)]
pub struct TouchPoint {
    /// Stays the same while the finger stays on the panel.
    pub id: u8,
    pub x: i32,
    pub y: i32,
}

/// The fingers on the panel.
#[derive(Clone, Copy, Default)]
pub struct Touches {
    points: [TouchPoint; MAX_POINTS],
    len: usize,
}

impl Touches {
    pub fn iter(&self) -> impl Iterator<Item = &TouchPoint> {
        self.points[..self.len].iter()
    }

    pub fn find(&self, id: u8) -> Option<&TouchPoint> {
        self.iter().find(|point| point.id == id)
    }
}

pub struct Touch {
    i2c: I2c<'static, Async>,
    address: u8,
}

impl Touch {
    /// Finds the controller at either of its I2C addresses.
    pub async fn new(r: TouchResources<'static>) -> Self {
        let mut i2c = I2c::new(r.i2c, Config::default().with_frequency(Rate::from_khz(400)))
            .unwrap()
            .with_sda(r.sda)
            .with_scl(r.scl)
            .into_async();

        for address in [0x14, 0x5d] {
            let mut status = [0u8; 1];
            if i2c
                .write_read_async(address, &STATUS, &mut status)
                .await
                .is_ok()
            {
                return Self { i2c, address };
            }
        }
        panic!("Goodix touch controller not found");
    }

    pub fn address(&self) -> u8 {
        self.address
    }

    /// Reads the controller. Returns `None` when it has no new data.
    pub async fn read(&mut self) -> Option<Touches> {
        let mut status = [0u8; 1];
        self.i2c
            .write_read_async(self.address, &STATUS, &mut status)
            .await
            .ok()?;
        if status[0] & 0x80 == 0 {
            return None;
        }

        let len = usize::from(status[0] & 0x0f).min(MAX_POINTS);
        let mut raw = [0u8; MAX_POINTS * POINT_BYTES];
        let read = if len == 0 {
            Ok(())
        } else {
            self.i2c
                .write_read_async(self.address, &POINTS, &mut raw[..len * POINT_BYTES])
                .await
        };
        let _ = self
            .i2c
            .write_async(self.address, &[STATUS[0], STATUS[1], 0])
            .await;
        read.ok()?;

        let mut touches = Touches {
            len,
            ..Default::default()
        };
        for (point, raw) in touches.points.iter_mut().zip(raw.chunks_exact(POINT_BYTES)) {
            let x = u16::from_le_bytes([raw[1], raw[2]]).min(WIDTH as u16 - 1);
            let y = u16::from_le_bytes([raw[3], raw[4]]).min(HEIGHT as u16 - 1);
            *point = TouchPoint {
                id: raw[0],
                x: i32::from(x),
                y: i32::from(y),
            };
        }
        Some(touches)
    }
}
