mod args;
mod compressor;
mod decompressor;
mod types;
mod utils;

use args::Args;
use clap::Parser;
use compressor::compressor;
use decompressor::decompressor;
use log::{debug, error, info, trace};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::thread;

fn main() {
    // Initialize the logger
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(|buf, record| {
            // Obtenemos el hilo actual que está emitiendo el log
            let current_thread = thread::current();
            // Intentamos sacar el nombre. Si no tiene (como el main), ponemos "Main"
            let thread_name = current_thread.name().unwrap_or("Main");
            // 1. Extraemos el nivel de log con sus colores ANSI originales
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

    // Get the arguments
    debug!("Getting the program arguments");
    let args = Args::parse();
    debug!("Force: {}", args.force);
    debug!("Threads: {}", args.threads);
    debug!("Compression level: {}", args.level);
    debug!("Disable LZ4HC: {}", args.disable_hc);
    debug!("Block size: {}", args.block_size);
    debug!("HDL Fix: {}", args.hdl_fix);

    info!("Starting the ZSO compressor/decompressor");

    // Check the input file existence
    debug!("Checking if the input file exists");
    let input_filename = args.input.clone();
    debug!("Input file: {:?}", input_filename);
    if !input_filename.exists() || !input_filename.is_file() {
        error!(
            "The input file {:?} doesn't exists or is not valid",
            input_filename
        );
        std::process::exit(1);
    }
    debug!("Opening the input file as File");
    let mut input_file = File::open(&input_filename).unwrap_or_else(|e| {
        error!(
            "Fatal error: The input file '{:?}' cannot be readed: {}",
            input_filename, e
        );
        std::process::exit(1)
    });

    debug!("Checking if the input file is an already compressed ZSO");
    let compressed: bool = {
        let mut input_header: [u8; 4] = [0; 4];

        input_file
            .read_exact(&mut input_header)
            .unwrap_or_else(|e| {
                error!("Fatal error: Cannot read the input file header: {}", e);
                std::process::exit(1)
            });

        trace!("Input file header: {:?}", &input_header);

        &input_header == b"ZISO"
    };
    debug!("Compressed?: {}", compressed);

    trace!("Rewind the file to the start point");
    let _ = input_file
        .seek(std::io::SeekFrom::Start(0))
        .unwrap_or_else(|e| {
            error!("Fatal error: Cannot rewind the input file: {}", e);
            std::process::exit(1)
        });

    // If the output is empty, then generate the filename based in the input
    debug!("Checking if the output file exists");
    let output_filename = args
        .output
        .clone()
        .unwrap_or_else(|| input_filename.with_extension(if compressed { "iso" } else { "zso" }));
    debug!("Output file: {:?}", output_filename);
    // Check the output file existence and if must be overwritten
    if (output_filename.exists() && output_filename.is_file()) && !args.force {
        error!(
            "The output file {:?} exists and no force flag was provided",
            output_filename
        );
        std::process::exit(1);
    }
    debug!("Opening the output file as File in write mode and truncate");
    let output_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&output_filename)
        .unwrap_or_else(|e| {
            error!(
                "Fatal error: Output file '{:?}' cannot be written: {}",
                output_filename, e
            );
            std::process::exit(1)
        });
    if compressed {
        if let Err(e) = decompressor(args, input_file, output_file) {
            error!("Decompression failed: {}", e);
            std::process::exit(1);
        }
    } else {
        if let Err(e) = compressor(args, input_file, output_file) {
            error!("Compression failed: {}", e);
            std::process::exit(1);
        }
    }
    ()
}
