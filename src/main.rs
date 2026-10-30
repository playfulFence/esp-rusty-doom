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
use embedded_graphics::text::Alignment;
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
use static_cell::{ConstStaticCell, StaticCell};

use crate::{
    canvas::{Mask, VIEW_HEIGHT, VIEW_WIDTH, VIEW_X, VIEW_Y, rgb565},
    controls::Controls,
    display::{Display, HEIGHT, WIDTH},
    game::{Game, Status},
    pma::Pma,
    scene::{Overlays, Scene},
    touch::Touch,
};

mod canvas;
mod controls;
mod display;
mod game;
mod mailbox;
mod pma;
mod scene;
mod touch;

const WHITE: u16 = rgb565(255, 255, 255);

esp_bootloader_esp_idf::esp_app_desc!();

/// The WAD's size and its DEFLATE stream, made by `build.rs`.
static COMPRESSED_WAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wad.deflate"));

static GAME_STACK: ConstStaticCell<Stack<{ 64 * 1024 }>> = ConstStaticCell::new(Stack::new());
static OVERLAYS: StaticCell<Overlays> = StaticCell::new();

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

    let mut controls = Controls::new();
    let overlays = OVERLAYS.init(Overlays {
        labels: controls.labels(),
        loading: Mask::text(
            "Loading DOOM...",
            WIDTH as i32 / 2,
            HEIGHT as i32 / 2,
            WHITE,
        ),
        died: Mask::text(
            "You died. Press FIRE to restart.",
            WIDTH as i32 / 2,
            (VIEW_Y + VIEW_HEIGHT / 2) as i32,
            WHITE,
        ),
    });
    let mut loading = Box::new(Scene::new(overlays));
    loading.message = Some(&overlays.loading);
    scene::post(loading);
    let display = Display::new(resources.lcd);

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

    // A scene that was posted but never shown.
    let mut spare: Option<Box<Scene>> = None;

    let mut frames = 0u32;
    let mut fps = 0u32;
    let mut game_time = Duration::from_ticks(0);
    let mut report = Instant::now();

    loop {
        if let Some(touches) = touch.read().await {
            controls.update(&touches);
        }
        mailbox::post_input(controls.input());

        let Some(picture) = mailbox::take_picture() else {
            Timer::after(Duration::from_millis(1)).await;
            continue;
        };
        let mut next = scene::take_done()
            .or_else(|| spare.take())
            .unwrap_or_else(|| Box::new(Scene::new(overlays)));
        frames += 1;
        game_time += picture.game_time;
        update_hud(&mut next, &picture.map, picture.status, fps);
        next.message = picture
            .status
            .is_none_or(|status| status.health <= 0)
            .then_some(&overlays.died);
        next.shapes = Some(controls.shapes());
        if let Some(old) = next.picture.replace(picture) {
            mailbox::give_back(old);
        }
        if let Some(unshown) = scene::post(next) {
            spare = Some(unshown);
        }

        let elapsed = report.elapsed();
        if elapsed >= Duration::from_secs(5) {
            fps = (frames as u64 * 1000 / elapsed.as_millis()) as u32;
            println!(
                "{} fps, game {} ms per frame; drawing the panel takes {}% of this core; {} late refills, {} frames with LCD underruns",
                fps,
                game_time.as_millis() / frames as u64,
                display.take_draw_micros() as u64 * 100 / elapsed.as_micros(),
                display.take_late_refills(),
                display.take_underruns()
            );
            frames = 0;
            game_time = Duration::from_ticks(0);
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

/// Writes the line above the game view, when its text changes.
fn update_hud(scene: &mut Scene, map: &str, status: Option<Status>, fps: u32) {
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
    if scene.hud_text == text {
        return;
    }

    let hud = &mut scene.hud;
    let y = VIEW_Y as i32 / 2;
    hud.clear();
    hud.write(map, VIEW_X as i32, y, Alignment::Left);
    hud.write(&line, WIDTH as i32 / 2, y, Alignment::Center);
    hud.write(&fps, (VIEW_X + VIEW_WIDTH) as i32, y, Alignment::Right);
    scene.hud_text = text;
}
