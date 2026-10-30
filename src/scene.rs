//! What the panel shows, and how it gets from the main loop to the scanout
//! interrupt.
//!
//! The main loop prepares a [`Scene`] and posts it. At the start of each frame, the
//! interrupt takes the newest posted scene, and hands the one that it showed before
//! back to the main loop for reuse.

use alloc::{boxed::Box, string::String};
use core::{
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};

use esp_hal::ram;
use neurodoom::SCREENWIDTH;

use crate::{
    canvas::{Mask, Shape, VIEW_HEIGHT, VIEW_Y, draw_view_row, rgb565},
    display::WIDTH,
    mailbox::Picture,
};

const HUD_COLOR: u16 = rgb565(255, 200, 0);

/// Text that never changes: the button labels and the messages.
pub struct Overlays {
    pub labels: [Mask; 3],
    pub loading: Mask,
    pub died: Mask,
}

pub struct Scene {
    pub picture: Option<Picture>,
    /// The controls. Their labels show with them.
    pub shapes: Option<[Shape; 10]>,
    /// The line above the game view, and its text.
    pub hud: Mask,
    pub hud_text: String,
    pub overlays: &'static Overlays,
    pub message: Option<&'static Mask>,
}

impl Scene {
    /// A black scene.
    pub fn new(overlays: &'static Overlays) -> Self {
        Self {
            picture: None,
            shapes: None,
            hud: Mask::new(0, 0, WIDTH as i32, VIEW_Y as i32, HUD_COLOR),
            hud_text: String::new(),
            overlays,
            message: None,
        }
    }

    fn draw_overlays(&self, y: i32, row: &mut [u16]) {
        if let Some(shapes) = &self.shapes {
            for shape in shapes {
                shape.draw_row(y, row);
            }
            for label in &self.overlays.labels {
                label.draw_row(y, row);
            }
        }
        self.hud.draw_row(y, row);
        if let Some(message) = self.message {
            message.draw_row(y, row);
        }
    }
}

/// The newest scene from the main loop.
static NEXT: AtomicPtr<Scene> = AtomicPtr::new(ptr::null_mut());
/// The scene that the interrupt is showing. Only the interrupt uses it.
static CURRENT: AtomicPtr<Scene> = AtomicPtr::new(ptr::null_mut());
/// The scene that the interrupt showed before, for the main loop to reuse.
static DONE: AtomicPtr<Scene> = AtomicPtr::new(ptr::null_mut());

/// Shows the scene from the next frame on. Returns an older scene that was posted but
/// never shown.
///
/// Call [`take_done`] before each post, so that the interrupt always has room to give
/// a scene back.
pub fn post(scene: Box<Scene>) -> Option<Box<Scene>> {
    let old = NEXT.swap(Box::into_raw(scene), Ordering::AcqRel);
    // SAFETY: only `Box::into_raw` puts non-null pointers into the slots.
    (!old.is_null()).then(|| unsafe { Box::from_raw(old) })
}

/// Takes the scene that the panel stopped showing, if any.
pub fn take_done() -> Option<Box<Scene>> {
    let done = DONE.swap(ptr::null_mut(), Ordering::AcqRel);
    // SAFETY: as in `post`.
    (!done.is_null()).then(|| unsafe { Box::from_raw(done) })
}

/// Draws panel rows from `first_row` on into `rows`, row by row. Called by the
/// scanout interrupt.
#[ram]
pub fn draw_rows(first_row: usize, rows: &mut [u16]) {
    if first_row == 0 {
        let next = NEXT.swap(ptr::null_mut(), Ordering::AcqRel);
        if !next.is_null() {
            let shown = CURRENT.swap(next, Ordering::AcqRel);
            DONE.store(shown, Ordering::Release);
        }
    }

    // SAFETY: the main loop does not touch the current scene, and only this
    // interrupt replaces it.
    let Some(scene) = (unsafe { CURRENT.load(Ordering::Acquire).as_ref() }) else {
        rows.fill(0);
        return;
    };

    let view_rows = VIEW_Y..VIEW_Y + VIEW_HEIGHT;
    let pairs = rows.as_chunks_mut::<{ WIDTH * 2 }>().0;
    for (i, pair) in pairs.iter_mut().enumerate() {
        let y = first_row + 2 * i;
        let (upper, lower) = pair.split_at_mut(WIDTH);
        // Both rows of a pair show the same game row, so the second one is a copy.
        match &scene.picture {
            Some(picture) if view_rows.contains(&y) => {
                let start = (y - VIEW_Y) / 2 * SCREENWIDTH;
                let screen_row = &picture.screen[start..start + SCREENWIDTH];
                draw_view_row(screen_row, &picture.palette, upper);
            }
            _ => upper.fill(0),
        }
        lower.copy_from_slice(upper);
        scene.draw_overlays(y as i32, upper);
        scene.draw_overlays(y as i32 + 1, lower);
    }
}
