use crate::args::LogArgs;
use crate::shared::lock::LockGuard;
use crate::shared::signal::install_ctrlc_handler;
use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use serialport::SerialPort;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

// Public log command entrypoint. This mode configures the receiver and then streams UBX bytes to disk.
pub fn run_log(args: LogArgs) -> Result<()> {
    let running = install_ctrlc_handler()?;
    run_log_with_signal(args, running)
}

// Shared logger implementation used by both `log` and `run` commands.
// A shared run flag allows run-mode to coordinate shutdown between logger and converter thread.
pub(crate) fn run_log_with_signal(args: LogArgs, running: Arc<AtomicBool>) -> Result<()> {
    // Prepare runtime output folder and enforce single-instance execution.
    fs::create_dir_all(&args.data_dir).with_context(|| {
        format!(
            "creating data directory failed: {}",
            args.data_dir.display()
        )
    })?;
    let _lock = LockGuard::acquire(&args.lock_file)?;

    // Parse config file and push UBX commands to the receiver before logging starts.
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

    // Main logging loop: read serial bytes, rotate files hourly, and flush periodically.
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

// Open a fresh UTC-timestamped output file and return the hour key for rotation comparisons.
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

// Write each UBX config packet with a short delay so the receiver can process command bursts.
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

// Parse `ubx.dat`-style lines into full UBX packets.
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

// Build UBX payload for supported textual commands from the config file.
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

// Numeric parsing helpers for config arguments.
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

// Build full UBX packet with header, payload length, and checksum.
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

// UBX Fletcher-like checksum over class/id/length/payload bytes.
fn ubx_checksum(data: &[u8]) -> (u8, u8) {
    let mut ck_a = 0_u8;
    let mut ck_b = 0_u8;
    for byte in data {
        ck_a = ck_a.wrapping_add(*byte);
        ck_b = ck_b.wrapping_add(ck_a);
    }
    (ck_a, ck_b)
}
