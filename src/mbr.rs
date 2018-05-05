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
