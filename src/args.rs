use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum NmeaLogFormat {
    Raw,
    Plain,
    Both,
}

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
    /// Run logger continuously and convert closed UTC hours inline
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
    #[arg(long, default_value_t = 5)]
    pub stats_interval_secs: u64,
    #[arg(long, default_value_t = 30)]
    pub nmea_log_interval_secs: u64,
    #[arg(long, value_enum, default_value_t = NmeaLogFormat::Raw)]
    pub nmea_log_format: NmeaLogFormat,
    #[arg(long, default_value_t = 50)]
    pub command_gap_ms: u64,
    #[arg(long, default_value = "/etc/gnss2tec-logger/ubx.dat")]
    pub config_file: PathBuf,
    #[arg(long, default_value = "/var/lib/gnss2tec-logger/data")]
    pub data_dir: PathBuf,
    #[arg(long, default_value = "/var/lib/gnss2tec-logger/ubx_log.lock")]
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
    #[arg(long, default_value = "/var/lib/gnss2tec-logger/data")]
    pub data_dir: PathBuf,
    #[arg(long, default_value = "/var/lib/gnss2tec-logger/archive")]
    pub archive_dir: PathBuf,
    #[arg(long, default_value = "/var/lib/gnss2tec-logger/convert.lock")]
    pub lock_file: PathBuf,
    #[arg(long, default_value = "/usr/lib/gnss2tec-logger/bin/ubx2rinex")]
    pub ubx2rinex_path: PathBuf,
    #[arg(long, default_value_t = false)]
    pub skip_nav: bool,
    #[arg(long, default_value_t = false)]
    pub keep_ubx: bool,
}

// Combined runtime mode config.
// In this mode, conversion is event-driven: when an hour closes, it is converted immediately.
#[derive(Args, Debug, Clone)]
pub struct RunArgs {
    #[arg(long, env = "GNSS2TEC_SERIAL_PORT", default_value = "/dev/ttyACM0")]
    pub serial_port: String,
    #[arg(long, env = "GNSS2TEC_BAUD_RATE", default_value_t = 115_200)]
    pub baud_rate: u32,
    #[arg(long, env = "GNSS2TEC_READ_TIMEOUT_MS", default_value_t = 250)]
    pub read_timeout_ms: u64,
    #[arg(long, env = "GNSS2TEC_READ_BUFFER_BYTES", default_value_t = 8_192)]
    pub read_buffer_bytes: usize,
    #[arg(long, env = "GNSS2TEC_FLUSH_INTERVAL_SECS", default_value_t = 5)]
    pub flush_interval_secs: u64,
    #[arg(long, env = "GNSS2TEC_STATS_INTERVAL_SECS", default_value_t = 5)]
    pub stats_interval_secs: u64,
    #[arg(
        long,
        env = "GNSS2TEC_NMEA_LOG_INTERVAL_SECS",
        default_value_t = 30
    )]
    pub nmea_log_interval_secs: u64,
    #[arg(
        long,
        env = "GNSS2TEC_NMEA_LOG_FORMAT",
        value_enum,
        default_value_t = NmeaLogFormat::Raw
    )]
    pub nmea_log_format: NmeaLogFormat,
    #[arg(long, env = "GNSS2TEC_COMMAND_GAP_MS", default_value_t = 50)]
    pub command_gap_ms: u64,
    #[arg(long, env = "GNSS2TEC_CONFIG_FILE", default_value = "/etc/gnss2tec-logger/ubx.dat")]
    pub config_file: PathBuf,
    #[arg(long, env = "GNSS2TEC_DATA_DIR", default_value = "/var/lib/gnss2tec-logger/data")]
    pub data_dir: PathBuf,
    #[arg(long, env = "GNSS2TEC_STATION", default_value = "NJIT")]
    pub station: String,
    #[arg(long, env = "GNSS2TEC_COUNTRY", default_value = "USA")]
    pub country: String,
    #[arg(
        long,
        env = "GNSS2TEC_RECEIVER_TYPE",
        default_value = "U-Blox ZED F9P/02B-00"
    )]
    pub receiver_type: String,
    #[arg(long, env = "GNSS2TEC_ANTENNA_TYPE", default_value = "TOPGNSS AN-105L")]
    pub antenna_type: String,
    #[arg(long, env = "GNSS2TEC_OBSERVER", default_value = "H. Kim/NJIT")]
    pub observer: String,
    #[arg(long, env = "GNSS2TEC_SHIFT_HOURS", default_value_t = 1)]
    pub shift_hours: u32,
    #[arg(long, env = "GNSS2TEC_MAX_DAYS_BACK", default_value_t = 3)]
    pub max_days_back: u32,
    #[arg(
        long,
        env = "GNSS2TEC_ARCHIVE_DIR",
        default_value = "/var/lib/gnss2tec-logger/archive"
    )]
    pub archive_dir: PathBuf,
    #[arg(
        long,
        env = "GNSS2TEC_UBX2RINEX_PATH",
        default_value = "/usr/lib/gnss2tec-logger/bin/ubx2rinex"
    )]
    pub ubx2rinex_path: PathBuf,
    #[arg(long, env = "GNSS2TEC_SKIP_NAV", default_value_t = false)]
    pub skip_nav: bool,
    #[arg(long, env = "GNSS2TEC_KEEP_UBX", default_value_t = false)]
    pub keep_ubx: bool,
    #[arg(long = "no-convert-on-start", action = ArgAction::SetFalse, default_value_t = true)]
    pub convert_on_start: bool,
}

impl RunArgs {
    // Build ConvertArgs from the shared fields so run-mode reuses conversion helpers.
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
            lock_file: PathBuf::from("/var/lib/gnss2tec-logger/convert.lock"),
            ubx2rinex_path: self.ubx2rinex_path.clone(),
            skip_nav: self.skip_nav,
            keep_ubx: self.keep_ubx,
        }
    }
}
