use clap::Parser;
use log::{debug, error, info, trace, warn};
use std::cmp;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

// Constants
const QUEUE_SIZE: usize = 16;

// Message block used to exchange data between threads
struct MessageBlock {
    id: usize,
    compressed: bool,
    data: Vec<u8>,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Input file. Example: game.iso
    input: PathBuf,

    /// Output file. Example: game.cso (optional)
    output: Option<PathBuf>,

    /// Force to overwrite the output file if exists
    #[arg(short, long, default_value_t = false)]
    force: bool,

    /// Threads number used to compress, by default the CPU cores.
    #[arg(short = 't', long, default_value_t = default_threads())]
    threads: usize,

    /// LZ4 compression level (1-12)
    #[arg(short = 'l', long, default_value_t = 12, value_parser = clap::value_parser!(u8).range(1..=12))]
    level: u8,

    /// Disable the LZ4HC compression
    #[arg(long = "nohc", default_value_t = false)]
    disable_hc: bool,

    /// The size of every block in the CSO file (2048-131072)(recommended 2048 for HDL).
    #[arg(long, default_value_t = 2048, value_parser = clap::value_parser!(u32).range(2048..=131072))]
    block_size: u32,

    /// HDL Fix to avoid a bug in the hdl_dump
    #[arg(long, default_value_t = false)]
    hdl_fix: bool,
}

// Simple function to return the number of available cores in the system
fn default_threads() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

fn main() {
    // Initialize the logger
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    // Get the arguments
    trace!("Getting the program arguments");
    let args = Args::parse();
    debug!("Force: {}", args.force);
    debug!("Threads: {}", args.threads);
    debug!("Compression level: {}", args.level);
    debug!("Disable LZ4HC: {}", args.disable_hc);
    debug!("Block size: {}", args.block_size);
    debug!("HDL Fix: {}", args.hdl_fix);

    // Check the input file existence
    trace!("Checking if the input file exists.");
    let input_filename = args.input.clone();
    debug!("Input file: {:?}", input_filename);
    if !input_filename.exists() || !input_filename.is_file() {
        error!(
            "The input file {:?} doesn't exists or is not valid.",
            input_filename
        );
        std::process::exit(1);
    }
    trace!("Opening the input file as File.");
    let mut input_file = File::open(&input_filename).unwrap_or_else(|e| {
        error!(
            "Fatal error: The input file '{:?}' cannot be readed: {}",
            input_filename, e
        );
        std::process::exit(1)
    });

    trace!("Checking if the input file is an already compressed CSO");
    let compressed: bool = {
        let mut input_header: [u8; 4] = [0; 4];

        input_file
            .read_exact(&mut input_header)
            .unwrap_or_else(|e| {
                error!("Fatal error: Cannot read the input file header: {}", e);
                std::process::exit(1)
            });

        &input_header == b"CISO"
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
    trace!("Checking if the output file exists");
    let output_filename = args
        .output
        .clone()
        .unwrap_or_else(|| input_filename.with_extension(if compressed { "iso" } else { "cso" }));
    debug!("Output file: {:?}", output_filename);
    // Check the output file existence and if must be overwritten
    if (output_filename.exists() && output_filename.is_file()) && !args.force {
        error!(
            "The output file {:?} exists and no force flag was provided.",
            output_filename
        );
        std::process::exit(1);
    }
    trace!("Opening the output file as File in write mode and truncate");
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
        let _ = decompressor(&args, input_file, output_file);
    } else {
        let _ = compressor(&args, input_file, output_file);
    }
    ()
}

fn compressor(args: &Args, mut input_file: File, mut output_file: File) -> Result<bool, String> {
    info!("The input file is a image file. Compressing...");

    trace!("Gettint input metadata");
    let input_metadata = input_file.metadata().unwrap_or_else(|e| {
        error!("Error reading the input file metadata: {}", e);
        std::process::exit(1)
    });

    // Calculate the number of blocks in the file
    let total_blocks: usize =
        (input_metadata.len() as usize + (args.block_size - 1) as usize) / args.block_size as usize;
    debug!("Number of blocks: {:?}", total_blocks);
    debug!("Last block size: {:?}", {
        if (input_metadata.len() % args.block_size as u64) == 0 {
            args.block_size as u64
        } else {
            input_metadata.len() % args.block_size as u64
        }
    });

    // Index table
    let mut index_table: Vec<u32> = vec![0; total_blocks + 1];

    let pos_shift: u8 = {
        match input_metadata.len() {
            0_u64..0x7FFFFFFF_u64 => 0,
            0x7FFFFFFF_u64..0xFFFFFFFF_u64 => 1,
            0xFFFFFFFF_u64..0x1FFFFFFFF_u64 => 2,
            0x1FFFFFFFF_u64..0x3FFFFFFFF_u64 => 3,
            0x3FFFFFFFF_u64..=u64::MAX => 4,
        }
    };

    // Write the CSO header
    let mut header = [0u8; 24];
    header[0..4].copy_from_slice(b"CISO"); // Magic string
    header[4..8].copy_from_slice(&24u32.to_le_bytes()); // Header size (always 24)
    header[8..16].copy_from_slice(&input_metadata.len().to_le_bytes()); // Original size without compress
    header[16..20].copy_from_slice(&args.block_size.to_le_bytes()); // Block size
    header[20] = 1; // Format version (v1)
    header[21] = pos_shift; // Index alignment
    // The 22th and 23th bytes are reserved

    // Write the header
    let _ = output_file.write_all(&header);

    // Write the empty index to reserve the space
    let _ = output_file
        .seek(SeekFrom::Current(((total_blocks + 1) * 4) as i64))
        .unwrap();

    // A way to stop all the threads if any of them has failed
    let kill_switch = Arc::new(AtomicBool::new(false));

    // Using sync_channel to limit the number of items
    // Si hay 100 bloques en la cola, el que intente enviar se pausará.
    let (tx_in, rx_in) = mpsc::sync_channel::<MessageBlock>(QUEUE_SIZE);
    let (tx_out, rx_out) = mpsc::sync_channel::<MessageBlock>(QUEUE_SIZE);

    // Protect the rx_in with a mutex to allow multiple workers to read from it.
    // Arc (Atomic Reference Counting) allows to share it between threads securely.
    let shared_rx_in = Arc::new(Mutex::new(rx_in));

    // Initialize the workers (compression threads)
    let mut workers = vec![];
    for _ in 0..args.threads {
        let rx = Arc::clone(&shared_rx_in);
        let tx = tx_out.clone();
        // Clone the kill_switch
        let worker_kill_switch = kill_switch.clone();

        let worker = thread::spawn(move || {
            loop {
                // If the kill_switch was activated, break the loop
                if worker_kill_switch.load(Ordering::Relaxed) {
                    break;
                }

                // Get the lock, pull a block and drop the lock immediately.
                let block_opt = {
                    let lock = rx.lock().unwrap();
                    lock.recv().ok() // ok() returns None if the queue was closed (EOF).
                };

                match block_opt {
                    Some(block) => {
                        // AQUÍ VA TU LÓGICA DE LZ4:
                        // let compressed = lz4::compress(&block.data);
                        //println!(
                        //    "Processing the block {} with a size of {}",
                        //    block.id,
                        //    block.data.len()
                        //);
                        let compressed_data = block.data;

                        // Send the compressed data to the output queue to be processed by the writer.
                        tx.send(MessageBlock {
                            id: block.id,
                            compressed: false,
                            data: compressed_data,
                        })
                        .unwrap();
                    }
                    None => break, // El canal se cerró, el worker termina su trabajo
                }
            }
        });
        workers.push(worker);
    }

    // Drop the output queue. This will allow the writer to detect when there's no more data.
    drop(tx_out);

    // Clone the kill_switch
    let reader_kill_switch = Arc::clone(&kill_switch);
    // Initialize the reader thread
    let reader_thread = thread::spawn(move || {
        let mut file = File::open("test.bin").expect("There was an error reading the file");
        let file_size = file
            .metadata()
            .expect("There was an error getting the file info")
            .len();
        println!("Tamaño total del archivo: {} bytes", file_size);
        let mut read_bytes_left = file_size;
        let mut id = 0;

        while read_bytes_left > 0 {
            // If the kill_switch was activated, break the loop
            if reader_kill_switch.load(Ordering::Relaxed) {
                break;
            }

            let to_read: usize = cmp::min(read_bytes_left as usize, 2048000);
            let mut data = vec![0; to_read];
            let compressed: bool = false;

            //println!("Reading the {} block with a size of {}", id, to_read);
            let _ = file.read_exact(&mut data);

            // Si la cola está llena (100 elementos), send() pausará este hilo automáticamente
            tx_in
                .send(MessageBlock {
                    id,
                    compressed,
                    data,
                })
                .unwrap();

            id += 1;
            read_bytes_left = file_size
                - file
                    .stream_position()
                    .expect("There was an erorr getting the stream pos");
        }
    });

    // Clone the kill_switch
    let reader_kill_switch = Arc::clone(&kill_switch);
    // Initialize the writter
    let writer_thread = thread::spawn(move || {
        let mut expected_id = 0;
        // Temporal buffer for the block ordering
        let mut out_of_order_buffer: HashMap<usize, Vec<u8>> = HashMap::new();

        let mut file =
            File::create("test.out").expect("There was an error opening the output file");

        // The loop works until all the workers tx_out are closed, pausing when there is no data.
        for block in rx_out {
            // If the kill_switch was activated, break the loop
            if reader_kill_switch.load(Ordering::Relaxed) {
                break;
            }

            // Check if the received block is the expected one
            if block.id == expected_id {
                // If the order matches then write it to the file
                let _ = file.write_all(&block.data);
                expected_id += 1;

                // Check if the next blocks are in the ordering buffer
                while let Some(buffered_data) = out_of_order_buffer.remove(&expected_id) {
                    // If the next block was waiting in the buffer, then write it into the file
                    let _ = file.write_all(&buffered_data);
                    expected_id += 1;
                }
            } else {
                // If the block doesn't matches the expected block then store it in the ordering buffer.
                out_of_order_buffer.insert(block.id, block.data);
            }
        }
    });

    // Wait for every thread and then finalize the program
    reader_thread.join().unwrap();
    for w in workers {
        w.join().unwrap();
    }
    writer_thread.join().unwrap();

    info!("File compressed succesfully!");

    Ok(true)
}

fn decompressor(args: &Args, mut input_file: File, mut output_file: File) -> Result<bool, String> {
    info!("The input file is a CSO file. Decompressing...");

    Ok(true)
}
