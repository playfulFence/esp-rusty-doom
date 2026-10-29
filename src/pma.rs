//! The second core's Physical Memory Attributes (PMA).
//!
//! The bootloader sets up the PMA only on the first core. They make PSRAM valid and
//! cacheable, so the second core faults on PSRAM until it gets the same entries.

use core::arch::asm;

const ENTRIES: usize = 16;

/// The PMA configuration and address registers of one core.
#[derive(Clone, Copy)]
pub struct Pma {
    config: [u32; ENTRIES],
    address: [u32; ENTRIES],
}

macro_rules! read_csrs {
    ($($csr:literal),*) => {
        [$({
            let value: u32;
            unsafe { asm!(concat!("csrr {0}, ", stringify!($csr)), out(reg) value) };
            value
        }),*]
    };
}

macro_rules! write_csr {
    ($index:expr, $value:expr, $($i:literal => $csr:literal),*) => {
        match $index {
            $($i => unsafe { asm!(concat!("csrw ", stringify!($csr), ", {0}"), in(reg) $value) },)*
            _ => unreachable!(),
        }
    };
}

fn write_config(index: usize, value: u32) {
    write_csr!(index, value,
        0 => 0xbc0, 1 => 0xbc1, 2 => 0xbc2, 3 => 0xbc3, 4 => 0xbc4, 5 => 0xbc5, 6 => 0xbc6,
        7 => 0xbc7, 8 => 0xbc8, 9 => 0xbc9, 10 => 0xbca, 11 => 0xbcb, 12 => 0xbcc,
        13 => 0xbcd, 14 => 0xbce, 15 => 0xbcf);
}

fn write_address(index: usize, value: u32) {
    write_csr!(index, value,
        0 => 0xbd0, 1 => 0xbd1, 2 => 0xbd2, 3 => 0xbd3, 4 => 0xbd4, 5 => 0xbd5, 6 => 0xbd6,
        7 => 0xbd7, 8 => 0xbd8, 9 => 0xbd9, 10 => 0xbda, 11 => 0xbdb, 12 => 0xbdc,
        13 => 0xbdd, 14 => 0xbde, 15 => 0xbdf);
}

impl Pma {
    /// Reads the entries of the core that runs this.
    pub fn read() -> Self {
        Self {
            config: read_csrs!(
                0xbc0, 0xbc1, 0xbc2, 0xbc3, 0xbc4, 0xbc5, 0xbc6, 0xbc7, 0xbc8, 0xbc9, 0xbca, 0xbcb,
                0xbcc, 0xbcd, 0xbce, 0xbcf
            ),
            address: read_csrs!(
                0xbd0, 0xbd1, 0xbd2, 0xbd3, 0xbd4, 0xbd5, 0xbd6, 0xbd7, 0xbd8, 0xbd9, 0xbda, 0xbdb,
                0xbdc, 0xbdd, 0xbde, 0xbdf
            ),
        }
    }

    /// Gives the core that runs this the same entries.
    pub fn apply(&self) {
        // Disabling from the top down keeps no half-written top-of-range entry active.
        for index in (0..ENTRIES).rev() {
            write_config(index, 0);
            write_address(index, 0);
        }
        for index in 0..ENTRIES {
            write_address(index, self.address[index]);
            write_config(index, self.config[index]);
        }
        unsafe { asm!("fence", "fence.i") };
    }
}
