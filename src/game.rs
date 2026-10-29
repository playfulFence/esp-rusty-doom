//! The Doom game around the engine: inputs, weapon switching, levels, death and
//! the screen palette.

use alloc::{boxed::Box, format, string::String, vec::Vec};

use esp_println::println;
use neurodoom::{
    AmmoType,
    Button,
    Buttons,
    ClassicEngine,
    LevelExit,
    PeerId,
    PlayerAction,
    WeaponType,
};

use crate::{canvas::rgb565, controls::Input};

const PLAYER: PeerId = PeerId(0);
const FIRST_MAP: &str = "E1M1";
pub const TICRATE: u32 = 35;

/// The order in which the weapon button goes through the weapons. The super shotgun
/// is only in Doom II, and the engine can't select it.
const WEAPON_ORDER: [WeaponType; 8] = [
    WeaponType::Fist,
    WeaponType::Chainsaw,
    WeaponType::Pistol,
    WeaponType::Shotgun,
    WeaponType::Chaingun,
    WeaponType::RocketLauncher,
    WeaponType::PlasmaRifle,
    WeaponType::Bfg,
];

/// Palettes in the WAD's PLAYPAL lump: 0 is normal, 1 to 8 get redder with damage,
/// 9 to 12 flash yellow for pickups.
const DAMAGE_PALETTES: usize = 1;
const BONUS_PALETTES: usize = 9;

/// What the HUD shows.
#[derive(Clone, Copy, PartialEq)]
pub struct Status {
    pub health: i32,
    pub armor: i32,
    pub ammo: Option<i32>,
}

pub struct Game {
    wad: &'static [u8],
    engine: Box<ClassicEngine>,
    map: String,
    /// RGB565 versions of the WAD's palettes.
    palettes: Vec<[u16; 256]>,
    /// The weapon that the player switches to, and the tics left to wait for it.
    switching: Option<(WeaponType, u32)>,
    dead_tics: u32,
    restart_held: bool,
    /// Doom's screen flashes: they count down by one each tic.
    damage_count: i32,
    bonus_count: i32,
    last_status: Option<(Status, i32)>,
}

impl Game {
    pub fn new(wad: &'static [u8]) -> Self {
        let engine = Box::new(ClassicEngine::new(wad, FIRST_MAP).expect("cannot load the WAD"));
        let palettes = engine
            .textures()
            .palettes
            .iter()
            .map(|palette| {
                let mut colors = [0u16; 256];
                for (color, rgb) in colors.iter_mut().zip(palette.chunks_exact(3)) {
                    *color = rgb565(rgb[0], rgb[1], rgb[2]);
                }
                colors
            })
            .collect();
        Self {
            wad,
            engine,
            map: String::from(FIRST_MAP),
            palettes,
            switching: None,
            dead_tics: 0,
            restart_held: false,
            damage_count: 0,
            bonus_count: 0,
            last_status: None,
        }
    }

    pub fn map_name(&self) -> &str {
        &self.map
    }

    /// Runs one 1/35 s step of the game.
    pub fn tic(&mut self, input: &Input) {
        let pressed = input.fire || input.use_;
        if self.status().is_none_or(|status| status.health <= 0) {
            self.dead_tics += 1;
            // Like Doom: after a moment, a new press restarts the level.
            if pressed && !self.restart_held && self.dead_tics > TICRATE {
                println!("Restarting {}", self.map);
                self.load(None);
            }
        } else {
            self.dead_tics = 0;
        }
        self.restart_held = pressed;

        let mut buttons = Buttons::empty();
        if input.fire {
            buttons |= Button::Attack;
        }
        if input.use_ {
            buttons |= Button::Use;
        }
        let action = PlayerAction {
            forward_move: input.forward,
            side_move: input.side,
            angle_turn: input.turn,
            buttons,
            weapon_select: self.weapon_select(),
        };
        self.engine.simulate(&[(PLAYER, action)], &[]);

        self.update_flashes();
        if self.engine.level_complete() {
            self.next_level();
        }
    }

    /// Draws the player's view into the indexed screen buffer.
    pub fn render(&mut self) {
        self.engine.render_for(PLAYER);
    }

    /// The last rendered picture, one palette index per pixel.
    pub fn screen(&self) -> &[u8] {
        &self.engine.renderer().screen
    }

    /// The palette for the picture, with the damage and pickup flashes.
    pub fn palette(&self) -> &[u16; 256] {
        let index = if self.damage_count > 0 {
            DAMAGE_PALETTES + ((self.damage_count + 7) / 8).clamp(1, 8) as usize - 1
        } else if self.bonus_count > 0 {
            BONUS_PALETTES + ((self.bonus_count + 7) / 8).clamp(1, 4) as usize - 1
        } else {
            0
        };
        self.palettes.get(index).unwrap_or(&self.palettes[0])
    }

    pub fn status(&self) -> Option<Status> {
        let world = self.engine.world();
        let player = world.controlled_by(PLAYER)?;
        let health = world.get(player)?.health;
        let state = world.player_state(player)?;
        Some(Status {
            health,
            armor: state.armor_points,
            ammo: ammo_type(state.ready_weapon).map(|ammo| state.ammo[ammo as usize]),
        })
    }

    /// Starts switching to the next weapon that the player has, with ammo.
    pub fn next_weapon(&mut self) {
        let world = self.engine.world();
        let Some(state) = world
            .controlled_by(PLAYER)
            .and_then(|player| world.player_state(player))
        else {
            return;
        };
        let current = self.switching.map_or(state.ready_weapon, |(weapon, _)| weapon);
        let start = WEAPON_ORDER.iter().position(|&w| w == current).unwrap_or(0);
        let next = (1..WEAPON_ORDER.len())
            .map(|step| WEAPON_ORDER[(start + step) % WEAPON_ORDER.len()])
            .find(|&weapon| {
                state.weapon_owned[weapon as usize]
                    && ammo_type(weapon).is_none_or(|ammo| state.ammo[ammo as usize] > 0)
            });
        if let Some(weapon) = next {
            self.switching = Some((weapon, TICRATE * 2));
        }
    }

    /// The engine changes weapons only when the current one is ready, so the request
    /// repeats until the switch happens.
    fn weapon_select(&mut self) -> u8 {
        let Some((weapon, tics)) = self.switching else {
            return 0;
        };
        let world = self.engine.world();
        let ready = world
            .controlled_by(PLAYER)
            .and_then(|player| world.player_state(player))
            .map(|state| state.ready_weapon);
        if ready == Some(weapon) || tics == 0 {
            self.switching = None;
            return 0;
        }
        self.switching = Some((weapon, tics - 1));
        weapon as u8 + 1
    }

    fn update_flashes(&mut self) {
        self.damage_count = (self.damage_count - 1).max(0);
        self.bonus_count = (self.bonus_count - 1).max(0);

        let Some(status) = self.status() else {
            return;
        };
        let world = self.engine.world();
        let weapons = world
            .controlled_by(PLAYER)
            .and_then(|player| world.player_state(player))
            .map_or(0, |state| {
                state.weapon_owned.iter().filter(|&&owned| owned).count() as i32
                    + state.ammo.iter().sum::<i32>()
            });
        if let Some((last, last_weapons)) = self.last_status {
            if status.health < last.health {
                self.damage_count = (self.damage_count + last.health - status.health).min(100);
            } else if status.health > last.health
                || status.armor > last.armor
                || weapons > last_weapons
            {
                self.bonus_count += 6;
            }
        }
        self.last_status = Some((status, weapons));
    }

    fn next_level(&mut self) {
        let secret = self.engine.level_exit() == LevelExit::Secret;
        match next_map(&self.map, secret) {
            Some(map) => {
                println!("{} cleared, loading {}", self.map, map);
                self.map = map;
                let carried = self.carried_state();
                self.load(carried);
            }
            None => {
                println!("{} cleared, the episode is over", self.map);
                self.map = String::from(FIRST_MAP);
                self.load(None);
            }
        }
    }

    /// The health, armor, weapons and ammo that the player takes to the next level.
    fn carried_state(&self) -> Option<(neurodoom::PlayerState, i32)> {
        let world = self.engine.world();
        let player = world.controlled_by(PLAYER)?;
        Some((world.player_state(player)?.clone(), world.get(player)?.health))
    }

    /// Loads the current map, and gives the player the carried state, if any.
    fn load(&mut self, carried: Option<(neurodoom::PlayerState, i32)>) {
        if let Err(error) = self.engine.load_map(self.wad, &self.map) {
            println!("Cannot load {}: {:?}", self.map, error);
            self.map = String::from(FIRST_MAP);
            self.engine.load_map(self.wad, &self.map).unwrap();
        }
        self.engine.spawn_map_things(PLAYER);
        self.switching = None;
        self.damage_count = 0;
        self.bonus_count = 0;
        self.last_status = None;

        let Some((old, health)) = carried else {
            return;
        };
        let world = self.engine.world_mut();
        let Some(player) = world.controlled_by(PLAYER) else {
            return;
        };
        if let Some(state) = world.player_state_mut(player) {
            state.armor_points = old.armor_points;
            state.armor_type = old.armor_type;
            state.ammo = old.ammo;
            state.max_ammo = old.max_ammo;
            state.weapon_owned = old.weapon_owned;
        }
        if let Some(entity) = world.get_mut(player) {
            entity.health = health;
        }
    }
}

fn ammo_type(weapon: WeaponType) -> Option<AmmoType> {
    match weapon {
        WeaponType::Fist | WeaponType::Chainsaw => None,
        WeaponType::Pistol | WeaponType::Chaingun => Some(AmmoType::Bullets),
        WeaponType::Shotgun | WeaponType::SuperShotgun => Some(AmmoType::Shells),
        WeaponType::PlasmaRifle | WeaponType::Bfg => Some(AmmoType::Cells),
        WeaponType::RocketLauncher => Some(AmmoType::Rockets),
    }
}

/// The next map in Doom's episodes, or `None` after the last one (ExM8). The secret
/// exit of ExM3 goes to ExM9, and ExM9 returns to ExM4.
fn next_map(current: &str, secret: bool) -> Option<String> {
    let &[b'E', episode, b'M', map] = current.as_bytes() else {
        return None;
    };
    let next = match (map, secret) {
        (b'3', true) => b'9',
        (b'9', _) => b'4',
        (b'8', _) => return None,
        (map @ b'1'..=b'7', _) => map + 1,
        _ => return None,
    };
    Some(format!("E{}M{}", episode as char, next as char))
}
