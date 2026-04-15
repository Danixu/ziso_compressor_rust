use clap::Parser;
use std::path::PathBuf;
use std::thread;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Input file. Example: game.iso
    pub input: PathBuf,

    /// Output file. Example: game.zso (optional)
    pub output: Option<PathBuf>,

    /// Force to overwrite the output file if exists
    #[arg(short, long, default_value_t = false)]
    pub force: bool,

    /// Threads number used to compress, by default the CPU cores.
    #[arg(short = 't', long, default_value_t = default_threads())]
    pub threads: usize,

    /// LZ4 compression level (1-12)
    #[arg(short = 'l', long, default_value_t = 12, value_parser = clap::value_parser!(i32).range(1..=12))]
    pub level: i32,

    /// Disable the LZ4HC compression
    #[arg(long = "nohc", default_value_t = false)]
    pub disable_hc: bool,

    /// The size of every block in the ZSO file (2048-131072)(recommended 2048 for HDL).
    #[arg(long, default_value_t = 2048, value_parser = clap::value_parser!(u32).range(2048..=131072))]
    pub block_size: u32,

    /// HDL Fix to avoid a bug in the hdl_dump
    #[arg(long, default_value_t = false)]
    pub hdl_fix: bool,
}

// Simple function to return the number of available cores in the system
pub fn default_threads() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}
