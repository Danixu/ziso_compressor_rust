# ZSO Format information

ZSO is a custom compressed disc image format similar to CSO version 1. It uses LZ4-based blocks and an index table encoded with a shift value to support larger files.

- Magic bytes: `ZISO`
- Preferred extension: `.zso`
- Header size: `24` bytes
- Format version: `1`

## File structure

The file is composed of:

1. Header
2. Index table
3. Data blocks
4. Optional HDL padding at the end

## Header

The header is always `24` bytes and uses little-endian encoding.

| Offset | Size | Type     | Description |
|-------:|------|----------|-------------|
| `0x00` | 4    | `char[4]` | Magic bytes: `ZISO` |
| `0x04` | 4    | `uint32`  | Header size: `24` |
| `0x08` | 8    | `uint64`  | Original ISO size |
| `0x10` | 4    | `uint32`  | Block size in bytes |
| `0x14` | 1    | `uint8`   | Format version (`1`) |
| `0x15` | 1    | `uint8`   | Position shift (`pos_shift`) |
| `0x16` | 2    | reserved  | Reserved bytes, normally zero |

## Index table

The index table contains `N + 1` entries, where `N` is the number of data blocks.

- `N = ceil(original_size / block_size)`
- The extra final entry is the EOF position for the last block

Each index entry is a `uint32` little-endian value.

### Index entry format

- Bit `31` (highest bit): `1` = uncompressed block, `0` = compressed block
- Bits `0..30`: shifted block start position

The stored position is:

```text
entry_position = block_offset >> pos_shift
```

And the real file offset is recovered with:

```text
block_offset = entry_position << pos_shift
```

The shift value is used so large offsets can still fit in a 31-bit field.

### Position shift values

The tool chooses `pos_shift` based on original file size:

- `0` for files below `0x7FFFFFFF` (~2 GiB)
- `1` for files below `0xFFFFFFFF` (~4 GiB)
- `2` for files below `0x1FFFFFFFF` (~8 GiB)
- `3` for files below `0x3FFFFFFFF` (~16 GiB)
- `4` for larger files

### Example

If `pos_shift = 1` and the raw offset is `0x12345678`, the stored index value is:

```text
entry_value = (0x12345678 >> 1)
```

To recover the offset:

```text
offset = (entry_value << 1)
```

## Data blocks

Data blocks follow the index table and contain one or more compressed or raw blocks in file order.

For each block:

- Blocks are usually compressed with LZ4HC by default
- If the compressed block is not smaller than the original block, the raw block is stored instead
- The index bit `0x8000_0000` marks raw/uncompressed blocks
- Block boundaries are inferred from consecutive index entries

### Block padding and alignment

Each stored block is padded to the nearest multiple of `1 << pos_shift`.

This means:

- The offset of each block starts at an aligned position
- The index entry stores the aligned offset divided by `2^pos_shift`
- The last entry in the index table marks the end-of-file offset

When enabled, the final file is padded at the end to a full `2048`-byte boundary after the index table and data blocks are written. This is useful for compatibility with some HDL tools.

## Compression mode

Compression can be performed with LZ4HC by default, or with standard LZ4 acceleration in compatibility mode.

The format supports decompression of both LZ4-compressed blocks and raw blocks.

## Notes

- The format is implemented specifically by this tool
- `pos_shift` is not a generic LZ4 feature; it is part of the ZISO index encoding used here
- The index stores absolute file offsets, not relative block sizes
- The final index entry is required to compute the size of the last data block
