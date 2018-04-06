pub enum BlockAccessError {
    BlockOutOfRange,
    MiscError
}

pub trait BlockAccessor {
    fn block_size(&self) -> u64;
    fn read_block(&mut self, block_num: u64, block: &mut [u8]);
    fn write_block(&mut self, block: &[u8]) -> Result<(), BlockAccessError>;
}
