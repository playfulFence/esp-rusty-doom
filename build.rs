//! Compresses the WAD for the firmware. The 4 MB shareware WAD does not fit in the
//! 4 MB of flash that the chip maps for code and constants, but it fits compressed.

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=DOOM_WAD");
    let wad = env::var("DOOM_WAD").expect("set DOOM_WAD to the path of the WAD file");
    println!("cargo:rerun-if-changed={wad}");

    let data = fs::read(&wad).unwrap_or_else(|error| panic!("cannot read {wad}: {error}"));
    let compressed = miniz_oxide::deflate::compress_to_vec(&data, 10);

    // The decompressed size, then the raw DEFLATE stream.
    let mut output = (data.len() as u32).to_le_bytes().to_vec();
    output.extend_from_slice(&compressed);
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("wad.deflate");
    fs::write(out, output).unwrap();
}
