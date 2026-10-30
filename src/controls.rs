//! On-screen controls like in mobile shooters: a floating stick on the left half
//! moves and strafes, a stick on the right half turns, and buttons fire, use and
//! switch weapons.

use crate::{
    canvas::{Mask, Paint, Shape, VIEW_Y, isqrt, rgb565},
    display::{HEIGHT, WIDTH},
    touch::Touches,
};

/// How far the knob moves from the stick's center.
const STICK_RADIUS: i32 = 75;
const KNOB_RADIUS: i32 = 32;
/// Movement smaller than this many pixels counts as none.
const DEAD_ZONE: i32 = 8;

/// Doom's running speeds.
const MAX_FORWARD: i32 = 50;
const MAX_SIDE: i32 = 40;
/// Doom's fast turn, in 1/65536 of a full circle per tic.
const MAX_TURN: i32 = 1280;

/// Extra pixels around a button that still count as a hit.
const BUTTON_SLACK: i32 = 12;

const WHITE: u16 = rgb565(255, 255, 255);
const GREY: u16 = rgb565(150, 150, 150);

/// What the controls ask the player to do.
#[derive(Clone, Copy, Default)]
pub struct Input {
    pub forward: i8,
    pub side: i8,
    pub turn: i16,
    pub fire: bool,
    pub use_: bool,
    /// The weapon button went down since the last call to [`Controls::input`].
    pub next_weapon: bool,
}

struct Stick {
    home: (i32, i32),
    center: (i32, i32),
    knob: (i32, i32),
    finger: Option<u8>,
}

impl Stick {
    const fn new(x: i32, y: i32) -> Self {
        Self {
            home: (x, y),
            center: (x, y),
            knob: (x, y),
            finger: None,
        }
    }

    /// Puts the stick under the finger, fully on the screen and below the HUD.
    fn grab(&mut self, finger: u8, x: i32, y: i32) {
        let margin = STICK_RADIUS + KNOB_RADIUS;
        self.center = (
            x.clamp(margin, WIDTH as i32 - margin),
            y.clamp(VIEW_Y as i32 + margin, HEIGHT as i32 - margin),
        );
        self.finger = Some(finger);
        self.drag(x, y);
    }

    fn drag(&mut self, x: i32, y: i32) {
        let (dx, dy) = (x - self.center.0, y - self.center.1);
        let distance = isqrt(dx * dx + dy * dy);
        self.knob = if distance > STICK_RADIUS {
            (
                self.center.0 + dx * STICK_RADIUS / distance,
                self.center.1 + dy * STICK_RADIUS / distance,
            )
        } else {
            (x, y)
        };
    }

    fn release(&mut self) {
        *self = Self::new(self.home.0, self.home.1);
    }

    /// The knob's offset from the center, from -1.0 to 1.0 on each axis.
    fn offset(&self) -> (f32, f32) {
        let axis = |delta: i32| {
            if delta.abs() < DEAD_ZONE {
                0.0
            } else {
                (delta - DEAD_ZONE * delta.signum()) as f32 / (STICK_RADIUS - DEAD_ZONE) as f32
            }
        };
        (
            axis(self.knob.0 - self.center.0),
            axis(self.knob.1 - self.center.1),
        )
    }

    fn shapes(&self) -> [Shape; 2] {
        let knob = if self.finger.is_some() { WHITE } else { GREY };
        [
            Shape {
                x: self.center.0,
                y: self.center.1,
                outer: STICK_RADIUS,
                inner: STICK_RADIUS - 4,
                paint: Paint::Glass(GREY),
            },
            Shape {
                x: self.knob.0,
                y: self.knob.1,
                outer: KNOB_RADIUS,
                inner: 0,
                paint: Paint::Glass(knob),
            },
        ]
    }
}

struct Button {
    center: (i32, i32),
    radius: i32,
    color: u16,
    label: &'static str,
    finger: Option<u8>,
}

impl Button {
    const fn new(x: i32, y: i32, radius: i32, color: u16, label: &'static str) -> Self {
        Self {
            center: (x, y),
            radius,
            color,
            label,
            finger: None,
        }
    }

    fn hit(&self, x: i32, y: i32) -> bool {
        let (dx, dy) = (x - self.center.0, y - self.center.1);
        let reach = self.radius + BUTTON_SLACK;
        dx * dx + dy * dy <= reach * reach
    }

    fn shapes(&self) -> [Shape; 2] {
        let (x, y) = self.center;
        let fill = if self.finger.is_some() {
            Paint::Solid(self.color)
        } else {
            Paint::Glass(self.color)
        };
        [
            Shape {
                x,
                y,
                outer: self.radius,
                inner: 0,
                paint: fill,
            },
            Shape {
                x,
                y,
                outer: self.radius,
                inner: self.radius - 3,
                paint: Paint::Glass(WHITE),
            },
        ]
    }

    fn label(&self) -> Mask {
        Mask::text(self.label, self.center.0, self.center.1, WHITE)
    }
}

pub struct Controls {
    walk: Stick,
    look: Stick,
    fire: Button,
    use_: Button,
    weapon: Button,
    weapon_pressed: bool,
}

impl Controls {
    pub const fn new() -> Self {
        Self {
            walk: Stick::new(150, 360),
            look: Stick::new(650, 360),
            fire: Button::new(705, 200, 55, rgb565(200, 30, 30), "FIRE"),
            use_: Button::new(575, 205, 42, rgb565(40, 140, 60), "USE"),
            weapon: Button::new(90, 95, 42, rgb565(40, 80, 180), "WPN"),
            weapon_pressed: false,
        }
    }

    /// Follows the fingers on the panel.
    pub fn update(&mut self, touches: &Touches) {
        for stick in [&mut self.walk, &mut self.look] {
            match stick.finger.map(|id| touches.find(id)) {
                Some(Some(point)) => stick.drag(point.x, point.y),
                Some(None) => stick.release(),
                None => {}
            }
        }
        for button in [&mut self.fire, &mut self.use_, &mut self.weapon] {
            if button.finger.is_some_and(|id| touches.find(id).is_none()) {
                button.finger = None;
            }
        }

        for point in touches.iter() {
            let id = Some(point.id);
            let owned = [
                self.walk.finger,
                self.look.finger,
                self.fire.finger,
                self.use_.finger,
                self.weapon.finger,
            ]
            .contains(&id);
            if owned {
                continue;
            }

            let (x, y) = (point.x, point.y);
            let buttons = [&mut self.fire, &mut self.use_, &mut self.weapon];
            if let Some(index) = buttons.iter().position(|button| button.hit(x, y)) {
                if buttons[index].finger.is_none() {
                    buttons[index].finger = id;
                    self.weapon_pressed |= index == 2;
                }
            } else if x < WIDTH as i32 / 2 {
                if self.walk.finger.is_none() {
                    self.walk.grab(point.id, x, y);
                }
            } else if self.look.finger.is_none() {
                self.look.grab(point.id, x, y);
            }
        }
    }

    pub fn input(&mut self) -> Input {
        let (side, forward) = self.walk.offset();
        let (turn, _) = self.look.offset();
        // Small deflections turn slowly for aiming, large ones turn fast.
        let turn = turn * (0.35 + 0.65 * turn.abs());
        Input {
            forward: (-forward * MAX_FORWARD as f32) as i8,
            side: (side * MAX_SIDE as f32) as i8,
            turn: (-turn * MAX_TURN as f32) as i16,
            fire: self.fire.finger.is_some(),
            use_: self.use_.finger.is_some(),
            next_weapon: core::mem::take(&mut self.weapon_pressed),
        }
    }

    /// The sticks and the buttons, without their labels.
    pub fn shapes(&self) -> [Shape; 10] {
        let [a, b] = self.walk.shapes();
        let [c, d] = self.look.shapes();
        let [e, f] = self.fire.shapes();
        let [g, h] = self.use_.shapes();
        let [i, j] = self.weapon.shapes();
        [a, b, c, d, e, f, g, h, i, j]
    }

    /// The buttons' labels. They never move.
    pub fn labels(&self) -> [Mask; 3] {
        [self.fire.label(), self.use_.label(), self.weapon.label()]
    }
}
