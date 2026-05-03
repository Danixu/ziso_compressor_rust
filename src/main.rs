mod args;
mod compressor;
mod decompressor;
mod types;
mod utils;

use args::Args;
use clap::Parser;
use compressor::compressor;
use decompressor::decompressor;
use log::{debug, info, trace};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::PathBuf;
use std::thread;
use types::LZ4_MAX_ACCELERATION;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize the logger
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(|buf, record| {
            let current_thread = thread::current();
            let thread_name = current_thread.name().unwrap_or("Main");
            let style = buf.default_level_style(record.level());

            writeln!(
                buf,
                "[{}] [{:<10}] [{}{:<5}{:#}] {}",
                buf.timestamp_millis(),
                thread_name,
                style,
                record.level(),
                style,
                record.args()
            )
        })
        .init();

    info!("Starting the ZSO compressor/decompressor");

    let args = Args::parse();
    run(args)
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    debug!("Getting the program arguments");

    // Validate and open input file
    let input_filename = args.input.clone();
    debug!("Input file: {:?}", input_filename);

    let mut input_file = open_input_file(&input_filename)?;

    // Determine if file is compressed by checking header
    let compressed = is_compressed_file(&mut input_file)?;
    debug!("Compressed?: {}", compressed);

    // Generate output filename if not provided
    let output_filename = args
        .output
        .clone()
        .unwrap_or_else(|| input_filename.with_extension(if compressed { "iso" } else { "zso" }));
    debug!("Output file: {:?}", output_filename);

    // Validate output file
    validate_output_file(&output_filename, args.force)?;

    // Open output file
    let output_file = open_output_file(&output_filename)?;

    // Print operation info
    print_operation_info(&args, &input_filename, &output_filename, compressed);

    // Execute compression or decompression
    if compressed {
        decompressor(args, input_file, output_file)?;
    } else {
        compressor(args, input_file, output_file)?;
    }

    Ok(())
}

fn open_input_file(filename: &PathBuf) -> Result<File, Box<dyn std::error::Error>> {
    if !filename.exists() || !filename.is_file() {
        return Err(format!(
            "The input file {:?} doesn't exist or is not valid",
            filename
        )
        .into());
    }

    debug!("Opening the input file as File");
    File::open(filename).map_err(|e| format!("Cannot open input file {:?}: {}", filename, e).into())
}

fn is_compressed_file(file: &mut File) -> Result<bool, Box<dyn std::error::Error>> {
    debug!("Checking if the input file is an already compressed ZSO");

    let mut input_header: [u8; 4] = [0; 4];
    file.read_exact(&mut input_header)
        .map_err(|e| format!("Cannot read the input file header: {}", e))?;

    trace!("Input file header: {:?}", &input_header);

    let compressed = &input_header == b"ZISO";

    // Rewind file to start
    trace!("Rewind the file to the start point");
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|e| format!("Cannot rewind the input file: {}", e))?;

    Ok(compressed)
}

fn validate_output_file(filename: &PathBuf, force: bool) -> Result<(), Box<dyn std::error::Error>> {
    debug!("Checking if the output file exists");

    if filename.exists() && filename.is_file() && !force {
        return Err(format!(
            "The output file {:?} exists and no force flag was provided",
            filename
        )
        .into());
    }

    Ok(())
}

fn open_output_file(filename: &PathBuf) -> Result<File, Box<dyn std::error::Error>> {
    debug!("Opening the output file as File in write mode and truncate");

    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(filename)
        .map_err(|e| format!("Cannot open output file {:?}: {}", filename, e).into())
}

fn print_operation_info(args: &Args, input: &PathBuf, output: &PathBuf, compressed: bool) {
    info!("Source: {:?}", input);
    info!("Destination: {:?}", output);
    info!("Force overwrite: {}", args.force);
    info!("Block size: {} bytes", args.block_size);
    info!("Number of threads: {}", args.threads);
    info!("HDL fix: {}", args.hdl_fix);

    if !compressed {
        info!("Compression level: {}", args.level);
        if args.disable_hc {
            info!("LZ4 HC compression: Disabled");
            info!(
                "LZ4 acceleration: {}",
                (LZ4_MAX_ACCELERATION - (args.level - 1) * LZ4_MAX_ACCELERATION / 11) as i32
            );
        } else {
            info!("LZ4 HC compression: Enabled");
            info!("LZ4 compression: {}", args.level);
        }
    }
}
