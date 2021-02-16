//! This is an implementation of the Mega Everdrive Pro's serial interface.
//!
//! It can be used to control the Mega Everdrive via a USB cable.
//!
//! It is a re-implementation of the original C# project:
//!   https://github.com/krikzz/MEGA-PRO
//!

use std::time::Duration;
use byteorder::{ByteOrder, BigEndian};
use serialport::SerialPort;
use anyhow::anyhow;
use log::{info, debug};

const PACKET_CMD: u8 = '+' as u8;

const ACK_BLOCK_SIZE: usize = 1024;
const MAX_ROM_SIZE: usize = 0xF80000;

const ADDR_ROM: u32 = 0x0000000;
const ADDR_SRAM: u32 = 0x1000000;
const ADDR_BRAM: u32 = 0x1080000;
const ADDR_CFG: u32 = 0x1800000;
const ADDR_SSR: u32 = 0x1802000;
const ADDR_FIFO: u32 = 0x1810000;

const SIZE_ROMX: u32 = 0x1000000;
const SIZE_SRAM: u32 = 0x80000;
const SIZE_BRAM: u32 = 0x80000;

const ADDR_FLA_MENU: u32 = 0x00000;
const ADDR_FLA_FPGA: u32 = 0x40000;
const ADDR_FLA_ICOR: u32 = 0x80000;

const FAT_READ: u8 = 0x01;
const FAT_WRITE: u8 = 0x02;
const FAT_OPEN_EXISTING: u8 = 0x00;
const FAT_CREATE_NEW: u8 = 0x04;
const FAT_CREATE_ALWAYS: u8 = 0x08;
const FAT_OPEN_ALWAYS: u8 = 0x10;
const FAT_OPEN_APPEND: u8 = 0x30;

const HOST_RST_OFF: u8 = 0;
const HOST_RST_SOFT: u8 = 1;
const HOST_RST_HARD: u8 = 2;

const CMD_STATUS: u8 = 0x10;
const CMD_GET_MODE: u8 = 0x11;
const CMD_IO_RST: u8 = 0x12;
const CMD_GET_VDC: u8 = 0x13;
const CMD_RTC_GET: u8 = 0x14;
const CMD_RTC_SET: u8 = 0x15;
const CMD_FLA_RD: u8 = 0x16;
const CMD_FLA_WR: u8 = 0x17;
const CMD_FLA_WR_SDC: u8 = 0x18;
const CMD_MEM_RD: u8 = 0x19;
const CMD_MEM_WR: u8 = 0x1A;
const CMD_MEM_SET: u8 = 0x1B;
const CMD_MEM_TST: u8 = 0x1C;
const CMD_MEM_CRC: u8 = 0x1D;
const CMD_FPG_USB: u8 = 0x1E;
const CMD_FPG_SDC: u8 = 0x1F;
const CMD_FPG_FLA: u8 = 0x20;
const CMD_FPG_CFG: u8 = 0x21;
const CMD_USB_WR: u8 = 0x22;
const CMD_FIFO_WR: u8 = 0x23;
const CMD_UART_WR: u8 = 0x24;
const CMD_REINIT: u8 = 0x25;
const CMD_SYS_INF: u8 = 0x26;
const CMD_GAME_CTR: u8 = 0x27;
const CMD_UPD_EXEC: u8 = 0x28;
const CMD_HOST_RST: u8 = 0x29;

const CMD_DISK_INIT: u8 = 0xC0;
const CMD_DISK_RD: u8 = 0xC1;
const CMD_DISK_WR: u8 = 0xC2;
const CMD_F_DIR_OPN: u8 = 0xC3;
const CMD_F_DIR_RD: u8 = 0xC4;
const CMD_F_DIR_LD: u8 = 0xC5;
const CMD_F_DIR_SIZE: u8 = 0xC6;
const CMD_F_DIR_PATH: u8 = 0xC7;
const CMD_F_DIR_GET: u8 = 0xC8;
const CMD_F_FOPN: u8 = 0xC9;
const CMD_F_FRD: u8 = 0xCA;
const CMD_F_FRD_MEM: u8 = 0xCB;
const CMD_F_FWR: u8 = 0xCC;
const CMD_F_FWR_MEM: u8 = 0xCD;
const CMD_F_FCLOSE: u8 = 0xCE;
const CMD_F_FPTR: u8 = 0xCF;
const CMD_F_FINFO: u8 = 0xD0;
const CMD_F_FCRC: u8 = 0xD1;
const CMD_F_DIR_MK: u8 = 0xD2;
const CMD_F_DEL: u8 = 0xD3;

const CMD_USB_RECOV: u8 = 0xF0;
const CMD_RUN_APP: u8 = 0xF1;

/// The operation mode of the Mega Everdrive Pro.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    /// Service / DFU mode.
    Service,
    /// Regular operation.
    App,
}

impl Mode {
    /// Get the lower-case name, used for debug printing.
    pub fn lower_name(self) -> &'static str {
        match self {
            Mode::Service => "service",
            Mode::App => "app",
        }
    }
}

/// A reset mode for the Mega Drive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetMode {
    /// Clear the reset flag (this is used to trigger reloads of code (I think)).
    Off,
    /// A soft reset: just go to the entry point.
    Soft,
    /// Reset the system entirely.
    Hard,
}

impl ResetMode {
    fn command(self) -> u8 {
        match self {
            ResetMode::Off => 0,
            ResetMode::Soft => 1,
            ResetMode::Hard => 2,
        }
    }
}

/// File metadata for files on the SD card.
pub struct FileMetadata {
    pub name: String,
    pub size: u32,
    pub date: u16,
    pub time: u16,
    pub attrib: u8,
}

/// Implement this trait to provide a source for the serial connection that
/// megalink uses. Since the link needs to be re-established after a connect,
/// picking a specific serial device is not always possible.
pub trait SerialFactory {
    fn open(&mut self) -> anyhow::Result<Box<dyn SerialPort>>;
}

/// The driver for the Mega Everdrive Pro serial interface.
pub struct EverdriveSerial<F> {
    factory: F,
    serial: Box<dyn SerialPort>,
}

impl<F: SerialFactory> EverdriveSerial<F> {
    fn open_serial(f: &mut F) -> anyhow::Result<Box<dyn SerialPort>> {
        let mut s = f.open()?;
        s.set_timeout(Duration::from_millis(100));

        let mut tmp = [0u8; 1024];
        loop {
            let n = match s.read(&mut tmp) {
                Ok(n) => n,
                Err(_) => break,
            };
            if n <= 0 {
                break;
            }
        }

        s.set_timeout(Duration::from_secs(1));
        Ok(s)
    }

    /// Create a new Mega Everdrive Pro controller.
    pub fn new(mut factory: F) -> anyhow::Result<EverdriveSerial<F>> {
        let serial = EverdriveSerial::open_serial(&mut factory)?;
        let mut s = EverdriveSerial {
            factory,
            serial,
        };

        // Do a status check early, so that if we get stuck (from an incorrect
        // device, or bad state), we get stuck early.
        s.get_status()?;

        debug!("initial status {}", s.get_status()?);
        Ok(s)
    }

    fn flush_cmd(&mut self) -> anyhow::Result<()> {
        debug!("flush cmd");
        self.serial.flush()?;
        // This _really_ should not be needed.... but it is.
        //tokio::time::sleep(Duration::from_millis(1)).await;
        Ok(())
    }

    fn tx_cmd(&mut self, cmd: u8) -> anyhow::Result<()> {
        debug!("tx cmd {:02x}", cmd);
        let data = [
            PACKET_CMD,
            !PACKET_CMD,
            cmd,
            !cmd];

        self.serial.write_all(&data)?;
        debug!("tx done");
        Ok(())
    }

    fn tx_u8(&mut self, v: u8) -> anyhow::Result<()> {
        let buf = [v];
        self.serial.write_all(&buf)?;
        Ok(())
    }

    fn tx_u16(&mut self, v: u16) -> anyhow::Result<()> {
        let mut buf = [0u8; 2];
        BigEndian::write_u16(&mut buf, v);
        self.serial.write_all(&buf)?;
        Ok(())
    }

    fn tx_u32(&mut self, v: u32) -> anyhow::Result<()> {
        let mut buf = [0u8; 4];
        BigEndian::write_u32(&mut buf, v);
        self.serial.write_all(&buf)?;
        Ok(())
    }

    fn tx_ack(&mut self, data: &[u8]) -> anyhow::Result<()> {
        for chunk in data.chunks(ACK_BLOCK_SIZE) {
            let resp = self.rx_u8()?;
            if resp != 0 {
                Err(anyhow!("error transferring data: {}", resp))?;
            }

            self.serial.write_all(chunk)?;
            self.flush_cmd()?;
        }

        Ok(())
    }

    fn tx_str(&mut self, s: &str) -> anyhow::Result<()> {
        self.tx_u16(s.len() as u16)?;
        self.serial.write_all(s.as_bytes())?;
        Ok(())
    }

    fn rx_u8(&mut self) -> anyhow::Result<u8> {
        debug!("rx 8");
        let mut v = [0u8; 1];
        self.serial.read_exact(&mut v)?;
        debug!("rx done");
        Ok(v[0])
    }

    fn rx_u16(&mut self) -> anyhow::Result<u16> {
        debug!("rx 16");
        let mut bytes = [0u8; 2];
        self.serial.read_exact(&mut bytes)?;
        debug!("rx done");
        Ok(BigEndian::read_u16(&bytes))
    }

    fn rx_u32(&mut self) -> anyhow::Result<u32> {
        debug!("rx 32");
        let mut bytes = [0u8; 4];
        self.serial.read_exact(&mut bytes)?;
        debug!("rx done");
        Ok(BigEndian::read_u32(&bytes))
    }

    fn rx_str(&mut self) -> anyhow::Result<String> {
        let len = self.rx_u16()? as usize;
        let mut bytes = vec![0u8; len];
        self.serial.read_exact(&mut bytes)?;
        Ok(String::from_utf8(bytes)?)
    }

    fn rx_file_metadata(&mut self) -> anyhow::Result<FileMetadata> {
        let size = self.rx_u32()?;
        let date = self.rx_u16()?;
        let time = self.rx_u16()?;
        let attrib = self.rx_u8()?;
        let name = self.rx_str()?;

        Ok(FileMetadata{
            name,
            size,
            date,
            time,
            attrib,
        })
    }

    /// Get the return code of the previous operation.
    ///
    /// This is cleared once read. 0 indicates success.
    pub fn get_status(&mut self) -> anyhow::Result<u8> {
        self.tx_cmd(CMD_STATUS)?;
        self.flush_cmd()?;
        let msg = self.rx_u16()?;

        if (msg & 0xff00) != 0xa500 {
            Err(anyhow!("invalid status response: {:04x}", msg))?;
        }

        Ok(msg as u8)
    }

    fn check_status(&mut self) -> anyhow::Result<()> {
        let res = self.get_status()?;
        if res != 0 {
            Err(anyhow!("unexpected status {}", res))?;
        }
        Ok(())
    }

    /// Get the current operating mode of the cartridge.
    pub fn get_mode(&mut self) -> anyhow::Result<Mode> {
        self.tx_cmd(CMD_GET_MODE)?;
        self.flush_cmd()?;

        let b = self.rx_u8()?;
        let mode = match b {
            0xa1 => Mode::Service,
            _ => Mode::App,
        };
        Ok(mode)
    }

    /// Change the current operating mode.
    pub fn set_mode(&mut self, target_mode: Mode) -> anyhow::Result<()> {
        let current_mode = self.get_mode()?;
        if current_mode == target_mode {
            return Ok(());
        }

        info!("changing to {} mode", target_mode.lower_name());

        match target_mode {
            Mode::Service => {
                self.tx_cmd(CMD_IO_RST)?;
                self.tx_u8(0)?;
            }
            Mode::App => {
                self.tx_cmd(CMD_RUN_APP)?;
            }
        }

        for _ in 0..100 {
            let serial = match EverdriveSerial::open_serial(&mut self.factory) {
                Ok(s) => s,
                Err(e) => {
                    debug!("error waiting for reset: {}", e);
                    continue;
                }
            };
            self.get_status()?;

            self.serial = serial;
            return Ok(());
        }

        Err(anyhow!("timeout reconnecting to device"))?
    }

    /// Reset the Mega Drive.
    pub fn reset_host(&mut self, mode: ResetMode) -> anyhow::Result<()> {
        self.tx_cmd(CMD_HOST_RST)?;
        self.tx_u8(mode.command())?;
        self.flush_cmd()?;
        Ok(())
    }

    /// Write to the Mega Drive's memory. This can be used to write to the ROM
    /// area with the Mega Everdrive.
    pub fn write_memory(&mut self, addr: u32, data: &[u8]) -> anyhow::Result<()> {
        if data.len() == 0 {
            return Ok(());
        }

        debug!("write {} to {:x}", data.len(), addr);

        self.tx_cmd(CMD_MEM_WR)?;
        self.tx_u32(addr)?;
        self.tx_u32(data.len() as u32)?;
        self.tx_u8(0)?;
        self.flush_cmd()?;

        self.serial.write_all(data)?;
        self.flush_cmd()?;
        Ok(())
    }

    /// Read from the Mega Drive's memory.
    pub fn read_memory(&mut self, addr: u32, data: &mut [u8]) -> anyhow::Result<()> {
        if data.len() == 0 {
            return Ok(());
        }

        self.tx_cmd(CMD_MEM_RD)?;
        self.tx_u32(addr)?;
        self.tx_u32(data.len() as u32)?;
        self.tx_u8(0)?;
        self.flush_cmd()?;

        self.serial.read_exact(data)?;
        Ok(())
    }

    /// Write to the FIFO used internally by the Mega Everdrive for communication
    /// with the IO co-processor.
    pub fn fifo_write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.write_memory(ADDR_FIFO, data)?;
        Ok(())
    }

    /// Write an integer to the FIFO.
    pub fn fifo_write_u16(&mut self, v: u16) -> anyhow::Result<()> {
        let mut buf = [0u8; 2];
        BigEndian::write_u16(&mut buf, v);
        self.fifo_write(&buf)?;
        Ok(())
    }

    /// Write an integer to the FIFO.
    pub fn fifo_write_u32(&mut self, v: u32) -> anyhow::Result<()> {
        let mut buf = [0u8; 4];
        BigEndian::write_u32(&mut buf, v);
        self.fifo_write(&buf)?;
        Ok(())
    }

    /// Write a string to the FIFO.
    pub fn fifo_write_str(&mut self, str: &str) -> anyhow::Result<()> {
        self.fifo_write_u16(str.len() as u16)?;
        self.fifo_write(str.as_bytes())?;
        Ok(())
    }

    /// Read some data from the FIFO.
    pub fn fifo_read(&mut self, data: &mut [u8]) -> anyhow::Result<()> {
        self.read_memory(ADDR_FIFO, data)?;
        Ok(())
    }

    /// Load and boot a game ROM.
    pub fn load_game(&mut self, name: &str, game: &[u8], skip_fpga: bool) -> anyhow::Result<()> {
        debug!("writing ROM: {} ({} bytes)", name, game.len());
        self.set_mode(Mode::App)?;
        self.reset_host(ResetMode::Soft)?;
        self.write_memory(ADDR_ROM, game)?;
        self.reset_host(ResetMode::Off)?;

        let resp = self.rx_u8()?;
        if resp != 'r' as u8 {
            Err(anyhow!("unexpected response: {}", resp))?;
        }

        debug!("testing");
        self.fifo_write("*t".as_bytes())?;
        self.flush_cmd()?;

        let resp = self.rx_u8()?;
        if resp != 'k' as u8 {
            Err(anyhow!("unexpected test response: {}", resp))?;
        }

        if skip_fpga {
            self.fifo_write("*u".as_bytes())?;
            self.flush_cmd()?;
        }

        debug!("setting game info");
        self.fifo_write("*g".as_bytes())?;
        self.fifo_write_u32(game.len() as u32)?;
        self.fifo_write_str(&format!("USB:{}", name))?;
        self.flush_cmd()?;

        // Clear response.
        self.rx_u8()?;
        Ok(())
    }

    /// Load an image into the FPGA from a slice.
    pub fn load_fpga_from_slice(&mut self, data: &[u8]) -> anyhow::Result<()> {
        debug!("loading FPGA image ({} bytes)", data.len());

        self.set_mode(Mode::App)?;
        self.reset_host(ResetMode::Soft)?;

        self.tx_cmd(CMD_FPG_USB)?;
        self.tx_u32(data.len() as u32)?;

        self.tx_ack(data)?;
        self.check_status()?;
        Ok(())
    }

    /// Load an image into the FPGA from flash storage.
    pub fn load_fpga_from_flash(&mut self, addr: u32) -> anyhow::Result<()> {
        debug!("loading FPGA image @ {:x}", addr);

        self.set_mode(Mode::App)?;
        self.reset_host(ResetMode::Soft)?;

        self.tx_cmd(CMD_FPG_FLA)?;
        self.tx_u32(addr)?;
        self.flush_cmd()?;
        self.check_status()?;
        Ok(())
    }

    /// Load an image into the FPGA from the SD card.
    pub fn load_fpga_from_sd(&mut self, path: &str) -> anyhow::Result<()> {
        debug!("loading FPGA from {}", path);

        self.set_mode(Mode::App)?;
        self.reset_host(ResetMode::Soft)?;

        let info = self.get_file_metadata(path)?;

        self.tx_cmd(CMD_FPG_SDC)?;
        self.tx_u32(info.size)?;
        self.tx_u8(0)?;
        self.flush_cmd()?;
        self.check_status()?;
        Ok(())
    }

    /// Fetch the metadata for a file on the SD card.
    pub fn get_file_metadata(&mut self, path: &str) -> anyhow::Result<FileMetadata> {
        self.tx_cmd(CMD_F_FINFO)?;
        self.tx_str(path)?;
        self.flush_cmd()?;

        let resp = self.rx_u8()?;
        if resp != 0 {
            Err(anyhow!("error fetching file metadata: {}", resp))?;
        }

        Ok(self.rx_file_metadata()?)
    }

    /// Set the current file handle, given a path on the SD card.
    pub fn open_file(&mut self, path: &str, mode: u8) -> anyhow::Result<()> {
        self.tx_cmd(CMD_F_FOPN)?;
        self.tx_u8(mode)?;
        self.tx_str(path)?;
        self.check_status()?;
        Ok(())
    }
}
