use std::time::Duration;
use futures::{future, FutureExt};
use tokio::io::{AsyncWriteExt, AsyncReadExt};
use tokio_serial::Serial;
use byteorder::{ByteOrder, BigEndian};
use async_trait::async_trait;
use anyhow::anyhow;
use log::{info, debug};

const PACKET_CMD: u8 = '+' as u8;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Service,
    App,
}

impl Mode {
    pub fn lower_name(self) -> &'static str {
        match self {
            Mode::Service => "service",
            Mode::App => "app",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetMode {
    Off,
    Soft,
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

#[async_trait]
pub trait SerialFactory {
    async fn open(&mut self) -> anyhow::Result<Serial>;
}

pub struct EverdriveSerial<F> {
    factory: F,
    serial: Serial,
}

impl<F: SerialFactory> EverdriveSerial<F> {
    async fn open_serial(f: &mut F) -> anyhow::Result<Serial> {
        let mut s = f.open().await?;
        let mut timeout = tokio::time::sleep(Duration::from_millis(100)).boxed();

        let mut tmp = [0u8; 1024];
        loop {
            let mut read = s.read(&mut tmp).boxed();

            let n = match future::select(&mut timeout, &mut read).await {
                future::Either::Right((n, _)) => n?,
                future::Either::Left((_, _)) => break,
            };

            if n <= 0 {
                break;
            }
        }

        Ok(s)
    }

    pub async fn new(mut factory: F) -> anyhow::Result<EverdriveSerial<F>> {
        let serial = EverdriveSerial::open_serial(&mut factory).await?;
        let mut s = EverdriveSerial {
            factory,
            serial,
        };

        // Do a status check early, so that if we get stuck (from an incorrect
        // device, or bad state), we get stuck early.
        s.get_status().await?;

        Ok(s)
    }

    async fn flush_cmd(&mut self) -> anyhow::Result<()> {
        debug!("flush cmd");
        self.serial.flush().await?;
        // This _really_ should not be needed.... but it is.
        tokio::time::sleep(Duration::from_millis(1)).await;
        Ok(())
    }

    async fn tx_cmd(&mut self, cmd: u8) -> anyhow::Result<()> {
        debug!("tx cmd {:02x}", cmd);
        let data = [
            PACKET_CMD,
            !PACKET_CMD,
            cmd,
            !cmd];

        self.serial.write_all(&data).await?;
        debug!("tx done");
        Ok(())
    }

    async fn tx_u8(&mut self, v: u8) -> anyhow::Result<()> {
        let buf = [v];
        self.serial.write_all(&buf).await?;
        Ok(())
    }

    async fn tx_u32(&mut self, v: u32) -> anyhow::Result<()> {
        let mut buf = [0u8; 4];
        BigEndian::write_u32(&mut buf, v);
        self.serial.write_all(&buf).await?;
        Ok(())
    }

    async fn rx_u8(&mut self) -> anyhow::Result<u8> {
        debug!("rx 8");
        let mut v = [0u8; 1];
        self.serial.read_exact(&mut v).await?;
        debug!("rx done");
        Ok(v[0])
    }

    async fn rx_u16(&mut self) -> anyhow::Result<u16> {
        debug!("rx 16");
        let mut bytes = [0u8; 2];
        self.serial.read_exact(&mut bytes).await?;
        debug!("rx done");
        Ok(BigEndian::read_u16(&bytes))
    }

    async fn rx_str(&mut self) -> anyhow::Result<String> {
        let len = self.rx_u16().await? as usize;
        let mut bytes = vec![0u8; len];
        self.serial.read_exact(&mut bytes).await?;
        Ok(String::from_utf8(bytes)?)
    }

    async fn get_status(&mut self) -> anyhow::Result<u8> {
        self.tx_cmd(CMD_STATUS).await?;
        self.flush_cmd().await?;
        let msg = self.rx_u16().await?;

        if (msg & 0xff00) != 0xa500 {
            Err(anyhow!("invalid status response: {:04x}", msg))?;
        }

        Ok(msg as u8)
    }

    pub async fn get_mode(&mut self) -> anyhow::Result<Mode> {
        self.tx_cmd(CMD_GET_MODE).await?;
        self.flush_cmd().await?;

        let b = self.rx_u8().await?;
        let mode = match b {
            0xa1 => Mode::Service,
            _ => Mode::App,
        };
        Ok(mode)
    }

    pub async fn set_mode(&mut self, target_mode: Mode) -> anyhow::Result<()> {
        let current_mode = self.get_mode().await?;
        if current_mode == target_mode {
            return Ok(());
        }

        info!("changing to {} mode", target_mode.lower_name());

        match target_mode {
            Mode::Service => {
                self.tx_cmd(CMD_IO_RST).await?;
                self.tx_u8(0).await?;
            }
            Mode::App => {
                self.tx_cmd(CMD_RUN_APP).await?;
            }
        }

        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(100)).await;

            let serial = match EverdriveSerial::open_serial(&mut self.factory).await {
                Ok(s) => s,
                Err(e) => {
                    debug!("error waiting for reset: {}", e);
                    continue;
                }
            };
            self.get_status().await?;

            self.serial = serial;
            return Ok(());
        }

        Err(anyhow!("timeout reconnecting to device"))?
    }

    pub async fn reset_host(&mut self, mode: ResetMode) -> anyhow::Result<()> {
        self.tx_cmd(CMD_HOST_RST).await?;
        self.tx_u8(mode.command()).await?;
        self.flush_cmd().await?;
        Ok(())
    }

    pub async fn write_memory(&mut self, addr: u32, data: &[u8]) -> anyhow::Result<()> {
        if data.len() == 0 {
            return Ok(());
        }

        self.tx_cmd(CMD_MEM_WR).await?;
        self.tx_u32(addr).await?;
        self.tx_u32(data.len() as u32).await?;
        self.tx_u8(0).await?;
        self.flush_cmd().await?;

        for chunk in data.chunks(8192) {
            self.serial.write_all(chunk).await?;
            self.flush_cmd().await?;
        }

        Ok(())
    }

    pub async fn read_memory(&mut self, addr: u32, data: &mut [u8]) -> anyhow::Result<()> {
        if data.len() == 0 {
            return Ok(());
        }

        self.tx_cmd(CMD_MEM_RD).await?;
        self.tx_u32(addr).await?;
        self.tx_u32(data.len() as u32).await?;
        self.tx_u8(0).await?;
        self.flush_cmd().await?;

        self.serial.read_exact(data).await?;
        Ok(())
    }

    pub async fn fifo_write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        debug!("FIFO WRITE ({}) {:?}", data.len(), data);
        self.write_memory(ADDR_FIFO, data).await?;
        Ok(())
    }

    pub async fn fifo_write_u16(&mut self, v: u16) -> anyhow::Result<()> {
        let mut buf = [0u8; 2];
        BigEndian::write_u16(&mut buf, v);
        self.fifo_write(&buf).await?;
        Ok(())
    }

    pub async fn fifo_write_u32(&mut self, v: u32) -> anyhow::Result<()> {
        let mut buf = [0u8; 4];
        BigEndian::write_u32(&mut buf, v);
        self.fifo_write(&buf).await?;
        Ok(())
    }

    pub async fn fifo_write_str(&mut self, str: &str) -> anyhow::Result<()> {
        self.fifo_write_u16(str.len() as u16).await?;
        self.fifo_write(str.as_bytes()).await?;
        Ok(())
    }

    pub async fn fifo_read(&mut self, data: &mut [u8]) -> anyhow::Result<()> {
        self.read_memory(ADDR_FIFO, data).await?;
        Ok(())
    }

    pub async fn load_game(&mut self, name: &str, game: &[u8], skip_fpga: bool) -> anyhow::Result<()> {
        debug!("writing ROM: {} ({} bytes)", name, game.len());
        self.set_mode(Mode::App).await?;
        self.reset_host(ResetMode::Soft).await?;
        self.write_memory(ADDR_ROM, game).await?;
        self.reset_host(ResetMode::Off).await?;

        tokio::time::sleep(Duration::from_millis(500)).await;

        let resp = self.rx_u8().await?;
        if resp != 'r' as u8 {
            Err(anyhow!("unexpected response: {}", resp))?;
        }

        debug!("testing");
        self.fifo_write("*t".as_bytes()).await?;
        self.flush_cmd().await?;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let resp = self.rx_u8().await?;
        debug!("AX {}", resp);
        if resp != 'k' as u8 {
            Err(anyhow!("unexpected test response: {}", resp))?;
        }

        if skip_fpga {
            self.fifo_write("*u".as_bytes()).await?;
            self.flush_cmd().await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        debug!("setting game info");
        self.fifo_write("*g".as_bytes()).await?;
        self.fifo_write_u32(game.len() as u32).await?;
        self.fifo_write_str(&format!("USB:{}", name)).await?;
        self.flush_cmd().await?;
        self.reset_host(ResetMode::Off).await?;
        Ok(())
    }
}
