use super::args::Args;
use log::{debug, error, info, trace};
use std::fs::File;

pub fn decompressor(args: Args, mut input_file: File, mut output_file: File) -> Result<(), String> {
    info!("The input file is a ZSO file. Decompressing...");

    Ok(())
}
