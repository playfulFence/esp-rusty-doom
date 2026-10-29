//! Doom on the ESP32-S31-Korvo-1, in pure Rust, with touch controls on the screen.
//!
//! The [`neurodoom`] engine runs the game from a WAD file that is built into the
//! firmware. The left half of the screen is a floating stick that walks and strafes,
//! the right half turns, and the buttons fire, use (doors and switches) and change
//! weapons.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::{boxed::Box, format, vec};

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::{
    mono_font::{MonoTextStyle, MonoTextStyleBuilder, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use esp_backtrace as _;
use esp_hal::{
    assign_resources,
    clock::{ClockConfig, CpuClock},
    ram,
    timer::timg::TimerGroup,
};
use esp_println::println;
use miniz_oxide::inflate::{
    self,
    TINFLStatus,
    core::{DecompressorOxide, inflate_flags},
};

use crate::{
    canvas::{Canvas, Rect, VIEW_X, VIEW_Y, VIEW_WIDTH, VIEW_HEIGHT},
    controls::Controls,
    display::{Display, HEIGHT, WIDTH},
    game::{Game, Status, TICRATE},
    touch::Touch,
};

mod canvas;
mod controls;
mod display;
mod game;
mod touch;

esp_bootloader_esp_idf::esp_app_desc!();

/// The WAD's size and its DEFLATE stream, made by `build.rs`.
static COMPRESSED_WAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wad.deflate"));

const TIC: Duration = Duration::from_micros(1_000_000 / TICRATE as u64);
/// When rendering falls behind, the game runs at most this many tics per frame and
/// then slows down.
const MAX_TICS_PER_FRAME: u32 = 3;

// ESP32-S31-Korvo-1 with its LCD subboard.
assign_resources! {
    pub Resources<'d> {
        lcd: LcdResources<'d> {
            lcd_cam: LCD_CAM,
            dma: DMA_AXI_CH0,
            vsync: GPIO45,
            hsync: GPIO44,
            de: GPIO43,
            pclk: GPIO40,
            data0: GPIO8,
            data1: GPIO9,
            data2: GPIO10,
            data3: GPIO11,
            data4: GPIO12,
            data5: GPIO13,
            data6: GPIO14,
            data7: GPIO15,
            data8: GPIO16,
            data9: GPIO17,
            data10: GPIO18,
            data11: GPIO19,
            data12: GPIO33,
            data13: GPIO34,
            data14: GPIO35,
            data15: GPIO36,
        },
        touch: TouchResources<'d> {
            i2c: I2C0,
            sda: GPIO0,
            scl: GPIO1,
        },
    }
}

#[esp_hal::main]
async fn main(_spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let peripherals = esp_hal::init(
        esp_hal::Config::default().with_cpu_clock(ClockConfig::from(CpuClock::max())),
    );
    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 48 * 1024);
    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);

    let timer = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timer.timer0);

    let resources = split_resources!(peripherals);

    let mut display = Display::new(resources.lcd);
    {
        let mut canvas = Canvas::new(&mut display);
        draw_centered(&mut canvas, "Loading DOOM...", HEIGHT as i32 / 2);
    }
    display.writeback(0..HEIGHT);

    let mut touch = Touch::new(resources.touch).await;
    println!("Goodix touch controller at 0x{:02x}", touch.address());

    let started = Instant::now();
    let wad = decompress_wad();
    println!(
        "Decompressed the {} byte WAD in {} ms",
        wad.len(),
        started.elapsed().as_millis()
    );
    let started = Instant::now();
    let mut game = Game::new(wad);
    println!(
        "Loaded {} in {} ms",
        game.map_name(),
        started.elapsed().as_millis()
    );
    println!("{}", esp_alloc::HEAP.stats());

    let mut controls = Controls::new();
    let mut drawn = controls.bounds();
    {
        let mut canvas = Canvas::new(&mut display);
        canvas.fill_rect(
            Rect {
                x0: 0,
                y0: 0,
                x1: WIDTH as i32,
                y1: HEIGHT as i32,
            },
            0,
        );
    }

    let mut next_tic = Instant::now();
    let mut frames = 0u32;
    let mut tics = 0u32;
    let mut fps = 0u32;
    let mut busy = Duration::from_ticks(0);
    let mut report = Instant::now();

    loop {
        if let Some(touches) = touch.read().await {
            controls.update(&touches);
        }
        let input = controls.input();
        if input.next_weapon {
            game.next_weapon();
        }

        let now = Instant::now();
        if now < next_tic {
            Timer::at(next_tic).await;
            continue;
        }
        let frame_start = now;
        let mut ran = 0;
        while next_tic <= now && ran < MAX_TICS_PER_FRAME {
            game.tic(&input);
            next_tic += TIC;
            ran += 1;
        }
        if next_tic <= now {
            next_tic = now + TIC;
        }
        tics += ran;

        game.render();
        {
            let mut canvas = Canvas::new(&mut display);
            for rect in drawn {
                canvas.fill_rect(rect, 0);
            }
            canvas.draw_view(game.screen(), game.palette());
            let status = game.status();
            draw_hud(&mut canvas, game.map_name(), status, fps);
            if status.is_none_or(|status| status.health <= 0) {
                draw_centered(
                    &mut canvas,
                    "You died. Press FIRE to restart.",
                    (VIEW_Y + VIEW_HEIGHT / 2) as i32,
                );
            }
            controls.draw(&mut canvas);
            drawn = controls.bounds();
        }
        display.writeback(0..HEIGHT);

        frames += 1;
        busy += frame_start.elapsed();
        let elapsed = report.elapsed();
        if elapsed >= Duration::from_secs(5) {
            fps = (frames as u64 * 1000 / elapsed.as_millis()) as u32;
            println!(
                "{} fps, {} tics/s, {} ms per frame",
                fps,
                tics as u64 * 1000 / elapsed.as_millis(),
                busy.as_millis() / frames.max(1) as u64
            );
            frames = 0;
            tics = 0;
            busy = Duration::from_ticks(0);
            report = Instant::now();
        }
    }
}

/// Unpacks the WAD into PSRAM.
fn decompress_wad() -> &'static [u8] {
    let (size, stream) = COMPRESSED_WAD.split_at(4);
    let size = u32::from_le_bytes(size.try_into().unwrap()) as usize;
    let mut wad = vec![0u8; size];
    let mut decompressor = Box::<DecompressorOxide>::default();
    let (status, _, written) = inflate::core::decompress(
        &mut decompressor,
        stream,
        &mut wad,
        0,
        inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
    );
    assert!(
        status == TINFLStatus::Done && written == size,
        "corrupt WAD: {status:?}"
    );
    wad.leak()
}

fn draw_hud(canvas: &mut Canvas<'_>, map: &str, status: Option<Status>, fps: u32) {
    let style = MonoTextStyleBuilder::new()
        .font(&FONT_10X20)
        .text_color(Rgb565::new(31, 50, 0))
        .background_color(Rgb565::BLACK)
        .build();
    let y = VIEW_Y as i32 / 2;
    let left = TextStyleBuilder::new().baseline(Baseline::Middle).build();
    let center = TextStyleBuilder::new()
        .alignment(Alignment::Center)
        .baseline(Baseline::Middle)
        .build();
    let right = TextStyleBuilder::new()
        .alignment(Alignment::Right)
        .baseline(Baseline::Middle)
        .build();

    let _ = Text::with_text_style(map, Point::new(VIEW_X as i32, y), style, left).draw(canvas);

    let status = status.unwrap_or(Status {
        health: 0,
        armor: 0,
        ammo: None,
    });
    let ammo = match status.ammo {
        Some(ammo) => format!("{ammo:3}"),
        None => format!("  -"),
    };
    let line = format!(
        "HEALTH {:3}%   ARMOR {:3}%   AMMO {}",
        status.health.max(0),
        status.armor,
        ammo
    );
    let _ = Text::with_text_style(&line, Point::new(WIDTH as i32 / 2, y), style, center)
        .draw(canvas);

    let fps = format!("{fps:2} FPS");
    let _ = Text::with_text_style(&fps, Point::new((VIEW_X + VIEW_WIDTH) as i32, y), style, right)
        .draw(canvas);
}

fn draw_centered(canvas: &mut Canvas<'_>, text: &str, y: i32) {
    let style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
    let text_style = TextStyleBuilder::new()
        .alignment(Alignment::Center)
        .baseline(Baseline::Middle)
        .build();
    let _ = Text::with_text_style(text, Point::new(WIDTH as i32 / 2, y), style, text_style)
        .draw(canvas);
}
