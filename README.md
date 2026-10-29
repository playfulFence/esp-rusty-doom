# esp-doom

Doom in pure Rust on the [ESP32-S31-Korvo-1](https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32s31/esp32-s31-korvo-1/index.html),
with touch controls on the screen like in mobile shooters.

- The [`neurodoom`](https://crates.io/crates/neurodoom) crate runs the game: a `no_std` Rust port
  of the Doom engine.
- The game runs on the second core. The first core reads the touch panel and draws the picture,
  scaled up 2x, on the 800x480 LCD.
- Speed: about 28 frames per second, close to the panel's 30 Hz refresh.

## Controls

| Control | What it does |
| --- | --- |
| Left half of the screen | A stick appears under your finger: walk and strafe |
| Right half of the screen | Turn left and right |
| FIRE | Shoot |
| USE | Open doors, press switches |
| WPN | Next weapon that has ammo |

After you die, press FIRE to restart the level.

## Game data

The firmware needs a Doom IWAD. The build compresses it and puts it into the firmware. The
shareware `doom1.wad` (episode 1, freely distributable) fits.

Get it from the shareware v1.9 package:

```sh
mkdir -p ../wads && cd ../wads
curl -LO https://youfailit.net/pub/idgames/idstuff/doom/doom19s.zip
unzip doom19s.zip && cat DOOMS_19.1 DOOMS_19.2 > dooms.zip && unzip -o dooms.zip DOOM1.WAD
mv DOOM1.WAD doom1.wad
md5 doom1.wad # f0cefca49926d00903cf57551d901abe
```

By default, the build reads `../wads/doom1.wad`. To use another file, set `DOOM_WAD`:

```sh
DOOM_WAD=/path/to/doom1.wad cargo run --release
```

## Build and flash

```sh
cargo run --release
```

The image is about 2 MB, so `cargo run` flashes at 921600 baud.

## Notes

- The engine has no sound and no menus.
- The bootloader sets up the Physical Memory Attributes (PMA) only on the first core. The second
  core copies them before it touches PSRAM, see `src/pma.rs`.
- The renderer's depth buffer moves to internal RAM after loading. PSRAM is too slow for it.
