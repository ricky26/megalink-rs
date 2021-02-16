use std::path::PathBuf;
use clap::Clap;
use log::{info, warn};
use anyhow::anyhow;
use megalink_rs::{EverdriveSerial, Mode, SerialFactory, ResetMode};
use serialport::SerialPort;

#[derive(Clap)]
struct Opts {
    #[clap(short, long)]
    serial_port: Option<String>,

    #[clap(subcommand)]
    command: Command,
}

#[derive(Clap)]
enum Command {
    SetMode(CmdSetMode),
    Reset(CmdReset),
    Recover(CmdRecover),
    Run(CmdRunGame),
    LoadFPGA(CmdLoadFPGA),
}

#[derive(Clap)]
struct CmdSetMode {
    mode: String,
}

#[derive(Clap)]
struct CmdReset {
    #[clap(short, long)]
    hard: bool,
}

#[derive(Clap)]
struct CmdRecover;

#[derive(Clap)]
struct CmdRunGame {
    path: PathBuf,

    #[clap(short, long)]
    skip_fpga: bool,

    #[clap(short, long)]
    fpga: Option<PathBuf>,
}

#[derive(Clap)]
struct CmdLoadFPGA {
    path: Option<PathBuf>,

    #[clap(short, long)]
    sd: Option<String>,

    #[clap(short, long)]
    flash: Option<u32>,
}

struct Factory {
    port_name: Option<String>,
    first: bool,
}

impl SerialFactory for Factory {
    fn open(&mut self) -> anyhow::Result<Box<dyn SerialPort>> {
        let first = self.first;
        self.first = false;

        let serial_port_path = self.port_name.clone().map_or_else(|| {
            let ports = serialport::available_ports()?;
            if ports.len() == 1 {
                Ok(ports.into_iter().next().unwrap().port_name)
            } else {
                let prefix = if ports.len() == 0 { "no" } else { "multiple" };

                if first {
                    warn!("{} serial ports available, pick one with --serial-port=PATH.", prefix);

                    if ports.len() > 0 {
                        warn!("available serial ports:");
                        for port in serialport::available_ports()? {
                            warn!(" {}", port.port_name);
                        }
                    }
                }

                Err(anyhow!("unable to select serial port"))
            }
        }, Ok)?;

        info!("using serial port {}", &serial_port_path);
        Ok(serialport::new(&serial_port_path, 9600).open()?)
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"))
        .init();
    let opts = Opts::parse();

    let factory = Factory { port_name: opts.serial_port.clone(), first: true };
    let mut everdrive = EverdriveSerial::new(factory)?;

    match opts.command {
        Command::SetMode(c) => {
            let mode = match c.mode.as_str() {
                "app" => Mode::App,
                "service" => Mode::Service,
                other => Err(anyhow!("unexpected mode {}", other))?,
            };
            everdrive.set_mode(mode)?;
        },
        Command::Reset(c) => {
            info!("resetting");
            let mode = if c.hard { ResetMode::Hard } else { ResetMode::Soft };
            everdrive.reset_host(mode)?;
        }
        Command::Recover(_) => {
          everdrive.recover()?;
        },
        Command::Run(c) => {
            let contents = std::fs::read(&c.path)?;
            let file_name = c.path.file_name().unwrap().to_str().unwrap();

            if let Some(fpga_path) = c.fpga.as_ref() {
                let fpga_bin = std::fs::read(fpga_path)?;
                everdrive.load_fpga_from_slice(&fpga_bin)?;
            }

            everdrive.load_game(file_name, &contents, c.skip_fpga || c.fpga.is_some())?;
        }
        Command::LoadFPGA(c) => {
            if let Some(p) = c.path.as_ref() {
                let fpga_bin = std::fs::read(p)?;
                everdrive.load_fpga_from_slice(&fpga_bin)?;
            } else if let Some(p) = c.sd.as_ref() {
                everdrive.load_fpga_from_sd(p)?;
            } else if let Some(addr) = c.flash.clone() {
                everdrive.load_fpga_from_flash(addr)?;
            } else {
                Err(anyhow!("load-fpga needs at least one path argument"))?;
            }
        },
    }

    everdrive.reset_host(ResetMode::Off)?;
    Ok(())
}
