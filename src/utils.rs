pub fn u32_to_disk_u8(data: &Vec<u32>) -> Vec<u8> {
    let mut output: Vec<u8> = Vec::new();
    for value in data {
        output.extend_from_slice(&value.to_le_bytes());
    }
    output
}

pub fn u8_from_disk_to_u32(data: &[u8]) -> Vec<u32> {
    data.chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

pub fn padding_calculator(pos_shift: u8, position: u64) -> u64 {
    if pos_shift == 0 {
        position
    } else {
        let padding_needed: u8 = (pos_shift - (position % pos_shift as u64) as u8) % pos_shift;
        if padding_needed > 0 {
            position + padding_needed as u64
        } else {
            position
        }
    }
}
