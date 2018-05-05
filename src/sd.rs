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
        for b in &blanks {
            spi.send(*b).unwrap_or_else(|_| panic!("Could not send cmd"));
        }

        let mut sd = SDCard { spi };

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
            block[block_index] = data[data_index];
        }
    }

    fn write_block(&mut self, _block_num: u64, _block: &[u8]) -> Result<(), BlockAccessError> {
        unimplemented!();
    }
}
