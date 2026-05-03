// Constants
pub const QUEUE_SIZE: usize = 32;
pub const QUEUE_TRANSFER_SIZE: usize = 2 * 1024 * 1024;
pub const LZ4_MAX_ACCELERATION: i32 = 64;

// Message block used to exchange data between threads
pub struct MessageBlock {
    pub id: usize,
    pub compressed: Vec<bool>,
    pub blocksize: Vec<u32>,
    pub data: Vec<u8>,
}
