use std::cmp;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

// 1. Estructura de nuestro bloque de datos
struct Block {
    id: usize,
    data: Vec<u8>,
}

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
