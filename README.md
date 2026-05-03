# ZISO Compressor

ZISO is a Rust-based multithreaded CSO/ZSO converter designed for fast compression and decompression of disc images. It supports LZ4 compression, optional HDL fix alignment, configurable block size, and multithreaded processing for efficient performance.

## Features

- Compress and decompress ISO/ZSO files
- Multithreaded processing with automatic CPU core detection
- LZ4 compression with optional HC mode
- Configurable block size
- HDL fix support for applications like `hdl_dump`
- Release workflow for multiplatform builds

## Comparison with previous implementation

This version improves my other existing tool written in C++ at [Danixu/ziso_compressor](https://github.com/Danixu/ziso_compressor) in areas such as:

- Multithreaded processing with better thread coordination
- Cleaner worker/reader/writer separation
- Improved error handling and cancellation
- More consistent padding and index handling for ZSO files
- Written in Rust for a better and safer resources usage.

Future updates will add benchmark comparisons, but on the first tests this tool is about 5x-6x faster.

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
