pub fn u32_to_disk_u8(data: &Vec<u32>) -> Vec<u8> {
    let mut output: Vec<u8> = Vec::new();
    for value in data {
        output.extend_from_slice(&value.to_le_bytes());
    }
    output
}
