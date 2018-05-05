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

        block_storage.read_block(u64::from(physical_start_block), &mut block);
        let boot_sector = BootSector::new(&block);

        Fat32 {
            block_storage,
            physical_start_block,
            boot_sector
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
            u32::from(self.boot_sector.bpb.reserved_logical_sectors) +
            self.boot_sector.bpb.sectors_per_fat * 2;

        let first_block_of_cluster =
            start_of_clusters_in_filesystem +
            u32::from(self.boot_sector.bpb.sectors_per_cluster) * (cluster_num-2);

        for block_num in first_block_of_cluster..(first_block_of_cluster +
                u32::from(self.boot_sector.bpb.sectors_per_cluster))
        {
            let mut block = [0; 512];
            self.block_storage.read_block(u64::from(block_num), &mut block);
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
            u64::from(self.physical_start_block) +
            u64::from(self.boot_sector.bpb.reserved_logical_sectors);

        let block_num_for_cluster: u64 =
            file_allocation_table_start_block +
            (u64::from(cluster_num) * BYTES_PER_CLUSTER_ENTRY) / u64::from(BYTES_PER_BLOCK);

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

    pub fn iter_contents_of_directory_cluster(&mut self, cluster_num: u32) -> DirectoryIterator<B> {
        DirectoryIterator::new(self, cluster_num)
    }

    /// Undefined behaviour when the size of block doesn't evenly divide
    /// a cluster
    pub fn iter_file<'a>(&'a mut self, file: &File) -> FileIterator<B> {
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

        if path.ends_with('/') {
            // Files don't end with '/'
            return None
        }

        // TODO don't make two iterators
        let path_length = path.split('/').count();
        let path_iter = path.split('/');

        'iter_part: for (part_num, part) in path_iter.enumerate() {
            if part.is_empty() {
                continue;
            }

            for item in self.iter_contents_of_directory_cluster(current_cluster) {
                match item {
                    DirectoryItem::Directory(d) => {
                        if d.name == part {
                            if part_num+1 == path_length {
                                return Some(DirectoryItem::Directory(d));
                            }
                            current_cluster = d.cluster;
                            continue 'iter_part;
                        }
                    },
                    DirectoryItem::File(f) => {
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
            u32::from(self.fat32.boot_sector.bpb.sectors_per_cluster) * BYTES_PER_BLOCK;

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
            fat32,
            cluster,
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
                    if item_name.is_empty() {
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
        if bytes.len() != 512 {
            panic!("Boot sector must be 512 bytes")
        }

        let jump_instruction: u32 = (u32::from(bytes[2]) << 16) +
                                    (u32::from(bytes[1]) <<  8) +
                                     u32::from(bytes[0]);
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
        let bpb = EBPB::new(&bytes[11..509]);
        let drive_number = bytes[509];
        if bytes[510] != 0x55 || bytes[511] != 0xAA {
            panic!("BPB check bytes not correct!");
        }

        BootSector {
            jump_instruction,
            oem_name,
            bpb,
            drive_number
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
            bytes_per_logical_sector,
            sectors_per_cluster,
            reserved_logical_sectors,
            number_of_fats,
            root_directory_entries,
            total_logical_sectors,
            media_descriptor,
            sectors_per_track,
            heads_per_disk,
            hidden_sectors,
            sector_count,
            sectors_per_fat,
            flags,
            version,
            root_directory_cluster,
            information_sector,
            backup_information_sector,
            drive_number,
            boot_signature,
            serial_number,
            label,
            file_system_type
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
            file_name[filename_position] = (u16::from(bytes[*input_position as usize]) << 8) +
                                            u16::from(bytes[input_position+1 as usize]);
        }

        LfnEntry {file_name}
    }

    fn name(&self) -> String<U13> {
        let mut name = String::new();

        for codepoint in &self.file_name {
            let ch = (codepoint >> 8) as u8;

            if ch == 0 {
                break;
            }

            if ch > 0x7F {
                // TODO don't skip non-ascii characters
                continue;
            }

            if name.push(ch as char).is_err() {
                break;
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
            file_name_bytes,
            file_extension_bytes,
            flags,
            cluster_num,
            size
        }
    }

    fn name(&self) -> String<U12> {
        let mut name = String::new();

        for ch in &self.file_name_bytes {
            name.push(*ch as char).unwrap();
        }

        name.push('.').unwrap();

        for ch in &self.file_extension_bytes {
            name.push(*ch as char).unwrap();
        }

        name
    }
}
