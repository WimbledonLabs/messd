use hal::blocking::delay::DelayMs;
use hal::spi::FullDuplex;
use hal::digital::OutputPin;

use block_accessor::{BlockAccessor, BlockAccessError};

pub struct SDCard<SPI, CS>
    where SPI: FullDuplex<u8>,
           CS: OutputPin
{
    spi: SPI,
    output_pin: CS
}

#[derive(Debug)]
pub enum SDCardInitializationError {
    SpiInitializationError,
    SpiConfigurationError,
    NoResponse,
}


impl<SPI, CS> SDCard<SPI, CS>
    where SPI: FullDuplex<u8>,
           CS: OutputPin
{
    pub fn new(spi: SPI, mut delay: impl DelayMs<u8>, mut output_pin: CS) -> Result<Self, SDCardInitializationError> {
        const   CMD0: [u8; 6] = [0x40, 0x00, 0x00, 0x00, 0x00, 0x95];
        const   CMD8: [u8; 6] = [0x48, 0x00, 0x00, 0x01, 0xAA, 0x87];
        const  CMD55: [u8; 6] = [0x77, 0x00, 0x00, 0x00, 0x00, 0x65];
        const ACMD41: [u8; 6] = [0x69, 0x40, 0x00, 0x00, 0x00, 0x77];
        const _CMD59: [u8; 6] = [0x7B, 0x00, 0x00, 0x00, 0x01, 0x83];

        output_pin.set_low();
        output_pin.set_high();
        let mut sd = Self { spi, output_pin };

        // We write 80 clock cycles to the SD card to allow it to startup,
        // this is from the physical layer spec
        let blanks = [0xFF; 10];
        sd.spi_start_transfer();
        for b in &blanks {
            sd.spi_xfer(Some(b), None);
        }
        sd.spi_stop_transfer();

        let mut response = [0xFF; 16];
        sd.send_cmd(&CMD0, &mut response);

        let sd_init_correct = Self::response_byte(&response, 0).map_or(false, |b| b == 0x01);
        if !sd_init_correct {
            panic!("Could not initialize SD Card");
        }

        let mut response = [0xFF; 16];
        sd.send_cmd(&CMD8, &mut response);
        let voltage_level_accepted = Self::response_byte(&response, 3).map_or(false, |b| b == 0x01);
        let check_pattern_good = Self::response_byte(&response, 4).map_or(false, |b| b == 0xAA);
        if !voltage_level_accepted || !check_pattern_good {
            panic!("Voltage check or check reponse failed");
        }

        // CRC would be here...

        //SEND_OP_COND
        // We need to wait for the SD card to initialize, so we poll it every
        // 10 ms
        let mut response = [0xFF; 16];
        for _ in 0..4 {
            sd.send_cmd(&CMD55, &mut response);
            sd.send_cmd(&ACMD41, &mut response);

            let initialization_complete = Self::response_byte(&response, 0).map_or(false, |b| b == 0x00);
            if initialization_complete {
                return Ok(sd);
            }

            delay.delay_ms(10);
        }

        panic!("\n\nFailed ACMD41, got {:?}", response);
    }

    fn response_byte(response: &[u8], byte_num: usize) -> Option<u8> {
        let position = response.iter().position(|b| *b != 0xFF);
        match position {
            Some(p) => response.get(p + byte_num).map(|b| *b),
            None => None
        }
    }

    // TODO: replace this with cool RAII stuff
    fn spi_start_transfer(&mut self) {
        self.output_pin.set_low();
    }

    fn spi_stop_transfer(&mut self) {
        self.output_pin.set_high();
    }

    fn spi_xfer(&mut self, send_byte: Option<&u8>, receive_byte: Option<&mut u8>) {
        if let Some(send_byte) = send_byte {
            self.spi.send(*send_byte).unwrap_or_else(|_| panic!("Could not write cmd"));
        } else {
            self.spi.send(0xFF).unwrap_or_else(|_| panic!("Could not write cmd"));
        }

        if let Some(receive_byte) = receive_byte {
            *receive_byte = self.spi.read().unwrap_or_else(|_| panic!("Could not write command"));
        } else {
            self.spi.read().unwrap_or_else(|_| panic!("Could not write command"));
        }
    }

    fn send_cmd(&mut self, cmd: &[u8], response: &mut [u8]) {
        let spacer = 0xFF;
        self.spi_start_transfer();
        self.spi_xfer(Some(&spacer), None);
        self.spi_stop_transfer();

        // Custom zip_longest, since itertools doesn't support no_std...
        let mut cmd_iter = cmd.iter();
        let mut response_iter = response.iter_mut();
        self.spi_start_transfer();
        loop {
            let (send_byte, receive_byte) = (cmd_iter.next(), response_iter.next());
            if send_byte.is_none() && receive_byte.is_none() {
                break;
            }
            self.spi_xfer(send_byte, receive_byte);
        }
        self.spi_stop_transfer();
    }
}

impl<SPI, CS> BlockAccessor for SDCard<SPI, CS>
    where SPI: FullDuplex<u8>,
           CS: OutputPin
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

        let mut data = [0xFF; 800];
        self.send_cmd(&cmd, &mut data);

        let mut data_start_byte_position = None;

        for (idx, b) in data.iter().enumerate() {
            if *b == DATA_START_BYTE {
                data_start_byte_position = Some(idx);
                break;
            }
        }

        if let Some(position) = data_start_byte_position {
            let mut data_index = position+1;
            let mut block_index = 0;
            while block_index < 512 {
                block[block_index] = data[data_index];
                block_index += 1;
                data_index += 1;
            }
        } else {
            panic!("Could not find data start byte")
        }
    }

    fn write_block(&mut self, _block_num: u64, _block: &[u8]) -> Result<(), BlockAccessError> {
        unimplemented!();
    }
}
