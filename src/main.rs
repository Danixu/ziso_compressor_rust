use clap::Parser;
use log::{debug, error, info, warn};
use std::cmp;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

// Message block used to exchange data between threads
struct MessageBlock {
    id: usize,
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
    debug!("Getting the program arguments");
    let args = Args::parse();

    // Check the input file existence
    debug!("Checking if the input file exists.");
    let input_filename = args.input.clone();
    debug!("Input file: {:?}", input_filename);
    if !input_filename.exists() || !input_filename.is_file() {
        error!(
            "The input file {:?} doesn't exists or is not valid.",
            input_filename
        );
        std::process::exit(1);
    }
    debug!("Opening the input file as File.");
    let mut input_file = File::open(&input_filename).unwrap_or_else(|e| {
        error!(
            "Fatal error: The input file '{:?}' cannot be readed: {}",
            input_filename, e
        );
        std::process::exit(1)
    });

    debug!("Checking if the input file is an already compressed CSO");
    let compressed: bool = {
        let mut input_header: [u8; 4] = [0; 4];

        input_file
            .read_exact(&mut input_header)
            .unwrap_or_else(|e| {
                error!("Fatal error: Cannot read the input file header");
                std::process::exit(1)
            });

        &input_header == b"CISO"
    };
    debug!("It's compressed?: {}", compressed);

    // If the output is empty, then generate the filename based in the input
    debug!("Checking if the output file exists");
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
    debug!("Opening the output file as File in write mode and truncate");
    let mut output_file = OpenOptions::new()
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
        info!("The input file is a CSO file. Decompressing...");
    } else {
        info!("The input file is a image file. Compressing...");
        let _ = compressor(&args, input_file, output_file);
    }
    ()
}

fn compressor(args: &Args, mut input_file: File, mut output_file: File) -> Result<bool, String> {
    let input_metadata = input_file.metadata().unwrap_or_else(|e| {
        error!("Error reading the input file metadata: {}", e);
        std::process::exit(1)
    });

    // Calculate the number of blocks in the file
    let total_blocks: usize =
        (input_metadata.len() as usize + (args.block_size - 1) as usize) / args.block_size as usize;
    debug!("Number of blocks: {:?}", total_blocks);

    // Index table
    let mut index_table: Vec<u32> = vec![0; total_blocks as usize + 1];

    // Write the CSO header
    let mut header = [0u8; 24];
    header[0..4].copy_from_slice(b"CISO"); // Magic string
    header[4..8].copy_from_slice(&24u32.to_le_bytes()); // Header size (always 24)
    header[8..16].copy_from_slice(&input_metadata.len().to_le_bytes()); // Original size without compress
    header[16..20].copy_from_slice(&args.block_size.to_le_bytes()); // Block size
    header[20] = 1; // Format version (v1)
    header[21] = 0; // Index alignment
    // The 22th and 23th bytes are reserved

    let _ = output_file.write_all(&header);

    // A way to stop all the threads if any of them has failed
    let kill_switch = Arc::new(AtomicBool::new(false));

    Ok(true)
}
/*
fn main() {
    // Usamos sync_channel para poner un límite.
    // Si hay 100 bloques en la cola, el que intente enviar se pausará.
    let (tx_in, rx_in) = mpsc::sync_channel::<Block>(16);
    let (tx_out, rx_out) = mpsc::sync_channel::<Block>(16);

    // Como varios workers van a leer de rx_in, necesitamos protegerlo con un Mutex.
    // Arc (Atomic Reference Counting) nos permite compartirlo entre los hilos de forma segura.
    let shared_rx_in = Arc::new(Mutex::new(rx_in));

    let num_workers = 4; // Puedes ajustarlo al número de núcleos físicos que tengas
    let mut workers = vec![];

    // 2. Iniciar los Trabajadores (Hilos Compresores)
    for _ in 0..num_workers {
        let rx = Arc::clone(&shared_rx_in);
        let tx = tx_out.clone();

        let worker = thread::spawn(move || {
            loop {
                // Tomamos el lock de la cola, sacamos un bloque y soltamos el lock inmediatamente
                let block_opt = {
                    let lock = rx.lock().unwrap();
                    lock.recv().ok() // ok() devuelve None si la cola se ha cerrado (fin de archivo)
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

                        // Enviamos el resultado a la cola de salida
                        tx.send(Block {
                            id: block.id,
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

    // Importante: El hilo principal suelta su copia del transmisor de salida.
    // Así, el hilo Escritor sabrá que ya no quedan más datos cuando los workers terminen.
    drop(tx_out);

    // 3. Iniciar el Productor (Hilo Lector)
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
            let to_read: usize = cmp::min(read_bytes_left as usize, 2048000);
            let mut data = vec![0; to_read];

            //println!("Reading the {} block with a size of {}", id, to_read);
            let _ = file.read_exact(&mut data);

            // Si la cola está llena (100 elementos), send() pausará este hilo automáticamente
            tx_in.send(Block { id, data }).unwrap();

            id += 1;
            read_bytes_left = file_size
                - file
                    .stream_position()
                    .expect("There was an erorr getting the stream pos");
        }
    });

    // 4. Iniciar el Consumidor (Hilo Escritor)
    let writer_thread = thread::spawn(move || {
        let mut expected_id = 0;
        // Búfer temporal para bloques que llegaron antes de tiempo (desordenados)
        let mut out_of_order_buffer: HashMap<usize, Vec<u8>> = HashMap::new();

        let mut file =
            File::create("test.out").expect("There was an error opening the output file");

        // El for loop sobre rx_out lee continuamente y se pausa si no hay datos.
        // Termina solo cuando todos los workers han cerrado su 'tx_out'.
        for block in rx_out {
            if block.id == expected_id {
                // Es el bloque que esperábamos. Lo escribimos.
                // AQUÍ VA TU LÓGICA DE ESCRITURA EN DISCO
                //println!("Escribiendo bloque a disco: {}", expected_id);
                let _ = file.write_all(&block.data);
                expected_id += 1;

                // Ahora comprobamos si los siguientes bloques ya estaban esperando en el búfer
                while let Some(buffered_data) = out_of_order_buffer.remove(&expected_id) {
                    // AQUÍ VA TU LÓGICA DE ESCRITURA EN DISCO (del dato guardado en memoria)
                    //println!("Escribiendo bloque desde memoria: {}", expected_id);
                    let _ = file.write_all(&buffered_data);
                    expected_id += 1;
                }
            } else {
                // El bloque llegó antes de su turno (por ejemplo, llegó el 3 pero esperamos el 2).
                // Lo guardamos en el diccionario temporal de memoria.
                out_of_order_buffer.insert(block.id, block.data);
            }
        }
    });

    // 5. Esperar a que todo termine limpiamente
    reader_thread.join().unwrap();
    for w in workers {
        w.join().unwrap();
    }
    writer_thread.join().unwrap();

    println!("¡Compresión finalizada con éxito!");
}
*/
