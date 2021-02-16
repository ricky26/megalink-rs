use std::path::PathBuf;
use clap::Clap;
use log::{info, warn};
use async_trait::async_trait;
use anyhow::anyhow;
use megalink_rs::{EverdriveSerial, Mode, SerialFactory, ResetMode};
use tokio_serial::Serial;
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
    Reset(CmdReset),
    Run(CmdRunGame),
}

#[derive(Clap)]
struct CmdReset {
    #[clap(short, long)]
    hard: bool,
}

#[derive(Clap)]
struct CmdRunGame {
    path: PathBuf,

    #[clap(short, long)]
    skip_fpga: bool,
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
        Command::Reset(r) => {
            info!("resetting");
            let mode = if r.hard { ResetMode::Hard } else { ResetMode::Soft };
            everdrive.reset_host(mode)?;
        },
        Command::Run(r) => {
            let contents = std::fs::read(&r.path)?;
            let file_name = r.path.file_name().unwrap().to_str().unwrap();
            everdrive.load_game(file_name, &contents, r.skip_fpga)?;
        },
    }

    Ok(())
}
