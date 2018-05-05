extern crate block_accessor;
extern crate crc16;
extern crate heapless;
extern crate streaming_iterator;
extern crate embedded_hal;
extern crate linux_embedded_hal;
extern crate nb;

#[macro_use]
extern crate bitflags;

pub mod file_block_accessor;

use embedded_hal::blocking::delay::DelayMs;
use embedded_hal::spi::FullDuplex;

use block_accessor::{BlockAccessor, BlockAccessError};

pub struct SDCard<SPI>
    where SPI: FullDuplex<u8>
{
    spi: SPI
}

#[derive(Debug)]
pub enum SDCardInitializationError {
    SpiInitializationError,
    SpiConfigurationError,
    NoResponse,
}

impl<SPI> SDCard<SPI>
    where SPI: FullDuplex<u8>
{
    pub fn new(mut spi: SPI, mut delay: impl DelayMs<u8>) -> Result<SDCard<SPI>, SDCardInitializationError> {
        const   CMD0: [u8; 6] = [0x40, 0x00, 0x00, 0x00, 0x00, 0x95];
        const   CMD8: [u8; 6] = [0x48, 0x00, 0x00, 0x01, 0xAA, 0x87];
        const  CMD55: [u8; 6] = [0x77, 0x00, 0x00, 0x00, 0x00, 0x65];
        const ACMD41: [u8; 6] = [0x69, 0x40, 0x00, 0x00, 0x00, 0x77];
        const _CMD59: [u8; 6] = [0x7B, 0x00, 0x00, 0x00, 0x01, 0x83];

        // We write 80 clock cycles to the SD card to allow it to startup,
        // this is from the physical layer spec
        let blanks = [0xFF; 10];
        for b in blanks.iter() {
            spi.send(*b).unwrap_or_else(|_| panic!("Could not send cmd"));
        }

        let mut sd = SDCard { spi: spi };

        let mut response = [0xFF; 8];
        sd.send_cmd(&CMD0, &mut response);
        if response[1] != 1 {
            panic!("Could not initialize SD Card");
        }

        let mut response = [0xFF; 8];
        sd.send_cmd(&CMD8, &mut response);
        if response[4] != 0x1 || response[5] != 0xAA {
            panic!("Voltage check or check reponse failed");
        }

        // CRC would be here...

        //SEND_OP_COND
        // We need to wait for the SD card to initialize, so we poll it every
        // 10 ms
        let mut response = [0xFF; 8];
        for _ in 0..4 {
            sd.send_cmd(&CMD55, &mut response);
            sd.send_cmd(&ACMD41, &mut response);

            if response[1] == 0 {
                break;
            }

            delay.delay_ms(10);
        }

        if response[1] != 0 {
            panic!("\n\nFailed ACMD41, got {:?}", response);
        }

        Ok(sd)
    }

    fn send_cmd(&mut self, cmd: &[u8], response: &mut [u8]) {
        let spacer = 0xFF;
        self.spi.send(spacer).unwrap_or_else(|_| panic!("Could not write spacer"));

        for b in cmd.iter() {
            self.spi.send(*b).unwrap_or_else(|_| panic!("Could not write cmd"));
        }

        for b in response.iter_mut() {
            *b = self.spi.read().unwrap_or_else(|_| panic!("Could not write command"));
        }
    }
}

impl<SPI> BlockAccessor for SDCard<SPI>
    where SPI: FullDuplex<u8>
{
    fn block_size(&self) -> u64 {
        512
    }

    fn read_block(&mut self, block_num: u64, block: &mut [u8]) {
        const DATA_START_BYTE: u8 = 0xFE;
        let cmd = [
            0x51,
            ((block_num & (0xFF << 24)) >> 24) as u8,
            ((block_num & (0xFF << 16)) >> 16) as u8,
            ((block_num & (0xFF <<  8)) >>  8) as u8,
              block_num                        as u8,
            0xFF
        ];

        let mut data = [0xFF; 700];
        self.send_cmd(&cmd, &mut data);

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
    use heapless::{String};
    use heapless::consts::{U12, U13, U128, U512, U4096};
    use byte_util::{little_endian_to_int, take_from_slice, taken_from_slice};
    use block_accessor::{BlockAccessor};

    pub const BYTES_PER_BLOCK: u32 = 512;
    const BYTES_PER_CLUSTER_ENTRY: u64 = 4;
    const BYTES_PER_DIRECTORY_ENTRY: u32 = 32;

    pub struct Fat32<B> where B: BlockAccessor {
        pub block_storage: B,
        pub physical_start_block: u32,
        pub boot_sector: BootSector
    }

    impl<B: BlockAccessor> Fat32<B> {
        pub fn new(mut block_storage: B, physical_start_block: u32) -> Fat32<B> {
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

        /// Get data from the specified cluster, returning the number of bytes
        /// read.
        ///
        /// The length of `result` defines the maximum number of bytes that will
        /// be read, starting from the byte in the position `byte_offset`.
        ///
        /// Cluster numbers only start at 2. This will panic on cluster numbers
        /// 0 and 1.2
        ///
        /// `fat32.boot_sector.bpb.sectors_per_cluster*512` defines the maximum number
        /// of bytes that can  be returned from this function.
        pub fn get_cluster(&mut self, cluster_num: u32, byte_offset: usize, result: &mut [u8]) -> usize {
            assert!(cluster_num >= 2);
            // Just use fixed-sized vec for now. ith 8 sectors_per_cluster this
            // is the right size
            // TODO: this is terrible
            let mut out: Vec<u8, U4096> = Vec::new();

            let start_of_clusters_in_filesystem: u32 =
                self.physical_start_block +
                self.boot_sector.bpb.reserved_logical_sectors as u32 +
                self.boot_sector.bpb.sectors_per_fat as u32 * 2;

            let first_block_of_cluster =
                start_of_clusters_in_filesystem +
                self.boot_sector.bpb.sectors_per_cluster as u32 *(cluster_num-2);

            for block_num in first_block_of_cluster..(first_block_of_cluster +
                    self.boot_sector.bpb.sectors_per_cluster as u32)
            {
                let mut block = [0; 512];
                self.block_storage.read_block(block_num as u64, &mut block);
                out.extend(block.iter());
            }

            let mut count = 0;
            for (position, byte) in out.iter()
                                       .skip(byte_offset)
                                       .take(result.len())
                                       .enumerate()
            {
                count += 1;
                result[position] = *byte;
            }

            count
        }

        /// Get the next cluster number from the file allocation table.
        /// Returns None when the provided cluster number is the last cluster in
        /// the chain.
        pub fn cluster_number_after(&mut self, cluster_num: u32) -> Option<u32> {
            assert!(cluster_num >= 2);

            let file_allocation_table_start_block: u64 =
                self.physical_start_block as u64 +
                self.boot_sector.bpb.reserved_logical_sectors as u64;

            let block_num_for_cluster: u64 =
                file_allocation_table_start_block +
                (cluster_num as u64 * BYTES_PER_CLUSTER_ENTRY) / BYTES_PER_BLOCK as u64;

            let mut block = [0; 512];
            self.block_storage.read_block(block_num_for_cluster, &mut block);

            let cluster_entry_offset: usize =
                (cluster_num as usize * BYTES_PER_CLUSTER_ENTRY as usize) % 512 as usize;

            let next_cluster = little_endian_to_int(
                &block[cluster_entry_offset..cluster_entry_offset+4]);

            if next_cluster == 0 {
                None
            } else {
                Some(next_cluster)
            }
        }

        pub fn print_root_dir(&mut self) {
            self.ls_cluster(2);
        }

        pub fn ls_cluster(&mut self, cluster_num: u32) {

            let mut block = [0; 512];

            // The root directory is the first cluster, the first cluster is
            // cluster 2
            self.get_cluster(cluster_num, 0, &mut block);

            for entry_bytes in block.chunks(BYTES_PER_DIRECTORY_ENTRY as usize) {
                let entry = Entry::new(entry_bytes);
                match entry {
                    Entry::Lfn(e) => {
                        for ucs_code_point in e.file_name.iter() {
                            if (ucs_code_point >> 8) > 0x7Fu16 {
                                break;
                            }
                            print!("{}", (ucs_code_point >> 8) as u8 as char);
                        }
                        print!("\n")
                    },
                    Entry::DirectoryEntry(e) => {
                        println!("{:#?}", e);
                        //unsafe {
                        //    println!("{:?}.{:?}",
                        //             str::from_utf8_unchecked(&e.file_name_bytes),
                        //             str::from_utf8_unchecked(&e.file_extension_bytes));
                        //}
                     },
                    Entry::Empty => println!("!placeholder!"),
                    Entry::Last => println!("!placeholder!")
                }
            }
        }

        pub fn iter_contents_of_directory_cluster<'a>(&'a mut self, cluster_num: u32) -> DirectoryIterator<'a, B> {
            DirectoryIterator::new(self, cluster_num)
        }

        /// Undefined behaviour when the size of block doesn't evenly divide
        /// a cluster
        pub fn iter_file<'a, 'b>(&'a mut self, file: File) -> FileIterator<B> {
            FileIterator {
                fat32: self,
                cluster: Some(file.cluster),
                bytes_read: 0,
                file_size: file.size,
            }
        }

        pub fn item_info(&mut self, path: &str) -> Option<DirectoryItem> {
            // Start at the root directory
            let mut current_cluster = 2;

            if path.ends_with("/") {
                // Files don't end with '/'
                return None
            }

            // TODO don't make two iterators
            let path_length = path.split('/').count();
            let path_iter = path.split('/');

            println!("Starting iteration with path length {:?}", path_length);
            'iter_part: for (part_num, part) in path_iter.enumerate() {
                println!("Starting part {:?}", part);
                if part.len() == 0 {
                    continue;
                }

                for item in self.iter_contents_of_directory_cluster(current_cluster) {
                    println!("Checking item {:?}", item);
                    match item {
                        DirectoryItem::Directory(d) => {
                            println!("Got directory {:?}", d);
                            if d.name == part {
                                if part_num+1 == path_length {
                                    return Some(DirectoryItem::Directory(d));
                                }
                                current_cluster = d.cluster;
                                continue 'iter_part;
                            } else {
                                println!("part {:?} does d.name {:?}", part, d.name);
                            }
                        },
                        DirectoryItem::File(f) => {
                            println!("Got file {:?}", f);
                            if f.name == part && part_num+1 == path_length {
                                // This is the last part, and should represent
                                // the file
                                return Some(DirectoryItem::File(f));
                            }
                        }
                    }
                }

                // Could not find the next item in the path, so we fail
                break;
            }
            println!("Done iteration");

            None
        }
    }

    pub struct FileIterator<'a, B: 'a>
        where B: BlockAccessor,
    {
        fat32: &'a mut Fat32<B>,
        cluster: Option<u32>,
        bytes_read: u32,
        file_size: u32,
    }

    use heapless::Vec;

    // Replace U512 with a generic ArrayLength type when GAT's are implemented
    // in rust
    impl<'a, B> Iterator for FileIterator<'a, B>
        where B: BlockAccessor
    {
        type Item = Vec<u8, U512>;

        fn next(&mut self) -> Option<Self::Item> {
            let bytes_per_cluster: u32 =
                self.fat32.boot_sector.bpb.sectors_per_cluster as u32 * BYTES_PER_BLOCK as u32;

            match self.cluster {
                None => None,
                Some(cluster_num) => {
                    let mut block = Vec::new();

                    if self.file_size == self.bytes_read {
                        return None;
                    }

                    let bytes_left = self.file_size - self.bytes_read;
                    let bytes_to_read = usize::min(bytes_left as usize, block.capacity());

                    // Shouldn't fail since it's resized with its own capacity
                    block.resize_default(bytes_to_read).unwrap();

                    let bytes_read = self.fat32.get_cluster(
                        cluster_num,
                        (self.bytes_read % (bytes_per_cluster)) as usize,
                        &mut block[0..bytes_to_read]) as u32;

                    self.bytes_read += bytes_read;

                    if self.bytes_read % bytes_per_cluster == 0 {
                        self.cluster = self.fat32.cluster_number_after(cluster_num);
                    }

                    Some(block)
                }
            }
        }
    }

    pub struct FileInfo { }

    pub struct DirectoryIterator<'a, B: 'a>
        where B: BlockAccessor
    {
        fat32: &'a mut Fat32<B>,
        cluster: u32,
        entry_in_cluster: u32
    }

    impl<'a, B: BlockAccessor> DirectoryIterator<'a, B> {
        fn new(fat32: &'a mut Fat32<B>, cluster: u32) -> 
            DirectoryIterator<'a, B>
        {
            DirectoryIterator {
                fat32: fat32,
                cluster: cluster,
                entry_in_cluster: 0
            }
        }
    }

    #[derive(Debug)]
    pub enum DirectoryItem {
        File(File),
        Directory(Directory)
    }

    #[derive(Debug)]
    pub struct File {
        pub name: String<U128>,
        pub cluster: u32,
        pub size: u32
        // Maybe flags later?
    }

    #[derive(Debug)]
    pub struct Directory {
        pub name: String<U128>,
        pub cluster: u32,
        // Maybe flags later?
    }

    impl<'a, B: BlockAccessor> Iterator for DirectoryIterator<'a, B> {
        type Item = DirectoryItem;

        fn next(&mut self) -> Option<DirectoryItem> {
            // TODO this will break if a directory is more than 1 cluster
            let mut item_name: String<U128> = String::new();

            loop {
                let cluster_offset: usize =
                    (self.entry_in_cluster * BYTES_PER_DIRECTORY_ENTRY) as usize;
                self.entry_in_cluster += 1;

                let mut entry_bytes = [0; 32];
                self.fat32.get_cluster(self.cluster, cluster_offset, &mut entry_bytes);

                match Entry::new(&entry_bytes) {
                    Entry::Lfn(e) => {
                        // TODO make this more efficient and less ugly
                        let mut new_item_name = String::new();
                        new_item_name.push_str(&e.name()).unwrap();

                        new_item_name.push_str(&item_name).unwrap();

                        item_name = new_item_name;
                    },
                    Entry::DirectoryEntry(e) => {
                        if item_name.len() == 0 {
                            item_name.push_str(&e.name()).unwrap();
                        }

                        if e.flags.contains(DirectoryEntryFlags::SUBDIRECTORY) {
                            return Some(DirectoryItem::Directory(
                                Directory {
                                    name: item_name,
                                    cluster: e.cluster_num
                                }
                            ));
                        } else {
                            return Some(DirectoryItem::File(
                                File {
                                    name: item_name,
                                    cluster: e.cluster_num,
                                    size: e.size
                                }
                            ));
                        }
                    },
                    Entry::Empty => item_name.clear(),
                    Entry::Last => return None
                }
            }
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
            taken_from_slice(&mut bytes, 12);
            let drive_number = get(&mut bytes);
            // Flags ignored for Fat32
            get(&mut bytes);
            let boot_signature = get(&mut bytes);
            let serial_number = getn(&mut bytes, 4);

            // Label ignored for now (TODO)
            taken_from_slice(&mut bytes, 11);
            let label = [0; 11];

            // Actual file system type ignored for now (TODO)
            taken_from_slice(&mut bytes, 8);
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

    #[derive(Debug)]
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
                    _ => Entry::DirectoryEntry(DirectoryEntry::new(bytes))
                }
            }
        }
    }

    #[derive(Debug)]
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

        fn name(&self) -> String<U13> {
            let mut name = String::new();

            for codepoint in self.file_name.iter() {
                let ch = (codepoint >> 8) as u8;

                if ch == 0 {
                    break;
                }

                if ch > 0x7F {
                    // TODO don't skip non-ascii characters
                    continue;
                }

                match name.push(ch as char) {
                    Err(_) => break,
                    _ => ()
                }
            }

            name
        }
    }

    bitflags! {
        pub struct DirectoryEntryFlags: u8 {
            const READ_ONLY    = 0b0000_0001;
            const HIDDEN       = 0b0000_0010;
            const SYSTEM       = 0b0000_0100;
            const VOLUME_LABEL = 0b0000_1000;
            const SUBDIRECTORY = 0b0001_0000;
            const ARCHIVE      = 0b0010_0000;
            const DEVICE       = 0b0100_0000;
            const RESERVED     = 0b1000_0000;
        }
    }

    #[derive(Debug)]
    pub struct DirectoryEntry {
        pub file_name_bytes: [u8; 8],
        pub file_extension_bytes: [u8; 3],
        pub flags: DirectoryEntryFlags,
        pub cluster_num: u32,
        pub size: u32
    }

    impl DirectoryEntry {
        fn new(bytes: &[u8]) -> DirectoryEntry {
            assert!(bytes.len() == 32);

            let mut file_name_bytes = [0; 8];
            for (idx, byte) in bytes[0..8].iter().enumerate() {
                file_name_bytes[idx] = *byte;
            }

            let mut file_extension_bytes = [0; 3];
            for (idx, byte) in bytes[0x08..0x0B].iter().enumerate() {
                file_extension_bytes[idx] = *byte;
            }

            let flags = DirectoryEntryFlags { bits: bytes[0x0B] };

            let low_cluster_num  = little_endian_to_int(&bytes[0x1A..0x1C]);
            let high_cluster_num = little_endian_to_int(&bytes[0x14..0x16]);

            let cluster_num = (high_cluster_num << 16) + low_cluster_num;
            let size = little_endian_to_int(&bytes[0x1C..0x20]);

            DirectoryEntry {
                file_name_bytes: file_name_bytes,
                file_extension_bytes: file_extension_bytes,
                flags: flags,
                cluster_num: cluster_num,
                size: size
            }
        }

        fn name(&self) -> String<U12> {
            let mut name = String::new();

            for ch in self.file_name_bytes.iter() {
                name.push(*ch as char).unwrap();
            }

            name.push('.').unwrap();

            for ch in self.file_extension_bytes.iter() {
                name.push(*ch as char).unwrap();
            }

            name
        }
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
    use linux_embedded_hal::spidev::{Spidev, SpidevOptions, SPI_MODE_0};

    use SDCard;
    use mbr::MBR;
    use fat32::{Fat32, DirectoryItem};

    use std::fs::File;
    use std::io::prelude::*;

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
