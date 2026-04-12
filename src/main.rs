use clap::Parser;
use log::{debug, error, info, trace};
use lzzzz::{lz4, lz4_hc};
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
const QUEUE_TRANSFER_SIZE: usize = 2 * 1024 * 1024;

// Message block used to exchange data between threads
struct MessageBlock {
    id: usize,
    compressed: Vec<bool>,
    blocksize: Vec<u32>,
    data: Vec<u8>,
}

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Input file. Example: game.iso
    input: PathBuf,

    /// Output file. Example: game.zso (optional)
    output: Option<PathBuf>,

    /// Force to overwrite the output file if exists
    #[arg(short, long, default_value_t = false)]
    force: bool,

    /// Threads number used to compress, by default the CPU cores.
    #[arg(short = 't', long, default_value_t = default_threads())]
    threads: usize,

    /// LZ4 compression level (1-12)
    #[arg(short = 'l', long, default_value_t = 12, value_parser = clap::value_parser!(i32).range(1..=12))]
    level: i32,

    /// Disable the LZ4HC compression
    #[arg(long = "nohc", default_value_t = false)]
    disable_hc: bool,

    /// The size of every block in the ZSO file (2048-131072)(recommended 2048 for HDL).
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
        let _ = decompressor(args, input_file, output_file);
    } else {
        let _ = compressor(args, input_file, output_file);
    }
    ()
}

fn compressor(args: Args, mut input_file: File, mut output_file: File) -> Result<bool, String> {
    info!("The input file is a image file. Compressing...");

    debug!("Gettint input metadata");
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
    trace!("Generating the index data");
    let mut index_table: Vec<u32> = vec![0; total_blocks + 1];

    // The position shift required to store the block position for bigger files
    let pos_shift: u8 = {
        match input_metadata.len() {
            0_u64..0x7FFFFFFF_u64 => 0, // PSP games, CD-ROM games and some small DVD games (below 2GB)
            0x7FFFFFFF_u64..0xFFFFFFFF_u64 => 1, // Most of the rest of the DVD games (below 4GB)
            0xFFFFFFFF_u64..0x1FFFFFFFF_u64 => 2, // Almost all the double layer DVD games (below 8GB)
            0x1FFFFFFFF_u64..0x3FFFFFFFF_u64 => 3, // Rare double layer DVD games (between 8GB and 16GB)
            0x3FFFFFFFF_u64..=u64::MAX => 4, // Above 16GB. There are no CD-ROM or DVD-ROM with this size.
        }
    };
    debug!("The index shifting will be {} for this iso", pos_shift);

    // Data block alignement variables
    let alignment = 1usize << pos_shift;
    debug!("The aligment will be {}", alignment);

    // Write the ZSO header
    debug!("Generating the ZSO header");
    let mut header = [0u8; 24];
    header[0..4].copy_from_slice(b"ZISO"); // Magic string
    header[4..8].copy_from_slice(&24u32.to_le_bytes()); // Header size (always 24)
    header[8..16].copy_from_slice(&input_metadata.len().to_le_bytes()); // Original size without compress
    header[16..20].copy_from_slice(&args.block_size.to_le_bytes()); // Block size
    header[20] = 1; // Format version (v1)
    header[21] = pos_shift; // Index alignment
    // The 22th and 23th bytes are reserved
    trace!("ZSO Header {:?}", &header);

    // Write the header
    debug!("Writting the ZSO header into the output file");
    let _ = output_file.write_all(&header);

    // Write the empty index to reserve the space
    debug!("Reserving the index data space in the output file");
    let _ = output_file
        .seek(SeekFrom::Current(((total_blocks + 1) * 4) as i64))
        .unwrap();

    // A way to stop all the threads if any of them has failed
    let kill_switch = Arc::new(AtomicBool::new(false));

    // We get the real number of bytes to read. In case that the block_size is modified, it can differ from the QUEUE_TRANSFER_SIZE.
    // 1MB / 2KB = 512, but 1MB / 3KB = 341,33333... and partial blocks must not be read.
    let queue_real_size: usize =
        QUEUE_TRANSFER_SIZE - (QUEUE_TRANSFER_SIZE % args.block_size as usize);

    // Using sync_channel to limit the number of items
    // Threads will wait until there's space on the queue.
    let (tx_in, rx_in) = mpsc::sync_channel::<MessageBlock>(QUEUE_SIZE);
    let (tx_out, rx_out) = mpsc::sync_channel::<MessageBlock>(QUEUE_SIZE);

    // Protect the rx_in with a mutex to allow multiple workers to read from it.
    // Arc (Atomic Reference Counting) allows to share it between threads securely.
    let shared_rx_in = Arc::new(Mutex::new(rx_in));

    // Initialize the workers (compression threads)
    debug!("Initializing the workers");
    let mut workers = vec![];
    for i in 0..args.threads {
        let rx = Arc::clone(&shared_rx_in);
        let tx = tx_out.clone();
        // Clone the kill_switch
        let worker_kill_switch = kill_switch.clone();

        debug!("Spawning the new thread");
        let worker: Result<thread::JoinHandle<()>, std::io::Error> = thread::Builder::new()
            .name(format!("Worker-{}", i))
            .spawn(move || {
                let mut comp_buffer = vec![0u8; lz4::max_compressed_size(args.block_size as usize)];
                let lz4_acceleration = lz4::ACC_LEVEL_DEFAULT;

                loop {
                    // If the kill_switch was activated, break the loop
                    if worker_kill_switch.load(Ordering::Relaxed) {
                        debug!("Kill switch activated. Breaking the loop.");
                        break;
                    }

                    // Get the lock, pull a block and drop the lock immediately.
                    debug!("Getting a new rx message");
                    let message_opt = {
                        trace!("Getting the rx lock");
                        let lock = rx.lock().unwrap();
                        trace!("Pulling the rx queue message");
                        lock.recv().ok() // ok() returns None if the queue was closed (EOF).
                    };

                    match message_opt {
                        Some(message) => {
                            debug!("Received a message. Processing...");

                            // Determine the number of received blocks
                            let blocks_number: usize = (message.data.len()
                                + (args.block_size as usize - 1))
                                / args.block_size as usize;

                            // Processing the data block by block
                            debug!("Processing {} blocks", blocks_number);
                            let mut out_message = MessageBlock {
                                id: message.id,
                                compressed: vec![false; blocks_number],
                                blocksize: vec![0; blocks_number],
                                data: Vec::new(),
                            };
                            for i in 0..blocks_number {
                                trace!("Working on block {}", i);
                                // Reference the block
                                let raw_block: &[u8] = &message.data[(i * args.block_size as usize)
                                    ..((i + 1) * args.block_size as usize)];

                                // Compress the block
                                let res = if args.disable_hc {
                                    trace!("Compressing using LZ4");
                                    lz4::compress(raw_block, &mut comp_buffer, lz4_acceleration)
                                } else {
                                    trace!("Compressing using LZ4HC");
                                    lz4_hc::compress(raw_block, &mut comp_buffer, args.level)
                                };

                                match res {
                                    // Ok(size) the block was compressed sucessfully.
                                    // And their size is less than the original.
                                    Ok(comp_size) if comp_size < args.block_size as usize => {
                                        // Extend the message block data with the buffer data
                                        trace!("Compressed from {} to {} bytes", raw_block.len(), comp_size);
                                        out_message
                                            .data
                                            .extend_from_slice(&comp_buffer[..comp_size]);
                                        // Setting the block as compressed and storing the final size.
                                        out_message.compressed[i] = true;
                                        out_message.blocksize[i] = comp_size as u32;
                                    }

                                    // - Ok(>raw_block): The size is the same or bigger, and doesn't worth to compress.
                                    Ok(comp_size) => {
                                        // Use the original data and set the size to the original one
                                        trace!("Data was no compressed because the size {:?} is bigger than the original ({})", comp_size, raw_block.len());
                                        out_message.data.extend_from_slice(
                                            &message.data[(i * args.block_size as usize)
                                                ..((i + 1) * args.block_size as usize)],
                                        );
                                        // Setting the block as non compressed and storing the real size.
                                        out_message.compressed[i] = false;
                                        out_message.blocksize[i] = args.block_size as u32;
                                    }

                                    Err(reason) => {
                                        error!("There was an error compressing the block: {:?}", reason);
                                        worker_kill_switch.store(true, Ordering::Relaxed);
                                    }
                                }

                                // Time to fix the data block to ensure that matches the alignment.
                                // Calculate the alignment using the data size
                                let padding_needed =
                                    (alignment - (out_message.blocksize[i] as usize % alignment)) % alignment;

                                // If the data doesn't matches the required position, pad the data with zeroes and correct the size.
                                if padding_needed > 0 {
                                    trace!("Padding the block with {} bytes", padding_needed);
                                    out_message.blocksize[i] += padding_needed as u32;
                                    out_message
                                        .data
                                        .resize(out_message.blocksize[i] as usize, 0);
                                }
                            }

                            // Send the compressed data to the output queue to be processed by the writer.
                            let sent = tx.send(out_message);
                            if sent.is_err() {
                                error!(
                                    "There was an error sending the processed data to the writer"
                                );
                                worker_kill_switch.store(true, Ordering::Relaxed);
                            }
                        }
                        None => {
                            debug!(
                                "There are no more messages in the queue. Closing the thread..."
                            );
                            break; // The reading channel was closed so the worker stops
                        }
                    }
                }
            });
        if worker.is_err() {
            error!("There was an error creating the workers threads!");
            kill_switch.store(true, Ordering::Relaxed);
        }
        workers.push(worker.unwrap());
    }

    // Drop the output queue. This will allow the writer to detect when there's no more data (workers will drop the tx_out too).
    drop(tx_out);

    // Clone the kill_switch
    let reader_kill_switch = Arc::clone(&kill_switch);
    // Initialize the reader thread
    let reader_thread = thread::Builder::new()
        .name("Reader".to_string())
        .spawn(move || {
            let Ok(file_metadata) = input_file.metadata() else {
                error!("There was an error getting the source file information");
                reader_kill_switch.store(true, Ordering::Relaxed);
                return;
            };
            let file_size = file_metadata.len();

            debug!("Total input file size: {} bytes", file_size);
            let mut read_bytes_left = file_size;
            let mut id = 0;

            while read_bytes_left > 0 {
                // If the kill_switch was activated, break the loop
                if reader_kill_switch.load(Ordering::Relaxed) {
                    debug!("Kill switch activated. Breaking the loop.");
                    break;
                }

                let to_read: usize = cmp::min(read_bytes_left as usize, queue_real_size);
                let mut data = vec![0; to_read];

                debug!("Reading the {} block with a size of {}", id, to_read);
                let _ = input_file.read_exact(&mut data);

                // Si la cola está llena (100 elementos), send() pausará este hilo automáticamente
                tx_in
                    .send(MessageBlock {
                        id: id,
                        compressed: Vec::new(),
                        blocksize: Vec::new(),
                        data: data,
                    })
                    .unwrap();

                id += 1;
                let current_pos = input_file.stream_position();
                match current_pos {
                    Ok(position) => read_bytes_left = file_size - position,
                    Err(value) => {
                        error!(
                            "There was an error reading the input file position{:?}",
                            value
                        );
                        reader_kill_switch.store(true, Ordering::Relaxed);
                    }
                }
            }
        });

    if reader_thread.is_err() {
        error!("There was an error creating the reader thread!");
        kill_switch.store(true, Ordering::Relaxed);
    }

    // Clone the kill_switch
    let writer_kill_switch = Arc::clone(&kill_switch);
    // Initialize the writter
    let writer_thread = thread::Builder::new()
        .name("Writer".to_string())
        .spawn(move || {
            let mut expected_id = 0;
            // Temporal buffer for the block ordering
            let mut out_of_order_buffer: HashMap<usize, MessageBlock> = HashMap::new();

            // The loop works until all the workers tx_out are closed, pausing when there is no data.
            for block in rx_out {
                // If the kill_switch was activated, break the loop
                if writer_kill_switch.load(Ordering::Relaxed) {
                    debug!("Kill switch activated. Breaking the loop.");
                    break;
                }

                // Check if the received block is the expected one
                if block.id == expected_id {
                    debug!("Writing the block {} from queue", expected_id);
                    // If the order matches then write it to the file
                    let _ = output_file.write_all(&block.data);
                    expected_id += 1;

                    // Check if the next blocks are in the ordering buffer
                    while let Some(buffered_data) = out_of_order_buffer.remove(&expected_id) {
                        debug!("Writing the block {} from the buffer", expected_id);
                        // If the next block was waiting in the buffer, then write it into the file
                        let _ = output_file.write_all(&buffered_data.data);
                        expected_id += 1;
                    }
                } else {
                    debug!(
                        "The received block ({}) was not expected right now (expected: {})",
                        block.id, expected_id
                    );
                    // If the block doesn't matches the expected block then store it in the ordering buffer.
                    out_of_order_buffer.insert(block.id, block);
                }
            }
        });

    if writer_thread.is_err() {
        error!("There was an error creating the writer thread!");
        kill_switch.store(true, Ordering::Relaxed);
    }

    // Wait for every thread and then finalize the program
    reader_thread.unwrap().join().unwrap();
    for w in workers {
        w.join().unwrap();
    }
    writer_thread.unwrap().join().unwrap();

    info!("File compressed succesfully!");

    Ok(true)
}

fn decompressor(args: Args, mut input_file: File, mut output_file: File) -> Result<bool, String> {
    info!("The input file is a ZSO file. Decompressing...");

    Ok(true)
}
