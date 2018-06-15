use byte_util::{little_endian_to_int};

#[derive(Debug)]
pub struct MBR {
    pub partition_entries: [Option<PartitionEntry>; 4]
}

impl MBR {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        const PARTITION_LOCATIONS: [u16; 4] = [0x1BE, 0x1CE, 0x1DE, 0x1EE];

        if bytes.len() != 512 {
            panic!("MBR must be 512 bytes");
        }

        let mut partitions: [Option<PartitionEntry>; 4] = [None, None, None, None];
        for (partition_number, partition_location) in PARTITION_LOCATIONS.iter().enumerate() {
            let partition_location = *partition_location as usize;
            partitions[partition_number] =
                PartitionEntry::from_bytes(&bytes[partition_location..partition_location+16]);
        }

        Self { partition_entries: partitions }
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
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 16 {
            panic!("Partition entry length must be 16");
        }

        // TODO: Make the rest of these little endian
        let status = bytes[0];

        let first_sector_chs_address = (u32::from(bytes[1]) << 16) +
                                       (u32::from(bytes[2]) <<  8) +
                                        u32::from(bytes[3]);
        let partition_type = bytes[4];
        if partition_type == 0 {
            return None;
        }

        let last_sector_chs_address = (u32::from(bytes[5]) << 16) +
                                      (u32::from(bytes[6]) <<  8) +
                                       u32::from(bytes[7]);
        let first_sector_block_address = little_endian_to_int(&bytes[8..12]);
        let sector_count = (u32::from(bytes[12]) << 24) +
                           (u32::from(bytes[13]) << 16) +
                           (u32::from(bytes[14]) <<  8) +
                            u32::from(bytes[15]);
        Some(
            Self {
                status,
                first_sector_chs_address,
                partition_type,
                last_sector_chs_address,
                first_sector_block_address,
                sector_count
            }
        )
    }
}
