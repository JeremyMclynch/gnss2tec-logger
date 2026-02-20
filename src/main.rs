use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, Timelike, Utc};
use clap::{Args, Parser, Subcommand};
use fs2::FileExt;
use serialport::SerialPort;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "gnss2tec-logger", version)]
#[command(about = "GNSS UBX logger and RINEX conversion pipeline")]
struct Cli {
    #[command(subcommand)]
    command: AppCommand,
}

#[derive(Subcommand, Debug)]
enum AppCommand {
    /// Stream UBX bytes from a serial receiver to hourly files
    Log(LogArgs),
    /// Convert UBX files to hourly RINEX, compress, archive, and clean up
    Convert(ConvertArgs),
    /// Run logger continuously and trigger periodic conversion in parallel
    Run(RunArgs),
}

#[derive(Args, Debug, Clone)]
struct LogArgs {
    #[arg(long, default_value = "/dev/ttyACM0")]
    serial_port: String,
    #[arg(long, default_value_t = 115_200)]
    baud_rate: u32,
    #[arg(long, default_value_t = 250)]
    read_timeout_ms: u64,
    #[arg(long, default_value_t = 8_192)]
    read_buffer_bytes: usize,
    #[arg(long, default_value_t = 5)]
    flush_interval_secs: u64,
    #[arg(long, default_value_t = 50)]
    command_gap_ms: u64,
    #[arg(long, default_value = "refrence scripts/ubx.dat")]
    config_file: PathBuf,
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
    #[arg(long, default_value = "ubx_log.lock")]
    lock_file: PathBuf,
}

#[derive(Args, Debug, Clone)]
struct ConvertArgs {
    #[arg(long, default_value = "NJIT")]
    station: String,
    #[arg(long, default_value = "USA")]
    country: String,
    #[arg(long, default_value = "U-Blox ZED F9P/02B-00")]
    receiver_type: String,
    #[arg(long, default_value = "TOPGNSS AN-105L")]
    antenna_type: String,
    #[arg(long, default_value = "H. Kim/NJIT")]
    observer: String,
    #[arg(long, default_value_t = 1)]
    shift_hours: u32,
    #[arg(long, default_value_t = 3)]
    max_days_back: u32,
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
    #[arg(long, default_value = "archive")]
    archive_dir: PathBuf,
    #[arg(long, default_value = "convert.lock")]
    lock_file: PathBuf,
    #[arg(long, default_value = "convbin")]
    convbin_path: PathBuf,
    #[arg(long, default_value = "gfzrnx_2.1.0_armlx64")]
    gfzrnx_path: PathBuf,
    #[arg(long, default_value = "rnx2crx")]
    rnx2crx_path: PathBuf,
    #[arg(long, default_value = "gzip")]
    gzip_path: PathBuf,
    #[arg(long, default_value_t = false)]
    skip_nav: bool,
    #[arg(long, default_value_t = false)]
    keep_ubx: bool,
}

#[derive(Args, Debug, Clone)]
struct RunArgs {
    #[arg(long, default_value = "/dev/ttyACM0")]
    serial_port: String,
    #[arg(long, default_value_t = 115_200)]
    baud_rate: u32,
    #[arg(long, default_value_t = 250)]
    read_timeout_ms: u64,
    #[arg(long, default_value_t = 8_192)]
    read_buffer_bytes: usize,
    #[arg(long, default_value_t = 5)]
    flush_interval_secs: u64,
    #[arg(long, default_value_t = 50)]
    command_gap_ms: u64,
    #[arg(long, default_value = "refrence scripts/ubx.dat")]
    config_file: PathBuf,
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
    #[arg(long, default_value = "ubx_log.lock")]
    log_lock_file: PathBuf,
    #[arg(long, default_value = "NJIT")]
    station: String,
    #[arg(long, default_value = "USA")]
    country: String,
    #[arg(long, default_value = "U-Blox ZED F9P/02B-00")]
    receiver_type: String,
    #[arg(long, default_value = "TOPGNSS AN-105L")]
    antenna_type: String,
    #[arg(long, default_value = "H. Kim/NJIT")]
    observer: String,
    #[arg(long, default_value_t = 1)]
    shift_hours: u32,
    #[arg(long, default_value_t = 3)]
    max_days_back: u32,
    #[arg(long, default_value = "archive")]
    archive_dir: PathBuf,
    #[arg(long, default_value = "convert.lock")]
    convert_lock_file: PathBuf,
    #[arg(long, default_value = "convbin")]
    convbin_path: PathBuf,
    #[arg(long, default_value = "gfzrnx_2.1.0_armlx64")]
    gfzrnx_path: PathBuf,
    #[arg(long, default_value = "rnx2crx")]
    rnx2crx_path: PathBuf,
    #[arg(long, default_value = "gzip")]
    gzip_path: PathBuf,
    #[arg(long, default_value_t = false)]
    skip_nav: bool,
    #[arg(long, default_value_t = false)]
    keep_ubx: bool,
    #[arg(long, default_value_t = 300)]
    convert_interval_secs: u64,
    #[arg(long = "no-convert-on-start", action = clap::ArgAction::SetFalse, default_value_t = true)]
    convert_on_start: bool,
}

impl RunArgs {
    fn to_log_args(&self) -> LogArgs {
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

    fn to_convert_args(&self) -> ConvertArgs {
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

struct LockGuard {
    file: File,
}

impl LockGuard {
    fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("creating lock directory failed: {}", parent.display())
                })?;
            }
        }

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("opening lock file failed: {}", path.display()))?;

        file.try_lock_exclusive()
            .with_context(|| format!("another instance is already running: {}", path.display()))?;

        Ok(Self { file })
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        AppCommand::Log(args) => run_log(args),
        AppCommand::Convert(args) => run_convert(args),
        AppCommand::Run(args) => run_mode(args),
    }
}

fn run_log(args: LogArgs) -> Result<()> {
    let running = install_ctrlc_handler()?;
    run_log_with_signal(args, running)
}

fn run_mode(args: RunArgs) -> Result<()> {
    let running = install_ctrlc_handler()?;
    let log_args = args.to_log_args();
    let convert_args = args.to_convert_args();
    let convert_interval = Duration::from_secs(args.convert_interval_secs.max(1));
    let convert_on_start = args.convert_on_start;

    let convert_running = Arc::clone(&running);
    let convert_handle = spawn_convert_loop(
        convert_args,
        convert_running,
        convert_interval,
        convert_on_start,
    );

    let log_result = run_log_with_signal(log_args, Arc::clone(&running));
    running.store(false, Ordering::SeqCst);
    join_convert_loop(convert_handle);
    log_result
}

fn run_log_with_signal(args: LogArgs, running: Arc<AtomicBool>) -> Result<()> {
    fs::create_dir_all(&args.data_dir).with_context(|| {
        format!(
            "creating data directory failed: {}",
            args.data_dir.display()
        )
    })?;
    let _lock = LockGuard::acquire(&args.lock_file)?;

    let packets = parse_ubx_config(&args.config_file)?;
    if packets.is_empty() {
        bail!(
            "no UBX commands found in configuration file: {}",
            args.config_file.display()
        );
    }

    let mut port = serialport::new(&args.serial_port, args.baud_rate)
        .timeout(Duration::from_millis(args.read_timeout_ms))
        .open()
        .with_context(|| {
            format!(
                "opening serial port failed: {} @ {}",
                args.serial_port, args.baud_rate
            )
        })?;

    send_ubx_packets(
        &mut *port,
        &packets,
        Duration::from_millis(args.command_gap_ms),
    )?;
    eprintln!(
        "Sent {} UBX configuration commands from {}",
        packets.len(),
        args.config_file.display()
    );

    let mut buffer = vec![0_u8; args.read_buffer_bytes.max(1_024)];
    let flush_interval = Duration::from_secs(args.flush_interval_secs.max(1));
    let mut last_flush = Instant::now();
    let mut total_bytes: u64 = 0;

    let (mut active_hour_key, mut writer, current_path) = open_new_log_file(&args.data_dir)?;
    eprintln!("Logging UBX data to {}", current_path.display());

    while running.load(Ordering::SeqCst) {
        match port.read(&mut buffer) {
            Ok(0) => {}
            Ok(size) => {
                writer
                    .write_all(&buffer[..size])
                    .context("writing UBX bytes to file failed")?;
                total_bytes += size as u64;
            }
            Err(err) if err.kind() == io::ErrorKind::TimedOut => {}
            Err(err) => {
                return Err(err).context("reading GNSS stream from serial port failed");
            }
        }

        let now = Utc::now();
        let hour_key = now.format("%Y%m%d_%H").to_string();
        if hour_key != active_hour_key {
            writer.flush().context("flushing log file failed")?;
            let (new_hour_key, new_writer, path) = open_new_log_file(&args.data_dir)?;
            active_hour_key = new_hour_key;
            writer = new_writer;
            eprintln!("Rotated UBX output to {}", path.display());
        }

        if last_flush.elapsed() >= flush_interval {
            writer.flush().context("periodic flush failed")?;
            last_flush = Instant::now();
        }
    }

    writer.flush().context("final flush failed")?;
    eprintln!("Logger stopped, wrote {} bytes", total_bytes);
    Ok(())
}

fn run_convert(args: ConvertArgs) -> Result<()> {
    fs::create_dir_all(&args.data_dir).with_context(|| {
        format!(
            "creating data directory failed: {}",
            args.data_dir.display()
        )
    })?;
    fs::create_dir_all(&args.archive_dir).with_context(|| {
        format!(
            "creating archive directory failed: {}",
            args.archive_dir.display()
        )
    })?;
    let _lock = LockGuard::acquire(&args.lock_file)?;

    let total_hours = i64::from(args.max_days_back) * 24;
    if total_hours <= 0 {
        bail!("max_days_back must be greater than zero");
    }

    let anchor = floor_to_hour(Utc::now() - ChronoDuration::hours(i64::from(args.shift_hours)));

    let mut processed_hours = 0_u32;
    for offset in 0..total_hours {
        let dt = anchor - ChronoDuration::hours(offset);
        let prefix = dt.format("%Y%m%d_%H").to_string();
        let ubx_files = list_hour_ubx_files(&args.data_dir, &prefix)?;
        if ubx_files.is_empty() {
            continue;
        }

        eprintln!(
            "Processing UTC hour {} with {} UBX file(s)",
            dt.format("%Y-%m-%d %H:00"),
            ubx_files.len()
        );
        process_hour(&args, dt, &ubx_files)?;
        processed_hours += 1;
    }

    eprintln!("Conversion complete; processed {} hour(s)", processed_hours);
    Ok(())
}

fn install_ctrlc_handler() -> Result<Arc<AtomicBool>> {
    let running = Arc::new(AtomicBool::new(true));
    let running_for_signal = Arc::clone(&running);
    ctrlc::set_handler(move || {
        running_for_signal.store(false, Ordering::SeqCst);
    })
    .context("installing Ctrl-C handler failed")?;
    Ok(running)
}

fn spawn_convert_loop(
    convert_args: ConvertArgs,
    running: Arc<AtomicBool>,
    interval: Duration,
    run_immediately: bool,
) -> JoinHandle<()> {
    thread::spawn(move || {
        if run_immediately && running.load(Ordering::SeqCst) {
            execute_convert_once(&convert_args);
        }

        while running.load(Ordering::SeqCst) {
            if !sleep_until_next_cycle(&running, interval) {
                break;
            }
            execute_convert_once(&convert_args);
        }
    })
}

fn join_convert_loop(handle: JoinHandle<()>) {
    if let Err(err) = handle.join() {
        eprintln!("Convert loop thread terminated unexpectedly: {:?}", err);
    }
}

fn sleep_until_next_cycle(running: &AtomicBool, interval: Duration) -> bool {
    let started = Instant::now();
    while running.load(Ordering::SeqCst) && started.elapsed() < interval {
        thread::sleep(Duration::from_millis(250));
    }
    running.load(Ordering::SeqCst)
}

fn execute_convert_once(convert_args: &ConvertArgs) {
    if let Err(err) = run_convert(convert_args.clone()) {
        eprintln!("Convert cycle failed (logger continues): {err:#}");
    }
}

fn process_hour(args: &ConvertArgs, dt: DateTime<Utc>, ubx_files: &[PathBuf]) -> Result<()> {
    let year = dt.format("%Y").to_string();
    let hour = dt.format("%H").to_string();
    let doy = format!("{:03}", dt.ordinal());
    let epoch_begin = format!("{}0000", dt.format("%Y-%m-%d_%H"));

    let obs_rnx_name = format!(
        "{}00{}_R_{}{}{}00_01H_01S_MO.rnx",
        args.station, args.country, year, doy, hour
    );
    let obs_rnx_path = args.data_dir.join(&obs_rnx_name);

    let mut obs_parts = Vec::with_capacity(ubx_files.len());
    for ubx in ubx_files {
        let obs_part = args
            .data_dir
            .join(format!("{}.obs", sanitize_stem_for_temp(ubx)?));
        run_convbin_obs(args, ubx, &obs_part)?;
        obs_parts.push(obs_part);
    }

    run_gfzrnx(args, &obs_parts, &obs_rnx_path, &epoch_begin)?;
    run_rnx2crx(args, &obs_rnx_path)?;

    let obs_crx_path = obs_rnx_path.with_extension("crx");
    gzip_file(&args.gzip_path, &obs_crx_path)?;
    let obs_gz_path = obs_crx_path.with_extension("crx.gz");
    if !obs_gz_path.exists() {
        bail!(
            "expected observation gzip output not found: {}",
            obs_gz_path.display()
        );
    }

    let nav_gz_path = if args.skip_nav {
        None
    } else {
        Some(build_nav_output(args, dt, ubx_files)?)
    };

    for obs_part in &obs_parts {
        remove_file_if_exists(obs_part)?;
    }
    remove_file_if_exists(&obs_rnx_path)?;

    if !args.keep_ubx {
        for ubx in ubx_files {
            remove_file_if_exists(ubx)?;
        }
    }

    let archive_path = args.archive_dir.join(&year).join(&doy);
    fs::create_dir_all(&archive_path)
        .with_context(|| format!("creating archive path failed: {}", archive_path.display()))?;

    move_into_dir(&obs_gz_path, &archive_path)?;
    if let Some(nav_gz) = nav_gz_path {
        move_into_dir(&nav_gz, &archive_path)?;
    }

    Ok(())
}

fn build_nav_output(
    args: &ConvertArgs,
    dt: DateTime<Utc>,
    ubx_files: &[PathBuf],
) -> Result<PathBuf> {
    let year = dt.format("%Y").to_string();
    let hour = dt.format("%H").to_string();
    let doy = format!("{:03}", dt.ordinal());

    let nav_rnx_name = format!(
        "{}00{}_R_{}{}{}00_01H_MN.rnx",
        args.station, args.country, year, doy, hour
    );
    let nav_rnx_path = args.data_dir.join(nav_rnx_name);

    let hour_key = dt.format("%Y%m%d_%H").to_string();
    let merged_ubx_path = args
        .data_dir
        .join(format!(".tmp_{hour_key}_nav_merged.ubx"));
    let nav_obs_dummy = args.data_dir.join(format!(".tmp_{hour_key}_nav.obs"));

    concat_binary_files(ubx_files, &merged_ubx_path)?;
    run_convbin_nav(args, &merged_ubx_path, &nav_obs_dummy, &nav_rnx_path)?;
    remove_file_if_exists(&merged_ubx_path)?;
    remove_file_if_exists(&nav_obs_dummy)?;

    gzip_file(&args.gzip_path, &nav_rnx_path)?;
    remove_file_if_exists(&nav_rnx_path)?;
    let nav_gz_path = nav_rnx_path.with_extension("rnx.gz");

    if !nav_gz_path.exists() {
        bail!(
            "expected navigation gzip output not found: {}",
            nav_gz_path.display()
        );
    }

    Ok(nav_gz_path)
}

fn run_convbin_obs(args: &ConvertArgs, input_ubx: &Path, output_obs: &Path) -> Result<()> {
    let receiver = format!("Unknown/{}", args.receiver_type);
    let antenna = format!("Unknown/{}", args.antenna_type);

    let mut cmd = Command::new(&args.convbin_path);
    cmd.arg("-os")
        .arg("-od")
        .arg("-oi")
        .arg("-r")
        .arg("ubx")
        .arg("-v")
        .arg("3.04")
        .arg(input_ubx)
        .arg("-hm")
        .arg(&args.station)
        .arg("-hr")
        .arg(&receiver)
        .arg("-ha")
        .arg(&antenna)
        .arg("-ho")
        .arg(&args.observer)
        .arg("-o")
        .arg(output_obs);

    run_checked_command(
        &mut cmd,
        &format!("convbin observation conversion ({})", input_ubx.display()),
    )
}

fn run_convbin_nav(
    args: &ConvertArgs,
    input_ubx: &Path,
    dummy_obs: &Path,
    output_nav: &Path,
) -> Result<()> {
    let receiver = format!("Unknown/{}", args.receiver_type);
    let antenna = format!("Unknown/{}", args.antenna_type);

    let mut cmd = Command::new(&args.convbin_path);
    cmd.arg("-os")
        .arg("-od")
        .arg("-oi")
        .arg("-r")
        .arg("ubx")
        .arg("-v")
        .arg("3.04")
        .arg(input_ubx)
        .arg("-hm")
        .arg(&args.station)
        .arg("-hr")
        .arg(&receiver)
        .arg("-ha")
        .arg(&antenna)
        .arg("-ho")
        .arg(&args.observer)
        .arg("-o")
        .arg(dummy_obs)
        .arg("-n")
        .arg(output_nav);

    run_checked_command(
        &mut cmd,
        &format!("convbin navigation conversion ({})", input_ubx.display()),
    )
}

fn run_gfzrnx(
    args: &ConvertArgs,
    obs_parts: &[PathBuf],
    output: &Path,
    epoch_begin: &str,
) -> Result<()> {
    if obs_parts.is_empty() {
        bail!("no observation fragments were generated for gfzrnx merge");
    }

    let mut cmd = Command::new(&args.gfzrnx_path);
    cmd.arg("-finp");
    for path in obs_parts {
        cmd.arg(path);
    }
    cmd.arg("-epo_beg")
        .arg(epoch_begin)
        .arg("-d")
        .arg("3600")
        .arg("-fout")
        .arg(output)
        .arg("-f");

    run_checked_command(&mut cmd, &format!("gfzrnx merge ({})", output.display()))
}

fn run_rnx2crx(args: &ConvertArgs, obs_rnx: &Path) -> Result<()> {
    let mut cmd = Command::new(&args.rnx2crx_path);
    cmd.arg("-f").arg(obs_rnx);
    run_checked_command(
        &mut cmd,
        &format!("rnx2crx compression ({})", obs_rnx.display()),
    )
}

fn gzip_file(gzip_path: &Path, input: &Path) -> Result<()> {
    let mut cmd = Command::new(gzip_path);
    cmd.arg("-f").arg(input);
    run_checked_command(&mut cmd, &format!("gzip compression ({})", input.display()))
}

fn run_checked_command(cmd: &mut Command, label: &str) -> Result<()> {
    let debug = format!("{cmd:?}");
    let output = cmd
        .output()
        .with_context(|| format!("spawning command failed for {label}: {debug}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "{label} failed with status {}.\nstdout:\n{}\nstderr:\n{}",
        output.status,
        stdout.trim(),
        stderr.trim()
    );
}

fn list_hour_ubx_files(data_dir: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(data_dir)
        .with_context(|| format!("reading data directory failed: {}", data_dir.display()))?
    {
        let entry = entry.with_context(|| format!("iterating {}", data_dir.display()))?;
        if !entry
            .file_type()
            .with_context(|| format!("reading metadata for {}", entry.path().display()))?
            .is_file()
        {
            continue;
        }

        let path = entry.path();
        if path.extension() != Some(OsStr::new("ubx")) {
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name.starts_with(prefix) {
            files.push(path);
        }
    }

    files.sort();
    Ok(files)
}

fn open_new_log_file(data_dir: &Path) -> Result<(String, File, PathBuf)> {
    let now = Utc::now();
    let hour_key = now.format("%Y%m%d_%H").to_string();
    let file_name = format!("{}.ubx", now.format("%Y%m%d_%H%M%S"));
    let path = data_dir.join(file_name);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening log output failed: {}", path.display()))?;
    Ok((hour_key, file, path))
}

fn send_ubx_packets(
    port: &mut dyn SerialPort,
    packets: &[Vec<u8>],
    pause_between_commands: Duration,
) -> Result<()> {
    for packet in packets {
        port.write_all(packet)
            .context("writing UBX config command failed")?;
        port.flush().context("flushing UBX config command failed")?;
        thread::sleep(pause_between_commands);
    }
    Ok(())
}

fn parse_ubx_config(config_file: &Path) -> Result<Vec<Vec<u8>>> {
    let contents = fs::read_to_string(config_file)
        .with_context(|| format!("reading UBX config failed: {}", config_file.display()))?;
    let mut packets = Vec::new();

    for (line_idx, raw) in contents.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with("!UBX ") {
            continue;
        }

        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 3 {
            bail!(
                "invalid UBX config line {} in {}",
                line_idx + 1,
                config_file.display()
            );
        }

        let command = tokens[1];
        let args = &tokens[2..];
        let (class, id, payload) = build_ubx_payload(command, args).with_context(|| {
            format!(
                "invalid UBX command at {}:{}",
                config_file.display(),
                line_idx + 1
            )
        })?;

        packets.push(build_ubx_packet(class, id, &payload));
    }

    Ok(packets)
}

fn build_ubx_payload(command: &str, args: &[&str]) -> Result<(u8, u8, Vec<u8>)> {
    match command {
        "CFG-MSG" => {
            if args.len() != 8 {
                bail!("CFG-MSG expects 8 arguments, got {}", args.len());
            }
            let mut payload = Vec::with_capacity(8);
            for item in args {
                payload.push(parse_u8_token(item)?);
            }
            Ok((0x06, 0x01, payload))
        }
        "CFG-GNSS" => {
            if args.len() != 9 {
                bail!("CFG-GNSS expects 9 arguments, got {}", args.len());
            }
            let mut payload = Vec::with_capacity(12);
            for item in &args[..8] {
                payload.push(parse_u8_token(item)?);
            }
            let flags = parse_u32_token(args[8])?;
            payload.extend_from_slice(&flags.to_le_bytes());
            Ok((0x06, 0x3E, payload))
        }
        "CFG-RATE" => {
            if args.len() != 3 {
                bail!("CFG-RATE expects 3 arguments, got {}", args.len());
            }
            let meas_rate = parse_u16_token(args[0])?;
            let nav_rate = parse_u16_token(args[1])?;
            let time_ref = parse_u16_token(args[2])?;
            let mut payload = Vec::with_capacity(6);
            payload.extend_from_slice(&meas_rate.to_le_bytes());
            payload.extend_from_slice(&nav_rate.to_le_bytes());
            payload.extend_from_slice(&time_ref.to_le_bytes());
            Ok((0x06, 0x08, payload))
        }
        _ => bail!("unsupported UBX command in config: {command}"),
    }
}

fn parse_u8_token(raw: &str) -> Result<u8> {
    let value = parse_u32_token(raw)?;
    u8::try_from(value).map_err(|_| anyhow!("value out of range for u8: {raw}"))
}

fn parse_u16_token(raw: &str) -> Result<u16> {
    let value = parse_u32_token(raw)?;
    u16::try_from(value).map_err(|_| anyhow!("value out of range for u16: {raw}"))
}

fn parse_u32_token(raw: &str) -> Result<u32> {
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).with_context(|| format!("invalid hex value: {raw}"));
    }
    raw.parse::<u32>()
        .with_context(|| format!("invalid integer value: {raw}"))
}

fn build_ubx_packet(class: u8, id: u8, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(payload.len() + 8);
    packet.extend_from_slice(&[0xB5, 0x62, class, id]);
    packet.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    packet.extend_from_slice(payload);
    let (ck_a, ck_b) = ubx_checksum(&packet[2..]);
    packet.push(ck_a);
    packet.push(ck_b);
    packet
}

fn ubx_checksum(data: &[u8]) -> (u8, u8) {
    let mut ck_a = 0_u8;
    let mut ck_b = 0_u8;
    for byte in data {
        ck_a = ck_a.wrapping_add(*byte);
        ck_b = ck_b.wrapping_add(ck_a);
    }
    (ck_a, ck_b)
}

fn sanitize_stem_for_temp(path: &Path) -> Result<String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("invalid UTF-8 file name: {}", path.display()))?;
    Ok(stem.replace('/', "_"))
}

fn concat_binary_files(inputs: &[PathBuf], output: &Path) -> Result<()> {
    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(output)
        .with_context(|| format!("opening merged UBX output failed: {}", output.display()))?;

    let mut buffer = vec![0_u8; 64 * 1024];
    for input in inputs {
        let mut file = File::open(input)
            .with_context(|| format!("opening UBX input failed: {}", input.display()))?;
        loop {
            let count = file
                .read(&mut buffer)
                .with_context(|| format!("reading UBX input failed: {}", input.display()))?;
            if count == 0 {
                break;
            }
            out.write_all(&buffer[..count])
                .with_context(|| format!("writing merged UBX failed: {}", output.display()))?;
        }
    }
    out.flush()
        .with_context(|| format!("flushing merged UBX failed: {}", output.display()))?;
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("removing file failed: {}", path.display())),
    }
}

fn move_into_dir(src: &Path, dst_dir: &Path) -> Result<PathBuf> {
    let file_name = src
        .file_name()
        .ok_or_else(|| anyhow!("missing file name for source: {}", src.display()))?;
    let dst = dst_dir.join(file_name);

    match fs::rename(src, &dst) {
        Ok(()) => Ok(dst),
        Err(_) => {
            fs::copy(src, &dst).with_context(|| {
                format!(
                    "copying file to archive failed: {} -> {}",
                    src.display(),
                    dst.display()
                )
            })?;
            fs::remove_file(src)
                .with_context(|| format!("removing source file failed: {}", src.display()))?;
            Ok(dst)
        }
    }
}

fn floor_to_hour(dt: DateTime<Utc>) -> DateTime<Utc> {
    dt.with_minute(0)
        .and_then(|v| v.with_second(0))
        .and_then(|v| v.with_nanosecond(0))
        .expect("UTC floor-to-hour should always be valid")
}
