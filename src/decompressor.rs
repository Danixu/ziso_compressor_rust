use super::args::Args;
use super::types::{MessageBlock, QUEUE_SIZE, QUEUE_TRANSFER_SIZE};
use super::utils::u8_from_disk_to_u32;
use log::{debug, error, info, trace};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, current};

pub fn decompressor(args: Args, mut input_file: File, mut output_file: File) -> Result<(), String> {
    info!("The input file is a ZSO file. Decompressing...");

    let input_metadata = input_file
        .metadata()
        .map_err(|e| format!("Error reading the input file metadata: {}", e))?;

    let mut input_file = BufReader::new(input_file);
    let mut output_file = BufWriter::new(output_file);

    // Getting the header
    debug!("Reading the ZSO header");
    let mut header: [u8; 24] = [0; 24];
    input_file
        .read_exact(&mut header)
        .map_err(|e| format!("Cannot read the ZSO header: {}", e))?;
    trace!("ZSO header: {:?}", &header);
    // Check the magic string
    if &header[0..4] != b"ZISO" {
        return Err("The input file is not a valid ZSO file (invalid magic string)".to_string());
    }

    // Get the original file size, block size and alignment
    let original_size = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let block_size = u32::from_le_bytes(header[16..20].try_into().unwrap());
    let pos_shift = header[21];
    debug!("Original size: {}", original_size);
    debug!("Block size: {}", block_size);
    debug!("Position shift: {}", pos_shift);

    // Determine the original blocks count
    let blocks_count = (original_size + (block_size as u64 - 1)) / block_size as u64;
    debug!("Blocks count: {}", blocks_count);

    // Read the index table
    debug!("Reading the index table");
    let index_table_size = ((blocks_count + 1) * 4) as usize;
    let mut index_table_bytes = vec![0u8; index_table_size];
    input_file
        .read_exact(&mut index_table_bytes)
        .map_err(|e| format!("Cannot read the index table: {}", e))?;

    let index_table: Vec<u32> = u8_from_disk_to_u32(&index_table_bytes);
    trace!("Index table: {:?}", &index_table);
    if index_table.is_empty() {
        return Err("Index table is empty".to_string());
    }

    // Verify the index table integrity
    debug!("Verifying the index table integrity");
    let mut current_pos = input_file
        .stream_position()
        .map_err(|e| format!("Cannot get the current position in the input file: {}", e))?;
    debug!(
        "Current position after reading the index table: {}",
        current_pos
    );
    let real_file_size = input_metadata.len();
    debug!("Real input file size: {}", real_file_size);

    // Calculate the padding to ensure that the compressed data starts at the correct alignment
    let padding_needed = (pos_shift - (current_pos % pos_shift as u64) as u8) % pos_shift;
    if padding_needed > 0 {
        current_pos += padding_needed as u64;
    }

    if index_table[0] != (current_pos >> pos_shift) as u32 {
        return Err(format!(
            "The first entry of the index table must match the compressed data start position: {}",
            (current_pos >> pos_shift) as u32
        ));
    } else if index_table[index_table.len() - 1] != (real_file_size >> pos_shift) as u32 {
        return Err(format!(
            "The last entry of the index table must match the input file size: {}",
            (real_file_size >> pos_shift) as u32
        ));
    }

    // A way to stop all the threads if any of them has failed
    let kill_switch = Arc::new(AtomicBool::new(false));

    // We get the max number of blocks to read. In case that they are uncompressed, will almost fit the buffer.
    // Will keep the size under control in the output buffer
    let queue_max_blocks: usize = QUEUE_TRANSFER_SIZE / args.block_size as usize;

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
                // Thread worker
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
            // Reader thread
            let mut current_block: usize = 0;
            let mut id = 0;

            while current_block < index_table_size {
                // If the kill_switch was activated, break the loop
                if reader_kill_switch.load(Ordering::Relaxed) {
                    debug!("Kill switch activated. Returning...");
                    return;
                }

                let readable_blocks = {
                    if (index_table_size - current_block) < queue_max_blocks {
                        index_table_size - current_block
                    } else {
                        queue_max_blocks
                    }
                };

                // Read the max number of blocks that fits the buffers
                let first_position = (index_table[current_block] << pos_shift) as usize;
                let last_position =
                    (index_table[current_block + readable_blocks + 1] << pos_shift) as usize;

                // Data to read
                let to_read = last_position - first_position;
                let mut data = vec![0; to_read];
                let mut cb_compressed: Vec<bool> = vec![false; readable_blocks];
                let mut cb_blocksize: Vec<u32> = vec![0; readable_blocks];

                debug!("Reading {} blocks with a size of {}", id, to_read);
                let _ = input_file.read_exact(&mut data);

                debug!("Setting the block info (compression and blocksize)");
                for i in 0..readable_blocks {
                    // Set the compressed state
                    cb_compressed[i] = (index_table[current_block + i] & 0x80000000) == 0;

                    // Calculate the block size
                    let start_offset_raw = index_table[current_block + i] & 0x7FFFFFFF;
                    let end_offset_raw = index_table[current_block + i + 1] & 0x7FFFFFFF;
                    cb_blocksize[i] = end_offset_raw - start_offset_raw;
                }

                // Send to the input channel
                tx_in
                    .send(MessageBlock {
                        id: id,
                        compressed: cb_compressed,
                        blocksize: cb_blocksize,
                        data: data,
                    })
                    .unwrap();

                id += 1;
                current_block += readable_blocks;
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
            // Variable to keep the blocks order
            let mut expected_id = 0;
            // Temporal buffer for the block ordering
            let mut out_of_order_buffer: HashMap<usize, MessageBlock> = HashMap::new();

            // The loop works until all the workers tx_out are closed, pausing when there is no data.
            for block in rx_out {
                // If the kill_switch was activated, break the loop
                if writer_kill_switch.load(Ordering::Relaxed) {
                    debug!("Kill switch activated. Returning...");
                    return;
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

            // Flush the output file to ensure all data is written to disk
            let _ = output_file.flush();
        });

    if writer_thread.is_err() {
        error!("There was an error creating the writer thread!");
        kill_switch.store(true, Ordering::Relaxed);
        return Err("Error creating writer thread".to_string());
    }

    // Wait for every thread and then finalize the program
    match reader_thread.unwrap().join() {
        Ok(_) => debug!("Waiting for the reader thread."),
        Err(e) => {
            kill_switch.store(true, Ordering::Relaxed);
            return Err(format!("Error waiting for reader thread: {:?}", e));
        }
    }
    for w in workers {
        match w.join() {
            Ok(_) => debug!("Waiting for the worker thread."),
            Err(e) => {
                kill_switch.store(true, Ordering::Relaxed);
                return Err(format!("Error waiting for worker thread: {:?}", e));
            }
        }
    }
    match writer_thread.unwrap().join() {
        Ok(_) => debug!("Waiting for the writer thread."),
        Err(e) => {
            kill_switch.store(true, Ordering::Relaxed);
            return Err(format!("Error waiting for writer thread: {:?}", e));
        }
    }

    info!("File decompressed succesfully!");

    Ok(())
}
