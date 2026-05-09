use super::args::Args;
use super::types::{MessageBlock, QUEUE_SIZE, QUEUE_TRANSFER_SIZE};
use super::utils::{padding_calculator, u8_from_disk_to_u32};
use log::{debug, error, info, trace};
use lzzzz::lz4;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self};
use std::time::Duration;

pub fn decompressor(args: Args, input_file: File, output_file: File) -> Result<(), String> {
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
    let index_table_raw_size = ((blocks_count + 1) * 4) as usize;
    let mut index_table_bytes = vec![0u8; index_table_raw_size];
    input_file
        .read_exact(&mut index_table_bytes)
        .map_err(|e| format!("Cannot read the index table: {}", e))?;

    let index_table: Vec<u32> = u8_from_disk_to_u32(&index_table_bytes);
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
    current_pos = padding_calculator(pos_shift, current_pos);

    debug!("First entry of the index table: {}", index_table[0]);
    debug!(
        "Last entry of the index table: {}",
        index_table[index_table.len() - 1]
    );

    if index_table[0] != (current_pos >> pos_shift) as u32 {
        return Err(format!(
            "The first entry of the index table must match the compressed data start position: {}",
            (current_pos >> pos_shift) as u32
        ));
    } else {
        let index_calculated_size = (index_table[index_table.len() - 1] as u64) << pos_shift;
        let rounded_size = ((index_calculated_size + 2047) / 2048) * 2048;

        if real_file_size != index_calculated_size && real_file_size != rounded_size {
            return Err(format!(
                "The file size doesn't matches the expected size in the index table: must be ({}) or the HDL fix size ({}): got {}",
                index_calculated_size, rounded_size, real_file_size
            ));
        }
    }

    // A way to stop all the threads if any of them has failed
    let kill_switch = Arc::new(AtomicBool::new(false));

    // Contador de bloques procesados para el progreso
    let processed_blocks = Arc::new(AtomicUsize::new(0));
    // Contadores de bytes para calcular el ratio de compresión
    let compressed_bytes = Arc::new(AtomicU64::new(0));
    let original_bytes = Arc::new(AtomicU64::new(0));
    // Kill switch para el hilo de reporte
    let reporter_kill_switch = Arc::new(AtomicBool::new(false));

    // Hilo para reportar el progreso
    let reporter_processed_blocks = Arc::clone(&processed_blocks);
    let reporter_compressed_bytes = Arc::clone(&compressed_bytes);
    let reporter_original_bytes = Arc::clone(&original_bytes);
    let reporter_total_blocks = (index_table.len() - 1) as usize;
    let reporter_kill_switch_clone = Arc::clone(&reporter_kill_switch);
    let reporter_thread = thread::spawn(move || {
        let mut last_percentage = 0;
        loop {
            if reporter_kill_switch_clone.load(Ordering::Relaxed) {
                // Asegurarse de que se imprima el 100% antes de terminar
                let final_comp = reporter_compressed_bytes.load(Ordering::Relaxed);
                let final_orig = reporter_original_bytes.load(Ordering::Relaxed);
                let ratio = if final_comp > 0 {
                    (final_comp as f64 / final_orig as f64) * 100.0
                } else {
                    0.0
                };
                print!("\rProcessed: 100% | Compression ratio: {:.2}%", ratio);
                std::io::Write::flush(&mut std::io::stdout()).unwrap();
                println!(); // Nueva línea al final
                break;
            }
            let current_processed = reporter_processed_blocks.load(Ordering::Relaxed);
            let percentage = if current_processed >= reporter_total_blocks {
                100
            } else {
                (current_processed * 100) / reporter_total_blocks
            };

            let comp = reporter_compressed_bytes.load(Ordering::Relaxed);
            let orig = reporter_original_bytes.load(Ordering::Relaxed);
            let ratio = if comp > 0 {
                (comp as f64 / orig as f64) * 100.0
            } else {
                0.0
            };

            if percentage > last_percentage {
                print!(
                    "\rProcessed: {}% | Compression ratio: {:.2}%",
                    percentage, ratio
                );
                std::io::Write::flush(&mut std::io::stdout()).unwrap();
                last_percentage = percentage;
            }

            thread::sleep(Duration::from_millis(100));
        }
    });

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
        // Clone the processed_blocks counter
        let worker_processed_blocks = Arc::clone(&processed_blocks);
        // Clone the byte counters for compression ratio
        let worker_compressed_bytes = Arc::clone(&compressed_bytes);
        let worker_original_bytes = Arc::clone(&original_bytes);

        debug!("Spawning the new thread");
        let worker: Result<thread::JoinHandle<()>, std::io::Error> = thread::Builder::new()
            .name(format!("Worker-{}", i))
            .spawn(move || {
                // Thread worker
                debug!("Worker thread started");
                debug!("Creating the decompression buffer with a size of {}", block_size);
                let mut decomp_buffer = vec![0u8; block_size as usize];

                loop {
                    // If the kill_switch was activated, break the loop
                    if worker_kill_switch.load(Ordering::Relaxed) {
                        debug!("Kill switch activated. Returning...");
                        return;
                    }

                    // Pull a block from the shared receiver.
                    debug!("Getting a new rx message");
                    let message = {
                        trace!("Getting the rx lock");
                        let lock = rx.lock().unwrap();
                        trace!("Pulling the rx queue message");
                        match lock.recv_timeout(Duration::from_millis(100)) {
                            Ok(message) => message,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    };

                    debug!("Received a message. Processing...");
                    let blocks_number: usize = message.compressed.len();
                    debug!("Processing the block id {} with {} blocks", message.id, blocks_number);
                    let mut out_message = MessageBlock {
                        id: message.id,
                        compressed: Vec::new(),
                        blocksize: Vec::new(),
                        data: Vec::new(),
                    };
                    let mut current_data_offset: usize = 0;

                    for i in 0..blocks_number {
                        trace!(
                            "Working on block {} with offset {} and size {}",
                            i, current_data_offset, message.blocksize[i]
                        );

                        let comp_data: &[u8] = &message.data[current_data_offset
                            ..current_data_offset + message.blocksize[i] as usize];

                        if message.compressed[i] {
                            trace!("Decompressing using LZ4");
                            match lz4::decompress_partial(comp_data, &mut decomp_buffer, block_size as usize) {
                                Ok(decomp_size) => {
                                    trace!(
                                        "The block was decompressed successfully with a size of {}.",
                                        decomp_size
                                    );
                                    out_message.data.extend_from_slice(&decomp_buffer[..decomp_size]);
                                    // Update byte counters
                                    worker_compressed_bytes.fetch_add(message.blocksize[i] as u64, Ordering::Relaxed);
                                    worker_original_bytes.fetch_add(decomp_size as u64, Ordering::Relaxed);
                                }
                                Err(reason) => {
                                    error!(
                                        "There was an error decompressing the block: {:?}",
                                        reason
                                    );
                                    worker_kill_switch.store(true, Ordering::Release);
                                    return;
                                }
                            }
                        } else {
                            trace!("Copying the uncompressed data");
                            out_message.data.extend_from_slice(comp_data);
                            // Update byte counters
                            worker_compressed_bytes.fetch_add(message.blocksize[i] as u64, Ordering::Relaxed);
                            worker_original_bytes.fetch_add(comp_data.len() as u64, Ordering::Relaxed);
                        }

                        // Update processed blocks counter
                        worker_processed_blocks.fetch_add(1, Ordering::Relaxed);
                        current_data_offset += message.blocksize[i] as usize;
                    }

                    debug!("Finished processing the block id {}. Sending to the output queue...", out_message.id);
                    match tx.send(out_message) {
                        Ok(_) => {}
                        Err(_) => {
                            error!("Writer channel disconnected. Aborting decompression");
                            worker_kill_switch.store(true, Ordering::Release);
                            return;
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
            // Reader thread
            let mut current_block: usize = 0;
            let mut id = 0;
            // The number of blocks is equal to the index table entries minus 1
            // (the last one is the end of the last block, not a real block).
            let index_entries = index_table.len() - 1;

            while current_block < index_entries as usize {
                // If the kill_switch was activated, break the loop
                if reader_kill_switch.load(Ordering::Relaxed) {
                    debug!("Kill switch activated. Returning...");
                    return;
                }

                let readable_blocks = {
                    if (index_entries as usize - current_block) < queue_max_blocks {
                        index_entries as usize - current_block
                    } else {
                        queue_max_blocks
                    }
                };
                debug!("Number of blocks that can be readed: {}", readable_blocks);

                // Read the max number of blocks that fits the buffers
                debug!(
                    "Reading the blocks {} to {}",
                    current_block,
                    current_block + readable_blocks
                );
                let starting_position =
                    ((index_table[current_block] & 0x7FFFFFFF) as usize) << pos_shift;
                let ending_position = ((index_table[current_block + readable_blocks] & 0x7FFFFFFF)
                    as usize)
                    << pos_shift;

                debug!("Starting position: {}", starting_position);
                debug!("Ending position: {}", ending_position);

                // Data to read
                let to_read = ending_position - starting_position;
                let mut data = vec![0; to_read];
                let mut cb_compressed: Vec<bool> = vec![false; readable_blocks];
                let mut cb_blocksize: Vec<u32> = vec![0; readable_blocks];

                debug!("Reading the block ID {} with a size of {}", id, to_read);
                if let Err(e) = input_file.read_exact(&mut data) {
                    error!("Error reading data from input file: {}", e);
                    reader_kill_switch.store(true, Ordering::Relaxed);
                    return;
                }

                debug!("Setting the block info (compression and blocksize)");
                for i in 0..readable_blocks {
                    // Set the compressed state
                    cb_compressed[i] = (index_table[current_block + i] & 0x80000000) == 0;

                    // Calculate the block size
                    let start_offset_raw: u64 =
                        ((index_table[current_block + i] & 0x7FFFFFFF) as u64) << pos_shift;
                    let end_offset_raw: u64 =
                        ((index_table[current_block + i + 1] & 0x7FFFFFFF) as u64) << pos_shift;
                    cb_blocksize[i] = (end_offset_raw - start_offset_raw) as u32;
                }

                // Send to the input channel with better error handling
                debug!("Sending the block ID {} to the workers", id);
                match tx_in.send(MessageBlock {
                    id: id,
                    compressed: cb_compressed,
                    blocksize: cb_blocksize,
                    data: data,
                }) {
                    Ok(_) => {
                        id += 1;
                        current_block += readable_blocks;
                    }
                    Err(_) => {
                        error!("Workers channel disconnected. Workers may have panicked.");
                        reader_kill_switch.store(true, Ordering::Relaxed);
                        return;
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
            // Variable to keep the blocks order
            let mut expected_id = 0;
            // Temporal buffer for the block ordering
            let mut out_of_order_buffer: HashMap<usize, MessageBlock> = HashMap::new();

            // The loop works until all the workers tx_out are closed, pausing when there is no data.
            loop {
                // If the kill_switch was activated, break the loop
                if writer_kill_switch.load(Ordering::Relaxed) {
                    debug!("Kill switch activated. Returning...");
                    break;
                }

                match rx_out.recv_timeout(Duration::from_millis(500)) {
                    Ok(block) => {
                        // Check if the received block is the expected one
                        if block.id == expected_id {
                            debug!(
                                "Writing {} bytes from the block {} from queue",
                                block.data.len(),
                                expected_id
                            );
                            // If the order matches then write it to the file
                            if let Err(e) = output_file.write_all(&block.data) {
                                error!("Error writing block {} to output file: {}", expected_id, e);
                                writer_kill_switch.store(true, Ordering::Release);
                                break;
                            }
                            expected_id += 1;

                            // Check if the next blocks are in the ordering buffer
                            while let Some(buffered_data) = out_of_order_buffer.remove(&expected_id)
                            {
                                debug!(
                                    "Writing {} bytes from the block {} from the buffer",
                                    buffered_data.data.len(),
                                    expected_id
                                );
                                // If the next block was waiting in the buffer, then write it into the file
                                if let Err(e) = output_file.write_all(&buffered_data.data) {
                                    error!(
                                        "Error writing buffered block {} to output file: {}",
                                        expected_id, e
                                    );
                                    writer_kill_switch.store(true, Ordering::Release);
                                    return;
                                }
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
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Timeout occurred, loop will check kill_switch again
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        // All workers have finished, channel is closed
                        debug!("Output channel closed. All workers finished processing.");
                        break;
                    }
                }
            }

            // Flush the output file to ensure all data is written to disk
            if let Err(e) = output_file.flush() {
                error!("Error flushing output file: {}", e);
            }
        });

    if writer_thread.is_err() {
        error!("There was an error creating the writer thread!");
        kill_switch.store(true, Ordering::Relaxed);
        return Err("Error creating writer thread".to_string());
    }

    // Wait for every thread and then finalize the program
    // Wait for the reader thread to finish
    let reader_result = reader_thread
        .unwrap()
        .join()
        .map_err(|_| "Reader thread panicked".to_string());

    if reader_result.is_err() {
        kill_switch.store(true, Ordering::Release);
        return reader_result.map(|_| ());
    }

    debug!("Reader thread finished successfully");

    // Wait for all worker threads to finish
    for (idx, w) in workers.into_iter().enumerate() {
        match w.join() {
            Ok(_) => debug!("Worker thread {} finished successfully", idx),
            Err(_) => {
                kill_switch.store(true, Ordering::Release);
                return Err(format!("Worker thread {} panicked", idx));
            }
        }
    }

    // Wait for the writer thread to finish
    match writer_thread.unwrap().join() {
        Ok(_) => debug!("Writer thread finished successfully"),
        Err(_) => {
            kill_switch.store(true, Ordering::Release);
            return Err("Writer thread panicked".to_string());
        }
    }

    // Ensure the reporter thread is stopped
    reporter_kill_switch.store(true, Ordering::Relaxed);
    reporter_thread.join().unwrap();

    info!("File decompressed succesfully!");

    Ok(())
}
