//! Doom on the ESP32-S31-Korvo-1, in pure Rust, with touch controls on the screen.
//!
//! The [`neurodoom`] engine runs the game from a WAD file that is built into the
//! firmware. The left half of the screen is a floating stick that walks and strafes,
//! the right half turns, and the buttons fire, use (doors and switches) and change
//! weapons.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::{boxed::Box, format, string::String, vec};

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
    system::Stack,
    timer::timg::TimerGroup,
};
use esp_println::println;
use miniz_oxide::inflate::{
    self,
    TINFLStatus,
    core::{DecompressorOxide, inflate_flags},
};
use static_cell::ConstStaticCell;

use crate::{
    canvas::{Canvas, VIEW_HEIGHT, VIEW_WIDTH, VIEW_X, VIEW_Y},
    controls::Controls,
    display::{Display, HEIGHT, WIDTH},
    game::{Game, Status},
    mailbox::Picture,
    pma::Pma,
    touch::Touch,
};

mod canvas;
mod controls;
mod display;
mod game;
mod mailbox;
mod pma;
mod touch;

esp_bootloader_esp_idf::esp_app_desc!();

/// The WAD's size and its DEFLATE stream, made by `build.rs`.
static COMPRESSED_WAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wad.deflate"));

static GAME_STACK: ConstStaticCell<Stack<{ 64 * 1024 }>> = ConstStaticCell::new(Stack::new());

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

    // The game runs on the second core, and this core draws its pictures.
    let pma = Pma::read();
    let game_stack = GAME_STACK.take();
    esp_rtos::start_second_core(peripherals.CPU_CTRL, game_stack, move || {
        pma.apply();
        let started = Instant::now();
        let mut game = Game::new(wad);
        // Internal RAM that the game's loading does not fill.
        esp_alloc::heap_allocator!(size: 252 * 1024);
        game.move_depth_buffer_to_internal_ram();
        println!(
            "Loaded {} in {} ms",
            game.map_name(),
            started.elapsed().as_millis()
        );
        println!("{}", esp_alloc::HEAP.stats());
        game.run()
    });

    let mut controls = Controls::new();
    let mut drawn = controls.bounds();
    let mut hud = String::new();
    let mut shown: Option<Picture> = None;
    let mut loading = true;

    let mut frames = 0u32;
    let mut fps = 0u32;
    let mut game_time = Duration::from_ticks(0);
    let mut draw_time = Duration::from_ticks(0);
    let mut report = Instant::now();

    loop {
        if let Some(touches) = touch.read().await {
            controls.update(&touches);
        }
        mailbox::post_input(controls.input());

        let Some(picture) = mailbox::take_picture(&mut shown) else {
            Timer::after(Duration::from_millis(1)).await;
            continue;
        };
        if loading {
            // The first picture replaces the loading message.
            Canvas::new(&mut display).clear();
            loading = false;
        }

        let draw_start = Instant::now();
        {
            let mut canvas = Canvas::new(&mut display);
            for rect in drawn {
                canvas.clear_around_view(rect);
            }
            let shapes = controls.shapes();
            canvas.draw_view(&picture.screen, &picture.palette, &shapes);
            canvas.draw_around_view(&shapes);
            controls.draw_labels(&mut canvas);
            drawn = controls.bounds();

            draw_hud(&mut canvas, &mut hud, &picture.map, picture.status, fps);
            if picture.status.is_none_or(|status| status.health <= 0) {
                draw_centered(
                    &mut canvas,
                    "You died. Press FIRE to restart.",
                    (VIEW_Y + VIEW_HEIGHT / 2) as i32,
                );
            }
        }
        display.writeback(0..HEIGHT);

        frames += 1;
        draw_time += draw_start.elapsed();
        game_time += picture.game_time;
        shown = Some(picture);

        let elapsed = report.elapsed();
        if elapsed >= Duration::from_secs(5) {
            fps = (frames as u64 * 1000 / elapsed.as_millis()) as u32;
            println!(
                "{} fps, per frame: game {} ms, drawing {} ms",
                fps,
                game_time.as_millis() / frames as u64,
                draw_time.as_millis() / frames as u64
            );
            frames = 0;
            game_time = Duration::from_ticks(0);
            draw_time = Duration::from_ticks(0);
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

/// Draws the line above the game view when its text changes.
fn draw_hud(
    canvas: &mut Canvas<'_>,
    last: &mut String,
    map: &str,
    status: Option<Status>,
    fps: u32,
) {
    let status = status.unwrap_or(Status {
        health: 0,
        armor: 0,
        ammo: None,
    });
    let ammo = match status.ammo {
        Some(ammo) => format!("{ammo:3}"),
        None => String::from("  -"),
    };
    let line = format!(
        "HEALTH {:3}%   ARMOR {:3}%   AMMO {}",
        status.health.max(0),
        status.armor,
        ammo
    );
    let fps = format!("{fps:2} FPS");
    let text = format!("{map}{line}{fps}");
    if *last == text {
        return;
    }
    *last = text;

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
    let _ =
        Text::with_text_style(&line, Point::new(WIDTH as i32 / 2, y), style, center).draw(canvas);
    let _ = Text::with_text_style(
        &fps,
        Point::new((VIEW_X + VIEW_WIDTH) as i32, y),
        style,
        right,
    )
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
