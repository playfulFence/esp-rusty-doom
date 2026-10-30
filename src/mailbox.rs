//! Passes the touch input to the game on the second core, and the game's pictures
//! back to the first core, which shows them.

use alloc::{string::String, vec, vec::Vec};
use core::cell::RefCell;

use critical_section::Mutex;
use embassy_time::Duration;
use neurodoom::{SCREENHEIGHT, SCREENWIDTH};

use crate::{controls::Input, game::Status};

/// One rendered game frame.
pub struct Picture {
    /// One palette index per pixel.
    pub screen: Vec<u8>,
    pub palette: [u16; 256],
    pub status: Option<Status>,
    pub map: String,
    /// How long the game took to make this picture.
    pub game_time: Duration,
}

impl Picture {
    fn new() -> Self {
        Self {
            screen: vec![0; SCREENWIDTH * SCREENHEIGHT],
            palette: [0; 256],
            status: None,
            map: String::new(),
            game_time: Duration::from_ticks(0),
        }
    }
}

struct Mailbox {
    input: Input,
    /// The newest picture that the first core has not taken yet.
    picture: Option<Picture>,
    /// A picture that the first core is done with, for reuse.
    spare: Option<Picture>,
}

static MAILBOX: Mutex<RefCell<Mailbox>> = Mutex::new(RefCell::new(Mailbox {
    input: Input {
        forward: 0,
        side: 0,
        turn: 0,
        fire: false,
        use_: false,
        next_weapon: false,
    },
    picture: None,
    spare: None,
}));

/// Replaces the input, but keeps a weapon button press until the game sees it.
pub fn post_input(input: Input) {
    critical_section::with(|cs| {
        let mut mailbox = MAILBOX.borrow_ref_mut(cs);
        let next_weapon = mailbox.input.next_weapon || input.next_weapon;
        mailbox.input = Input {
            next_weapon,
            ..input
        };
    });
}

pub fn take_input() -> Input {
    critical_section::with(|cs| {
        let mut mailbox = MAILBOX.borrow_ref_mut(cs);
        let input = mailbox.input;
        mailbox.input.next_weapon = false;
        input
    })
}

/// Fills a picture and posts it, in place of an older one that was not taken.
pub fn post_picture(fill: impl FnOnce(&mut Picture)) {
    let mut picture = critical_section::with(|cs| MAILBOX.borrow_ref_mut(cs).spare.take())
        .unwrap_or_else(Picture::new);
    fill(&mut picture);
    critical_section::with(|cs| {
        let mut mailbox = MAILBOX.borrow_ref_mut(cs);
        if let Some(old) = mailbox.picture.replace(picture) {
            mailbox.spare = Some(old);
        }
    });
}

/// Takes the newest picture, if there is one.
pub fn take_picture() -> Option<Picture> {
    critical_section::with(|cs| MAILBOX.borrow_ref_mut(cs).picture.take())
}

/// Gives back a picture that the panel no longer shows, for reuse.
pub fn give_back(picture: Picture) {
    let unused = critical_section::with(|cs| MAILBOX.borrow_ref_mut(cs).spare.replace(picture));
    drop(unused);
}
