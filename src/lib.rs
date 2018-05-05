#![no_std]

#[cfg(test)]
#[macro_use]
extern crate std;

// Crates with macros
#[macro_use]
extern crate bitflags;

// Other crates
extern crate crc16;
extern crate embedded_hal;
extern crate heapless;
extern crate nb;

// Internal crates
extern crate block_accessor;

// Crate modules
pub mod byte_util;
pub mod fat32;
pub mod mbr;
pub mod sd;

#[cfg(test)]
mod tests {
    extern crate linux_embedded_hal;
    extern crate file_block_accessor;
    extern crate md5;

    use block_accessor::{BlockAccessor};
    use self::file_block_accessor::BlockAccessFile;
    use self::linux_embedded_hal::spidev::{Spidev, SpidevOptions, SPI_MODE_0};

    use sd::SDCard;
    use mbr::MBR;
    use fat32::{Fat32, DirectoryItem};

    use std::fs::File;
    use std::io::prelude::*;
    use std::vec::Vec;

    use embedded_hal::blocking::delay::DelayMs;
    use embedded_hal::spi::FullDuplex;

    use nb;

    struct SpidevAdapter {
        spi: Spidev
    }

    impl SpidevAdapter {
        fn get() -> SpidevAdapter {
            let mut spi = Spidev::open("/dev/spidev0.0").unwrap();
            let mut options = SpidevOptions::new();
            options
                 .bits_per_word(8)
                 .max_speed_hz(1_000_000)
                 .mode(SPI_MODE_0)
                 .build();
            spi.configure(&options).unwrap();

            SpidevAdapter { spi }
        }
    }

    impl FullDuplex<u8> for SpidevAdapter {
        type Error = ();

        fn read(&mut self) -> nb::Result<u8, Self::Error> {
            let mut response_byte: [u8; 1] = [0xFF];
            self.spi.read(&mut response_byte).unwrap();
            Ok(response_byte[0])
        }

        fn send(&mut self, byte: u8) -> nb::Result<(), Self::Error> {
            self.spi.write(&[byte]).unwrap();
            Ok(())
        }
    }

    struct Delayer {}

    impl Delayer {
        fn new() -> Delayer {
            Delayer {}
        }
    }

    impl DelayMs<u8> for Delayer {
        fn delay_ms(&mut self, ms: u8) {
            use std::{thread, time};
            let millis = time::Duration::from_millis(ms as u64);
            thread::sleep(millis);
        }
    }

    #[test]
    fn basic_file_block_access() {
        let mut t = BlockAccessFile::new("../card-dump/sd-trim.img").unwrap();
        let mut block = [0;512];
        t.read_block(0, &mut block);
        assert_eq!(block[510], 0x55);
        assert_eq!(block[511], 0xAA);
    }

    #[test]
    fn basic_ls() {
        let mut t = BlockAccessFile::new("../card-dump/sd-trim.img").unwrap();

        let mut block = [0; 512];
        t.read_block(0, &mut block);

        let mbr = MBR::from_bytes(&block);
        let partition = mbr.partition_entries.get(0).unwrap().as_ref().unwrap();

        let mut fat32 = Fat32::new(t, partition.first_sector_block_address);
        // fat32.ls_cluster(3);

        for item in fat32.iter_contents_of_directory_cluster(4) {
            match item {
                DirectoryItem::File(f) => {
                    println!("{:?}", f.name);
                },
                DirectoryItem::Directory(d) => {
                    println!("{:?}", d.name);
                }
            }
        }

        match fat32.item_info("projects/shmorc/python_welcome.mp3").unwrap() {
            DirectoryItem::File(f) => {
                assert_eq!(f.size, 19225);

                let mut local_file = File::create("python_welcome.mp3").unwrap();

                for block in fat32.iter_file(f) {
                    local_file.write(&block).unwrap();
                }
            },
            _ => panic!("Should be a file")
        }

        let mut f = File::open("python_welcome.mp3").unwrap();
        let mut data = Vec::new();
        data.resize(19225, 0);
        f.read(&mut data).unwrap();
        assert_eq!(format!("{:x}", md5::compute(&data)), "092e9eb573c0c158acb29f76b3287440");

        println!("{:?}", fat32.item_info("projects"));
        // assert!(false);
    }

    #[test]
    #[ignore]
    fn sd_read() {
        let mut block = [0; 512];
        let spi = SpidevAdapter::get();
        let mut sd = SDCard::new(spi, Delayer::new()).unwrap();
        sd.read_block(0, &mut block);
        assert_eq!(block[510], 0x55);
        assert_eq!(block[511], 0xAA);

        sd.read_block(2048, &mut block);
        assert_eq!(block[0], 235);
        assert_eq!(block[1],  88);

        sd.read_block(0, &mut block);
        let mbr = MBR::from_bytes(&block);
        let partition1 = mbr.partition_entries.get(0).unwrap().as_ref().unwrap();
        let partition2 = mbr.partition_entries.get(1).unwrap().as_ref().unwrap();
        assert_eq!(mbr.partition_count(), 2);
        assert_eq!(partition1.partition_type, 0x0B);
        assert_eq!(partition2.partition_type, 0x0B);

        sd.read_block(2048, &mut block);
        println!("Boot Sector Bytes");
        for b in block.iter() {
            print!("{}, ", b);
        }
        println!("");
        println!("{:?}", mbr);
    }

    #[test]
    #[ignore]
    fn fat32_basic() {
        let spi = SpidevAdapter::get();
        let mut sd = SDCard::new(spi, Delayer::new()).unwrap();

        let mut block = [0; 512];
        sd.read_block(0, &mut block);

        let mbr = MBR::from_bytes(&block);
        let partition = mbr.partition_entries.get(0).unwrap().as_ref().unwrap();

        let mut _fat32 = Fat32::new(sd, partition.first_sector_block_address);
        // assert_eq!(fat32.ls(&[""]), vec!["projects".to_string(), "TEST_PAR.T1".to_string()]);
    }
}
