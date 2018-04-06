mod block_accessor;
mod file_block_accessor;

#[cfg(test)]
mod tests {
    use block_accessor::BlockAccessor;
    use file_block_accessor::BlockAccessFile;

    #[test]
    fn it_works() {
        let mut t = BlockAccessFile::new("src/lib.rs").unwrap();
        let mut block = [0u8;512];
        t.read_block(0, &mut block);
        for val in block.iter() {
            print!("{}", *val as char);
        }
    }
}
