extern crate spidev;
extern crate crc16;

pub mod block_accessor;
pub mod file_block_accessor;

// External crates
use spidev::{Spidev};

use std::io::prelude::*;

// Internal crates
use block_accessor::{BlockAccessor, BlockAccessError};

pub struct SDCard {
    spi: Spidev
}

#[derive(Debug)]
pub enum SDCardInitializationError {
    SpiInitializationError,
    SpiConfigurationError,
    NoResponse,
}

impl SDCard {
    pub fn new(mut spi: Spidev) -> Result<SDCard, SDCardInitializationError> {
        const   CMD0: [u8; 6] = [0x40, 0x00, 0x00, 0x00, 0x00, 0x95];
        const   CMD8: [u8; 6] = [0x48, 0x00, 0x00, 0x01, 0xAA, 0x87];
        const  CMD55: [u8; 6] = [0x77, 0x00, 0x00, 0x00, 0x00, 0x65];
        const ACMD41: [u8; 6] = [0x69, 0x40, 0x00, 0x00, 0x00, 0x77];
        const _CMD59: [u8; 6] = [0x7B, 0x00, 0x00, 0x00, 0x01, 0x83];

        // We write 80 clock cycles to the SD card to allow it to startup,
        // this is from the physical layer spec
        let blanks = [0xFF; 10];
        spi.write(&blanks).unwrap();
        let mut sd = SDCard { spi: spi };
        let mut response = [0; 256];

        sd.send_cmd(&CMD0, &mut response);
        if response[1] != 1 {
            panic!("Could not initialize SD Card");
        }

        sd.send_cmd(&CMD8, &mut response);
        if response[4] != 0x1 || response[5] != 0xAA {
            panic!("Voltage check or check reponse failed");
        }

        // CRC would be here...

        //SEND_OP_COND
        // We need to wait for the SD card to initialize, so we poll it every
        // 10 ms
        let mut response = [0; 8];
        for _ in 0..4 {
            use std::{thread, time};

            sd.send_cmd(&CMD55, &mut response);
            sd.send_cmd(&ACMD41, &mut response);

            if response[1] == 0 {
                break;
            }

            let ten_millis = time::Duration::from_millis(10);
            thread::sleep(ten_millis);
        }

        if response[1] != 0 {
            panic!("\n\nFailed ACMD41, got {:?}", response);
        }

        Ok(sd)
    }

    fn send_cmd(&mut self, cmd_bytes: &[u8], response: &mut [u8]) -> usize {
        let spacer = [0xFF; 1];
        self.spi.write(&spacer).expect("Could not write spacer");
        self.spi.write(cmd_bytes).expect(format!("Could not write command {:?}", cmd_bytes).as_str());

        self.spi.read(response).expect("Could not read response")
    }
}

impl BlockAccessor for SDCard {
    fn block_size(&self) -> u64 {
        512
    }

    fn read_block(&mut self, block_num: u64, block: &mut [u8]) {
        const DATA_START_BYTE: u8 = 0xFE;
        let mut data = [0; 700];
        let cmd = [
            0x51,
            ((block_num & (0xFF << 24)) >> 24) as u8,
            ((block_num & (0xFF << 16)) >> 16) as u8,
            ((block_num & (0xFF <<  8)) >>  8) as u8,
              block_num                        as u8,
            0xFF
        ];

        let spacer = [0xFF; 1];
        self.spi.write(&spacer).unwrap();
        self.spi.write(&cmd).unwrap();
        self.spi.read(&mut data).unwrap();

        let data_start = match data.iter().enumerate().find(|&(_, val)| *val == DATA_START_BYTE) {
            Some((index, _val)) => index + 1,
            None => panic!("Could not find data start byte")
        };

        for (data_index, block_index) in (data_start..(data_start+512)).zip(0..512) {
            *block.get_mut(block_index).unwrap() = data[data_index];
        }

        println!("");
    }

    fn write_block(&mut self, _block_num: u64, _block: &[u8]) -> Result<(), BlockAccessError> {
        unimplemented!();
    }
}

pub mod fat32 {
    use byte_util::{little_endian_to_int, take_from_slice, taken_from_slice};
    use block_accessor::{BlockAccessor};
    pub struct Fat32<N> where N: BlockAccessor {
        pub block_storage: N,
        pub physical_start_block: u32,
        pub boot_sector: BootSector
    }

    impl<N: BlockAccessor> Fat32<N> {
        pub fn new(mut block_storage: N, physical_start_block: u32) -> Fat32<N> {
            let mut block = [0; 512];

            println!("Reading block {}", physical_start_block);

            block_storage.read_block(physical_start_block as u64, &mut block);
            let boot_sector = BootSector::new(&block);

            Fat32 {
                block_storage: block_storage,
                physical_start_block: physical_start_block,
                boot_sector: boot_sector
            }
        }

        pub fn ls(&self, _path: &[&str]) -> Vec<String> {
            Vec::new()
        }
    }

    pub struct BootSector {
        pub jump_instruction: u32,
        pub oem_name: [u8; 8],
        pub bpb: EBPB,
        pub drive_number: u8
    }

    impl BootSector {
        pub fn new(bytes: &[u8]) -> BootSector {
            println!("Boot Sector Bytes In Boot Sector");
            for b in bytes.iter() {
                print!("{}, ", b);
            }
            println!("");

            if bytes.len() != 512 {
                panic!("Boot sector must be 512 bytes")
            }

            let jump_instruction: u32 = ((bytes[2] as u32) << 16) +
                                        ((bytes[1] as u32) <<  8) +
                                          bytes[0] as u32;
            let oem_name = [
                bytes[3],
                bytes[4],
                bytes[5],
                bytes[6],
                bytes[7],
                bytes[8],
                bytes[9],
                bytes[10],
            ];
            let ebpb = EBPB::new(&bytes[11..509]);
            let drive_number = bytes[509];
            if bytes[510] != 0x55 || bytes[511] != 0xAA {
                panic!("BPB check bytes not correct!");
            }

            BootSector {
                jump_instruction: jump_instruction,
                oem_name: oem_name,
                bpb: ebpb,
                drive_number: drive_number
            }
        }
    }

    pub struct EBPB {
            // DOS 2.0 BPB
            pub bytes_per_logical_sector: u16,
            pub sectors_per_cluster: u8,
            pub reserved_logical_sectors: u16,
            pub number_of_fats: u8,
            pub root_directory_entries: u16,
            pub total_logical_sectors: u16,
            pub media_descriptor: u8,

            // logical_sectors_per_fat ignored for Fat32
            // logical_sectors_per_fat;

            // DOS 3.31 BPB
            pub sectors_per_track: u16,
            pub heads_per_disk: u16,
            pub hidden_sectors: u32,
            pub sector_count: u32,

            // DOS 7.1 EBPB
            pub sectors_per_fat: u32,
            pub flags: [u8; 2],
            pub version: u16,
            pub root_directory_cluster: u32,
            pub information_sector: u16,
            pub backup_information_sector: u16,
            pub drive_number: u8,
            // Flags ignored for Fat32
            // more flags?
            pub boot_signature: u8,
            pub serial_number: u32,
            pub label: [u8; 11],
            pub file_system_type: [u8; 8],
    }

    fn get(bytes: &mut &[u8]) -> u8 {
        take_from_slice(bytes)
    }

    fn getn(bytes: &mut &[u8], n: usize) -> u32 {
        if n > 4 {
            panic!("Can't get more than four bytes into a u32");
        }
        little_endian_to_int(taken_from_slice(bytes, n))
    }

    impl EBPB {
        fn new(mut bytes: &[u8]) -> EBPB {
            assert_eq!(bytes.len(), 498);
            // DOS 2.0 BPB
            let bytes_per_logical_sector = getn(&mut bytes, 2) as u16;
            let sectors_per_cluster = get(&mut bytes);
            let reserved_logical_sectors = getn(&mut bytes, 2) as u16;
            let number_of_fats = get(&mut bytes);
            let root_directory_entries = getn(&mut bytes, 2) as u16;
            let total_logical_sectors = getn(&mut bytes, 2) as u16;
            let media_descriptor = get(&mut bytes);
            getn(&mut bytes, 2); // logical_sectors_per_fat ignored for Fat32

            assert_eq!(bytes.len(), 498-13);
            // DOS 3.31 BPB
            let sectors_per_track = getn(&mut bytes, 2) as u16;
            let heads_per_disk = getn(&mut bytes, 2) as u16;
            let hidden_sectors = getn(&mut bytes, 4);
            let sector_count = getn(&mut bytes, 4);

            assert_eq!(bytes.len(), 498-25);
            // DOS 7.1 EBPB
            let sectors_per_fat = getn(&mut bytes, 4);

            // Flags ignored for now (TODO)
            taken_from_slice(&mut bytes, 2);
            let flags = [0; 2];

            let version = getn(&mut bytes, 2) as u16;
            let root_directory_cluster = getn(&mut bytes, 4);
            let information_sector = getn(&mut bytes, 2) as u16;
            let backup_information_sector = getn(&mut bytes, 2) as u16;
            // Reserved section ignored
            taken_from_slice(&mut bytes, 12).to_owned();
            let drive_number = get(&mut bytes);
            // Flags ignored for Fat32
            get(&mut bytes);
            let boot_signature = get(&mut bytes);
            let serial_number = getn(&mut bytes, 4);

            // Label ignored for now (TODO)
            taken_from_slice(&mut bytes, 11);
            let label = [0; 11];

            // Actual file system type ignored for now (TODO)
            taken_from_slice(&mut bytes, 8).clone();
            let file_system_type: [u8; 8] = [0; 8];

            assert_eq!(boot_signature, 0x29);

            EBPB {
                bytes_per_logical_sector: bytes_per_logical_sector,
                sectors_per_cluster: sectors_per_cluster,
                reserved_logical_sectors: reserved_logical_sectors,
                number_of_fats: number_of_fats,
                root_directory_entries: root_directory_entries,
                total_logical_sectors: total_logical_sectors,
                media_descriptor: media_descriptor,
                sectors_per_track: sectors_per_track,
                heads_per_disk: heads_per_disk,
                hidden_sectors: hidden_sectors,
                sector_count: sector_count,
                sectors_per_fat: sectors_per_fat,
                flags: flags,
                version: version,
                root_directory_cluster: root_directory_cluster,
                information_sector: information_sector,
                backup_information_sector: backup_information_sector,
                drive_number: drive_number,
                boot_signature: boot_signature,
                serial_number: serial_number,
                label: label,
                file_system_type: file_system_type
            }
        }
    }

    pub enum Entry {
        DirectoryEntry(DirectoryEntry),
        Lfn(LfnEntry),
        Last,
        Empty,
    }

    impl Entry {
        pub fn new(bytes: &[u8]) -> Entry {
            if bytes.len() != 32 {
                panic!("Fat32 entry length must be 32 bytes!");
            }

            match bytes[0x00] {
                0x00 => Entry::Last,
                0xE5 => Entry::Empty,
                _ => match bytes[0x0B] {
                    0x0F => Entry::Lfn(LfnEntry::new(bytes)),
                    _ => Entry::DirectoryEntry(DirectoryEntry {})
                }
            }
        }
    }

    pub struct LfnEntry {
        pub file_name: [u16; 13]
    }

    impl LfnEntry {
        fn new(bytes: &[u8]) -> LfnEntry {
            assert!(bytes.len() == 32);
            assert_eq!(bytes[0x0B], 0x0F);

            let mut file_name = [0; 13];
            // Grab UCS-2 characters from three slices in the LFN entry
            for (filename_position, input_position) in
                [0x01, 0x03, 0x05, 0x07, 0x09,
                 0x0E, 0x10, 0x12, 0x14, 0x16, 0x18,
                 0x1C, 0x1E].iter().enumerate()
            {
                file_name[filename_position] = ((bytes[*input_position as usize] as u16) << 8) +
                                                (bytes[input_position+1 as usize] as u16);
            }

            LfnEntry {file_name: file_name}
        }
    }

    pub struct DirectoryEntry {

    }
}

pub mod mbr {
    use byte_util::{little_endian_to_int};

    #[derive(Debug)]
    pub struct MBR {
        pub partition_entries: [Option<PartitionEntry>; 4]
    }

    impl MBR {
        pub fn from_bytes(bytes: &[u8]) -> MBR {
            if bytes.len() != 512 {
                panic!("MBR must be 512 bytes");
            }

            const PARTITION_LOCATIONS: [u16; 4] = [0x1BE, 0x1CE, 0x1DE, 0x1EE];

            let mut partitions: [Option<PartitionEntry>; 4] = [None, None, None, None];
            for (partition_number, partition_location) in PARTITION_LOCATIONS.iter().enumerate() {
                let partition_location = *partition_location as usize;
                partitions[partition_number] =
                    PartitionEntry::from_bytes(&bytes[partition_location..partition_location+16]);
            }

            MBR { partition_entries: partitions }
        }

        pub fn partition_count(&self) -> u8 {
            self.partition_entries.iter().filter(|entry| entry.is_some()).count() as u8
        }
    }

    #[derive(Debug)]
    pub struct PartitionEntry {
        pub status: u8,
        pub first_sector_chs_address: u32,
        pub partition_type: u8,
        pub last_sector_chs_address: u32,
        pub first_sector_block_address: u32,
        pub sector_count: u32
    }

    impl PartitionEntry {
        pub fn from_bytes(bytes: &[u8]) -> Option<PartitionEntry> {
            if bytes.len() != 16 {
                panic!("Partition entry length must be 16");
            }

            // TODO: Make the rest of these little endian
            let status = bytes[0];

            let first_sector_chs_address = ((bytes[1] as u32) << 16) +
                                           ((bytes[2] as u32) <<  8) +
                                           bytes[3] as u32;
            let partition_type = bytes[4];
            if partition_type == 0 {
                println!("PartitionEntry is empty - partition type {}", partition_type);
                return None;
            }

            let last_sector_chs_address = ((bytes[5] as u32) << 16) +
                                          ((bytes[6] as u32) <<  8) +
                                            bytes[7] as u32;
            let first_sector_block_address = little_endian_to_int(&bytes[8..12]);
            let sector_count = ((bytes[12] as u32) << 24) +
                               ((bytes[13] as u32) << 16) +
                               ((bytes[14] as u32) <<  8) +
                               bytes[15] as u32;
            Some(
                PartitionEntry {
                    status: status,
                    first_sector_chs_address: first_sector_chs_address,
                    partition_type: partition_type,
                    last_sector_chs_address: last_sector_chs_address,
                    first_sector_block_address: first_sector_block_address,
                    sector_count: sector_count
                }
            )
        }
    }

}

pub mod byte_util {
    pub fn little_endian_to_int(bytes: &[u8]) -> u32 {
        let mut sum: u32 = 0;
        let mut shift_value = 0;
        for b in bytes.iter() {
            sum += (*b as u32) << shift_value;
            shift_value += 8
        }
        sum
    }

    pub fn take_from_slice<'a>(bytes: &mut &[u8]) -> u8 {
        let (a, b) = bytes.split_first().unwrap();
        *bytes = b;
        *a
    }

    pub fn taken_from_slice<'a>(bytes: &'a mut &[u8], midpoint: usize) -> &'a[u8] {
        let (a, b) = bytes.split_at(midpoint);
        *bytes = b;
        a
    }

}

#[cfg(test)]
mod tests {
    use block_accessor::{BlockAccessor};
    use file_block_accessor::BlockAccessFile;
    use spidev::{Spidev, SpidevOptions};

    use SDCard;
    use mbr::MBR;
    use fat32::Fat32;

    #[test]
    fn it_works() {
        let mut t = BlockAccessFile::new("../card-dump/sd.img").unwrap();
        let mut block = [0;512];
        t.read_block(0, &mut block);
        assert_eq!(block[510], 0x55);
        assert_eq!(block[511], 0xAA);
    }

    use std::io;
    use spidev::{SPI_MODE_0};

    fn create_spi() -> io::Result<Spidev> {
        let mut spi = try!(Spidev::open("/dev/spidev0.0"));
        let mut options = SpidevOptions::new();
        options
             .bits_per_word(8)
             .max_speed_hz(1_000_000)
             .mode(SPI_MODE_0)
             .build();
        try!(spi.configure(&options));
        Ok(spi)
    }


    #[test]
    #[ignore]
    fn sd_read() {
        let mut block = [0; 512];
        let spi = create_spi().expect("Could not get spi");
        let mut sd = SDCard::new(spi).unwrap();
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
        let spi = create_spi().expect("Could not get spi");
        let mut sd = SDCard::new(spi).unwrap();

        let mut block = [0; 512];
        sd.read_block(0, &mut block);

        let mbr = MBR::from_bytes(&block);
        let partition = mbr.partition_entries.get(0).unwrap().as_ref().unwrap();

        let mut fat32 = Fat32::new(sd, partition.first_sector_block_address);
        assert_eq!(fat32.ls(&[""]), vec!["projects".to_string(), "TEST_PAR.T1".to_string()]);
    }
}
