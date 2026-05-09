# ZISO Compressor

ZISO is a Rust-based multithreaded CSO/ZSO converter designed for fast compression and decompression of disc images. It supports LZ4 compression, optional HDL fix alignment, configurable block size, and multithreaded processing for efficient performance.

## Features

- Compress and decompress ISO/ZSO files
- Multithreaded processing with automatic CPU core detection
- LZ4 compression with optional HC mode
- Configurable block size
- HDL fix support for applications like `hdl_dump`
- Release workflow for multiplatform builds

## Download

Download the latest binaries for your platform from the [GitHub releases page](https://github.com/Danixu/ziso_compressor/releases/latest):

- **Linux x86_64 (MUSL)** - `ziso-linux-x86_64-musl.zip`
- **Linux x86_64 (glibc)** - `ziso-linux-x86_64-gnu.zip`
- **Windows x64** - `ziso-windows-x64.zip`
- **macOS ARM64** - `ziso-macos-arm64.zip`
- **macOS x64** - `ziso-macos-x64.zip`

### ⚠️ Note on MUSL vs glibc builds

The MUSL build is statically linked and more portable, but **significantly slower** than the glibc version. If you're using Linux with glibc (the default on most distributions), we strongly recommend using the glibc build for better performance. The glibc version offers **up to 12x better performance** due to optimized memory allocation, system call handling, and lz4 compressor optimizations. Tests on an i7-1355U show approximately 9 seconds with the glibc version vs 80 seconds with the MUSL version.

**Recommendation:**
- Use **glibc build** if available on your system (faster, recommended)
- Use **MUSL build** only if you need portability across different Linux distributions

## Comparison with previous implementation

This version improves my other existing tool written in C++ at [Danixu/ziso_compressor](https://github.com/Danixu/ziso_compressor) in areas such as:

- Multithreaded processing with better thread coordination
- Cleaner worker/reader/writer separation
- Improved error handling and cancellation
- More consistent padding and index handling for ZSO files
- Written in Rust for a better and safer resources usage.

## Benchmarks: Original C++ ZISO vs Rust ZISO

These benchmarks compare the original single-threaded C++ implementation with this multithreaded Rust version compiled with glibc. Testing was performed on a Slimbook Elemental 15 with an Intel i7-1355U processor and 32GB RAM running Kubuntu 26.04. Each benchmark was run 3 times and the best result is reported. The speedup column shows the performance improvement ratio of the Rust version over the C++ version.

### Compression

| PS2 Game Title | Original Size | Compressed Size | C++ ZISO Time | Rust ZISO Time | Speedup |
|----------------|---------------|-----------------|---------------|----------------|---------|
| 24 - The Game | 5.8GB | 5.2GB | 59.40s | 10.90s | 5.4x |
| Arcade Action - 30 Games | 358MB | 290MB | 4.50s | 0.81s | 5.6x |
| Arcade Classics Volume 1 | 188MB | 165MB | 2.41s | 0.50s | 4.8x |
| Armored Core - Last Raven | 3.8GB | 3.2GB | 37.65s | 6.61s | 5.7x |
| Armored Core - Nexus (Disc 1) | 4.3GB | 3.6GB | 41.96s | 8.66s | 4.8x |
| Silent Line - Armored Core | 1.8GB | 1.4GB | 25.42s | 4.19s | 6.1x |

### Decompression

| PS2 Game Title | Original Size | C++ ZISO Time | Rust ZISO Time | Speedup |
|----------------|---------------|---------------|----------------|---------|
| 24 - The Game | 5.8GB | 4.77s | 3.46s | 1.38x |
| Arcade Action - 30 Games | 358MB | 0.26s | 0.33s | 0.8x |
| Arcade Classics Volume 1 | 188MB | 0.14s | 0.18s | 0.8x |
| Armored Core - Last Raven | 3.8GB | 3.39s | 2.00s | 1.7x |
| Armored Core - Nexus (Disc 1) | 4.3GB | 3.88s | 2.31s | 1.7x |
| Silent Line - Armored Core | 1.8GB | 1.48s | 0.97s | 1.5x |

In this case the bottleneck is the NVME disk, so with small files the threads overhead is bigger than the performance improvement.

## Usage

```bash
ziso [OPTIONS] <INPUT> [OUTPUT]******
```

### Arguments******

- `<INPUT>`: Input file path. Example: `game.iso`
- `[OUTPUT]`: Optional output path. If omitted, the tool generates a file name using the appropriate extension (`.zso` for compression, `.iso` for decompression)

### Options

- `-f, --force`
  - Overwrite the output file if it already exists.

- `-t, --threads <THREADS>`
  - Number of threads used for compression or decompression.
  - Default: number of available CPU cores.

- `-l, --level <LEVEL>`
  - LZ4 compression level (1-12).
  - Default: `12`.

- `--nohc`
  - Disable LZ4HC compression and use standard LZ4 acceleration.

- `--block-size <BLOCK_SIZE>`
  - Block size for the ZSO file.
  - Valid range: `2048` to `131072`.
  - Recommended: `2048` for HDL compatibility.

- `--hdl-fix`
  - Apply HDL fix alignment to avoid a bug in `hdl_dump`.

- `-h, --help`
  - Display command help.

- `-V, --version`
  - Display version information.

## Examples

Compress an ISO to ZSO using defaults:

```bash
ziso game.iso
```

Compress with a custom output name and force overwrite:

```bash
ziso game.iso game.zso --force
```

Compress with a lower LZ4 level and disable HC mode:

```bash
ziso game.iso game.zso --level 6 --nohc
```

Compress with HDL fix support:

```bash
ziso game.iso game.zso --hdl-fix
```

Decompress a ZSO file back to ISO:

```bash
ziso game.zso
```

## Build

Requires Rust toolchain installed.

```bash
cargo build --release
```

To build for a specific target:

```bash
cargo build --release --target x86_64-unknown-linux-musl
```

## Release Workflow

The repository includes a GitHub Actions workflow that triggers on new tags matching `v*`. It builds release binaries for:

- Linux x86_64 MUSL
- Linux x86_64 glibc
- Windows x64
- macOS ARM64
- macOS x64

The workflow packages each binary as a ZIP file and creates a GitHub release with the artifacts.

## License

Apache License 2.0

## Changelog

### 1.0.0

- Initial release of `ziso`.
- Added multithreaded ZSO compression and decompression.
- Added LZ4 compression level control and optional `--nohc` mode.
- Added `--block-size` support and HDL fix alignment.
- Added GitHub Actions release workflow.
