use clap::{ArgAction, Args, Parser, Subcommand};
use std::path::PathBuf;

// CLI root definition. This is the single entrypoint for all supported modes.
#[derive(Parser, Debug)]
#[command(name = "gnss2tec-logger", version)]
#[command(about = "GNSS UBX logger and RINEX conversion pipeline")]
pub struct Cli {
    #[command(subcommand)]
    pub command: AppCommand,
}

// Subcommands map directly to one module each under src/commands/.
#[derive(Subcommand, Debug)]
pub enum AppCommand {
    /// Stream UBX bytes from a serial receiver to hourly files
    Log(LogArgs),
    /// Convert UBX files to hourly RINEX, compress, archive, and clean up
    Convert(ConvertArgs),
    /// Run logger continuously and trigger periodic conversion in parallel
    Run(RunArgs),
}

// Logging-only configuration. This mirrors the old ubx_log.sh behavior.
#[derive(Args, Debug, Clone)]
pub struct LogArgs {
    #[arg(long, default_value = "/dev/ttyACM0")]
    pub serial_port: String,
    #[arg(long, default_value_t = 115_200)]
    pub baud_rate: u32,
    #[arg(long, default_value_t = 250)]
    pub read_timeout_ms: u64,
    #[arg(long, default_value_t = 8_192)]
    pub read_buffer_bytes: usize,
    #[arg(long, default_value_t = 5)]
    pub flush_interval_secs: u64,
    #[arg(long, default_value_t = 50)]
    pub command_gap_ms: u64,
    #[arg(long, default_value = "refrence scripts/ubx.dat")]
    pub config_file: PathBuf,
    #[arg(long, default_value = "data")]
    pub data_dir: PathBuf,
    #[arg(long, default_value = "ubx_log.lock")]
    pub lock_file: PathBuf,
}

// Conversion configuration. This mirrors convert.sh while keeping paths configurable.
#[derive(Args, Debug, Clone)]
pub struct ConvertArgs {
    #[arg(long, default_value = "NJIT")]
    pub station: String,
    #[arg(long, default_value = "USA")]
    pub country: String,
    #[arg(long, default_value = "U-Blox ZED F9P/02B-00")]
    pub receiver_type: String,
    #[arg(long, default_value = "TOPGNSS AN-105L")]
    pub antenna_type: String,
    #[arg(long, default_value = "H. Kim/NJIT")]
    pub observer: String,
    #[arg(long, default_value_t = 1)]
    pub shift_hours: u32,
    #[arg(long, default_value_t = 3)]
    pub max_days_back: u32,
    #[arg(long, default_value = "data")]
    pub data_dir: PathBuf,
    #[arg(long, default_value = "archive")]
    pub archive_dir: PathBuf,
    #[arg(long, default_value = "convert.lock")]
    pub lock_file: PathBuf,
    #[arg(long, default_value = "convbin")]
    pub convbin_path: PathBuf,
    #[arg(long, default_value = "gfzrnx_2.1.0_armlx64")]
    pub gfzrnx_path: PathBuf,
    #[arg(long, default_value = "rnx2crx")]
    pub rnx2crx_path: PathBuf,
    #[arg(long, default_value = "gzip")]
    pub gzip_path: PathBuf,
    #[arg(long, default_value_t = false)]
    pub skip_nav: bool,
    #[arg(long, default_value_t = false)]
    pub keep_ubx: bool,
}

// Combined runtime mode config. It includes all logging + conversion fields and scheduling controls.
#[derive(Args, Debug, Clone)]
pub struct RunArgs {
    #[arg(long, default_value = "/dev/ttyACM0")]
    pub serial_port: String,
    #[arg(long, default_value_t = 115_200)]
    pub baud_rate: u32,
    #[arg(long, default_value_t = 250)]
    pub read_timeout_ms: u64,
    #[arg(long, default_value_t = 8_192)]
    pub read_buffer_bytes: usize,
    #[arg(long, default_value_t = 5)]
    pub flush_interval_secs: u64,
    #[arg(long, default_value_t = 50)]
    pub command_gap_ms: u64,
    #[arg(long, default_value = "refrence scripts/ubx.dat")]
    pub config_file: PathBuf,
    #[arg(long, default_value = "data")]
    pub data_dir: PathBuf,
    #[arg(long, default_value = "ubx_log.lock")]
    pub log_lock_file: PathBuf,
    #[arg(long, default_value = "NJIT")]
    pub station: String,
    #[arg(long, default_value = "USA")]
    pub country: String,
    #[arg(long, default_value = "U-Blox ZED F9P/02B-00")]
    pub receiver_type: String,
    #[arg(long, default_value = "TOPGNSS AN-105L")]
    pub antenna_type: String,
    #[arg(long, default_value = "H. Kim/NJIT")]
    pub observer: String,
    #[arg(long, default_value_t = 1)]
    pub shift_hours: u32,
    #[arg(long, default_value_t = 3)]
    pub max_days_back: u32,
    #[arg(long, default_value = "archive")]
    pub archive_dir: PathBuf,
    #[arg(long, default_value = "convert.lock")]
    pub convert_lock_file: PathBuf,
    #[arg(long, default_value = "convbin")]
    pub convbin_path: PathBuf,
    #[arg(long, default_value = "gfzrnx_2.1.0_armlx64")]
    pub gfzrnx_path: PathBuf,
    #[arg(long, default_value = "rnx2crx")]
    pub rnx2crx_path: PathBuf,
    #[arg(long, default_value = "gzip")]
    pub gzip_path: PathBuf,
    #[arg(long, default_value_t = false)]
    pub skip_nav: bool,
    #[arg(long, default_value_t = false)]
    pub keep_ubx: bool,
    #[arg(long, default_value_t = 300)]
    pub convert_interval_secs: u64,
    #[arg(long = "no-convert-on-start", action = ArgAction::SetFalse, default_value_t = true)]
    pub convert_on_start: bool,
}

impl RunArgs {
    // Build LogArgs from the shared fields so run-mode reuses the exact log implementation.
    pub fn to_log_args(&self) -> LogArgs {
        LogArgs {
            serial_port: self.serial_port.clone(),
            baud_rate: self.baud_rate,
            read_timeout_ms: self.read_timeout_ms,
            read_buffer_bytes: self.read_buffer_bytes,
            flush_interval_secs: self.flush_interval_secs,
            command_gap_ms: self.command_gap_ms,
            config_file: self.config_file.clone(),
            data_dir: self.data_dir.clone(),
            lock_file: self.log_lock_file.clone(),
        }
    }

    // Build ConvertArgs from the shared fields so run-mode reuses the exact conversion implementation.
    pub fn to_convert_args(&self) -> ConvertArgs {
        ConvertArgs {
            station: self.station.clone(),
            country: self.country.clone(),
            receiver_type: self.receiver_type.clone(),
            antenna_type: self.antenna_type.clone(),
            observer: self.observer.clone(),
            shift_hours: self.shift_hours,
            max_days_back: self.max_days_back,
            data_dir: self.data_dir.clone(),
            archive_dir: self.archive_dir.clone(),
            lock_file: self.convert_lock_file.clone(),
            convbin_path: self.convbin_path.clone(),
            gfzrnx_path: self.gfzrnx_path.clone(),
            rnx2crx_path: self.rnx2crx_path.clone(),
            gzip_path: self.gzip_path.clone(),
            skip_nav: self.skip_nav,
            keep_ubx: self.keep_ubx,
        }
    }
}
