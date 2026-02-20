use crate::args::{ConvertArgs, RunArgs};
use crate::commands::convert::{convert_hour_utc, ensure_converter_available};
use crate::commands::log::{parse_ubx_config, send_ubx_packets};
use crate::shared::lock::LockGuard;
use crate::shared::nmea::NmeaMonitor;
use crate::shared::signal::install_ctrlc_handler;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration as ChronoDuration, Timelike, Utc};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// Public run command entrypoint.
// This is the simplified primary mode: one process, one logging loop, background conversion worker.
pub fn run_mode(args: RunArgs) -> Result<()> {
    let running = install_ctrlc_handler()?;

    // Prepare directories once at startup.
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

    // Configure receiver before entering logging loop.
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

    // Start conversion worker so logging never blocks on conversion execution.
    let convert_args = args.to_convert_args();
    let (convert_tx, convert_worker) = spawn_conversion_worker(convert_args, Arc::clone(&running));

    // Optional startup catch-up: enqueue recent past hours for background conversion.
    if args.convert_on_start {
        let enqueued = enqueue_startup_catchup_hours(&args, &convert_tx);
        if enqueued > 0 {
            eprintln!("Startup catch-up enqueued {} hour(s)", enqueued);
        }
    }

    // Main single-thread logging loop.
    let mut buffer = vec![0_u8; args.read_buffer_bytes.max(1_024)];
    let flush_interval = Duration::from_secs(args.flush_interval_secs.max(1));
    let stats_interval = if args.stats_interval_secs == 0 {
        None
    } else {
        Some(Duration::from_secs(args.stats_interval_secs.max(1)))
    };
    let mut last_flush = Instant::now();
    let mut last_stats = Instant::now();
    let mut stats_window_bytes: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut nmea_monitor = NmeaMonitor::new(args.nmea_log_interval_secs, args.nmea_log_format);

    let (mut active_hour_key, mut active_hour_start, mut writer, current_path) =
        open_new_log_file_for_time(&args.data_dir, Utc::now())?;
    eprintln!("Logging UBX data to {}", current_path.display());

    while running.load(Ordering::SeqCst) {
        match port.read(&mut buffer) {
            Ok(0) => {}
            Ok(size) => {
                writer
                    .write_all(&buffer[..size])
                    .context("writing UBX bytes to file failed")?;
                total_bytes += size as u64;
                stats_window_bytes += size as u64;
                nmea_monitor.ingest(&buffer[..size]);
            }
            Err(err) if err.kind() == io::ErrorKind::TimedOut => {}
            Err(err) => {
                return Err(err).context("reading GNSS stream from serial port failed");
            }
        }

        let now = Utc::now();
        let hour_key = now.format("%Y%m%d_%H").to_string();
        if hour_key != active_hour_key {
            // Flush and rotate quickly first to avoid any logging gaps.
            writer.flush().context("flushing log file failed")?;
            let closed_hour = active_hour_start;

            let (new_hour_key, new_hour_start, new_writer, path) =
                open_new_log_file_for_time(&args.data_dir, now)?;
            let old_writer = std::mem::replace(&mut writer, new_writer);
            drop(old_writer);
            active_hour_key = new_hour_key;
            active_hour_start = new_hour_start;
            eprintln!("Rotated UBX output to {}", path.display());

            if let Err(err) = convert_tx.send(closed_hour) {
                eprintln!(
                    "Conversion worker channel closed; skipped conversion for {}: {}",
                    closed_hour.format("%Y-%m-%d %H:00"),
                    err
                );
            }
        }

        if last_flush.elapsed() >= flush_interval {
            writer.flush().context("periodic flush failed")?;
            last_flush = Instant::now();
        }

        if let Some(interval) = stats_interval
            && last_stats.elapsed() >= interval
        {
            let elapsed = last_stats.elapsed().as_secs_f64().max(0.001);
            let bps = ((stats_window_bytes as f64 * 8.0) / elapsed).round() as u64;
            eprintln!(
                "[STAT] {:>10} B {:>7} bps {}",
                total_bytes, bps, args.serial_port
            );
            stats_window_bytes = 0;
            last_stats = Instant::now();
        }

        nmea_monitor.maybe_emit_logs();
    }

    writer.flush().context("final flush failed")?;
    drop(convert_tx);
    if convert_worker.join().is_err() {
        eprintln!("Conversion worker panicked");
    }
    eprintln!("Run mode stopped, wrote {} bytes", total_bytes);
    Ok(())
}

// Open a fresh timestamped UBX file and return the corresponding UTC hour bucket key.
fn open_new_log_file_for_time(
    data_dir: &Path,
    now: DateTime<Utc>,
) -> Result<(String, DateTime<Utc>, File, PathBuf)> {
    let hour_start = floor_to_hour(now);
    let hour_key = hour_start.format("%Y%m%d_%H").to_string();
    let file_name = format!("{}.ubx", now.format("%Y%m%d_%H%M%S"));
    let path = data_dir.join(file_name);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening log output failed: {}", path.display()))?;
    Ok((hour_key, hour_start, file, path))
}

// Truncate a DateTime to top-of-hour in UTC for deterministic hour bucket handling.
fn floor_to_hour(dt: DateTime<Utc>) -> DateTime<Utc> {
    dt.with_minute(0)
        .and_then(|v| v.with_second(0))
        .and_then(|v| v.with_nanosecond(0))
        .expect("UTC floor-to-hour should always be valid")
}

fn spawn_conversion_worker(
    convert_args: ConvertArgs,
    running: Arc<AtomicBool>,
) -> (Sender<DateTime<Utc>>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<DateTime<Utc>>();
    let handle = thread::spawn(move || conversion_worker_loop(convert_args, running, rx));
    (tx, handle)
}

fn conversion_worker_loop(
    convert_args: ConvertArgs,
    running: Arc<AtomicBool>,
    rx: Receiver<DateTime<Utc>>,
) {
    eprintln!("Conversion worker started");
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(hour) => convert_one_hour(&convert_args, hour),
            Err(RecvTimeoutError::Timeout) => {
                if !running.load(Ordering::SeqCst) {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    // Drain any enqueued jobs before exiting.
    while let Ok(hour) = rx.try_recv() {
        convert_one_hour(&convert_args, hour);
    }
    eprintln!("Conversion worker stopped");
}

fn convert_one_hour(convert_args: &ConvertArgs, hour: DateTime<Utc>) {
    let _lock = match LockGuard::acquire(&convert_args.lock_file) {
        Ok(lock) => lock,
        Err(err) => {
            eprintln!(
                "Conversion lock unavailable; skipped conversion for {}: {err:#}",
                hour.format("%Y-%m-%d %H:00")
            );
            return;
        }
    };

    if let Err(err) = ensure_converter_available(convert_args) {
        eprintln!(
            "Converter unavailable; skipped conversion for {}: {err:#}",
            hour.format("%Y-%m-%d %H:00")
        );
        return;
    }

    if let Err(err) = convert_hour_utc(convert_args, hour) {
        eprintln!(
            "Hour conversion failed for {} (logger continues): {err:#}",
            hour.format("%Y-%m-%d %H:00")
        );
    }
}

fn enqueue_startup_catchup_hours(args: &RunArgs, tx: &Sender<DateTime<Utc>>) -> usize {
    let total_hours = i64::from(args.max_days_back) * 24;
    if total_hours <= 0 {
        return 0;
    }

    let anchor = floor_to_hour(Utc::now() - ChronoDuration::hours(i64::from(args.shift_hours)));
    let mut enqueued = 0_usize;
    for offset in 0..total_hours {
        let hour = anchor - ChronoDuration::hours(offset);
        if tx.send(hour).is_err() {
            break;
        }
        enqueued += 1;
    }
    enqueued
}
