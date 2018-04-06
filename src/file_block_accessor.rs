use std::fs::File;
use block_accessor::*;
use std::io;
use std::io::{Seek, Read};

pub struct BlockAccessFile {
    backing_file: File
}

impl BlockAccessFile {
    pub fn new(file_name: &str) -> io::Result<BlockAccessFile> {
        let file = File::open(file_name)?;
        Ok(BlockAccessFile {backing_file: file})
    }
}

impl BlockAccessor for BlockAccessFile {
    fn block_size(&self) -> u64 {
        512
    }

    fn read_block(&mut self, block_num: u64, block: &mut [u8]) {
        self.backing_file.seek(io::SeekFrom::Start(block_num*512)).unwrap();
        self.backing_file.read(block).unwrap();
    }

    fn write_block(&mut self, _block: &[u8]) -> Result<(), BlockAccessError> {
        Err(BlockAccessError::MiscError)
    }
}
