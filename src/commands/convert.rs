use crate::args::ConvertArgs;
use crate::shared::lock::LockGuard;
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, Timelike, Utc};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

// Public convert command entrypoint.
// This scans recent UTC hours, runs open-source `ubx2rinex`, and archives hourly outputs.
pub fn run_convert(args: ConvertArgs) -> Result<()> {
    // Prepare output folders and enforce single-instance conversion.
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
    let processed_hours = convert_recent_hours(&args, total_hours)?;
    eprintln!("Conversion complete; processed {} hour(s)", processed_hours);
    Ok(())
}

// Convert a recent UTC time window.
// This helper is shared by `convert` command and `run` startup catch-up logic.
pub(crate) fn convert_recent_hours(args: &ConvertArgs, total_hours: i64) -> Result<u32> {
    if total_hours <= 0 {
        bail!("max_days_back must be greater than zero");
    }

    ensure_converter_available(args)?;

    // Anchor on previous full UTC hour by default (shift_hours), then walk backwards.
    let anchor = floor_to_hour(Utc::now() - ChronoDuration::hours(i64::from(args.shift_hours)));

    let mut processed_hours = 0_u32;
    for offset in 0..total_hours {
        let dt = anchor - ChronoDuration::hours(offset);
        if convert_hour_utc(args, dt)? {
            processed_hours += 1;
        }
    }

    Ok(processed_hours)
}

// Convert one specific UTC hour if input UBX files are present.
pub(crate) fn convert_hour_utc(args: &ConvertArgs, dt: DateTime<Utc>) -> Result<bool> {
    let prefix = dt.format("%Y%m%d_%H").to_string();
    let ubx_files = list_hour_ubx_files(&args.data_dir, &prefix)?;
    if ubx_files.is_empty() {
        return Ok(false);
    }

    eprintln!(
        "Processing UTC hour {} with {} UBX file(s)",
        dt.format("%Y-%m-%d %H:00"),
        ubx_files.len()
    );

    process_hour(args, dt, &ubx_files)?;
    Ok(true)
}

// Convert one UTC hour of UBX files into OBS (+optional NAV) and archive.
fn process_hour(args: &ConvertArgs, dt: DateTime<Utc>, ubx_files: &[PathBuf]) -> Result<()> {
    let year = dt.format("%Y").to_string();
    let hour = dt.format("%H").to_string();
    let doy = format!("{:03}", dt.ordinal());
    let hour_prefix = format!(
        "{}00{}_R_{}{}{}",
        args.station, args.country, year, doy, hour
    );

    // Remove stale outputs for this hour so the produced files are unambiguous.
    remove_matching_hour_outputs(&args.data_dir, &hour_prefix)?;

    run_ubx2rinex_for_hour(args, ubx_files)?;

    let outputs = collect_hour_outputs(&args.data_dir, &hour_prefix)?;
    validate_hour_outputs(&outputs, args.skip_nav, &hour_prefix)?;

    // Move final outputs into archive/<year>/<doy>/.
    let archive_path = args.archive_dir.join(&year).join(&doy);
    fs::create_dir_all(&archive_path)
        .with_context(|| format!("creating archive path failed: {}", archive_path.display()))?;

    for output in &outputs {
        move_into_dir(output, &archive_path)?;
    }

    if !args.keep_ubx {
        for ubx in ubx_files {
            remove_file_if_exists(ubx)?;
        }
    }

    Ok(())
}

// Run open-source Rust converter in passive-file mode for one hour of UBX inputs.
fn run_ubx2rinex_for_hour(args: &ConvertArgs, ubx_files: &[PathBuf]) -> Result<()> {
    let station_name = format!("{}00", args.station);
    let data_prefix = args.data_dir.to_string_lossy().to_string();
    let (converter_program, used_path_fallback) = resolve_converter_program(&args.ubx2rinex_path);

    let mut cmd = Command::new(&converter_program);
    for ubx in ubx_files {
        cmd.arg("--file").arg(ubx);
    }

    cmd.arg("--name")
        .arg(&station_name)
        .arg("-c")
        .arg(&args.country)
        .arg("--long")
        .arg("--period")
        .arg("1 h")
        .arg("--sampling")
        .arg("1 s")
        .arg("--crx")
        .arg("--gzip")
        .arg("--prefix")
        .arg(data_prefix)
        .arg("--model")
        .arg(&args.receiver_type)
        .arg("--antenna")
        .arg(&args.antenna_type)
        .arg("--observer")
        .arg(&args.observer);

    if !args.skip_nav {
        cmd.arg("--nav");
    }

    let label = if used_path_fallback {
        format!(
            "ubx2rinex conversion (requested {} not found; used PATH lookup)",
            args.ubx2rinex_path.display()
        )
    } else {
        "ubx2rinex conversion".to_string()
    };

    run_checked_command(&mut cmd, &label)
}

// Verify `ubx2rinex` binary exists and can be executed.
pub(crate) fn ensure_converter_available(args: &ConvertArgs) -> Result<()> {
    let (converter_program, used_path_fallback) = resolve_converter_program(&args.ubx2rinex_path);
    let mut cmd = Command::new(&converter_program);
    cmd.arg("--version");
    run_checked_command(
        &mut cmd,
        &if used_path_fallback {
            format!(
                "ubx2rinex availability check (requested {} not found; used PATH lookup)",
                args.ubx2rinex_path.display()
            )
        } else {
            format!(
                "ubx2rinex availability check ({})",
                args.ubx2rinex_path.display()
            )
        },
    )
}

// Resolve converter executable path.
// If configured absolute path is missing, fall back to PATH lookup for NixOS/non-Debian layouts.
fn resolve_converter_program(configured_path: &Path) -> (OsString, bool) {
    if configured_path.exists() {
        return (configured_path.as_os_str().to_owned(), false);
    }
    (OsString::from("ubx2rinex"), true)
}

// Remove existing outputs that match this hour pattern.
fn remove_matching_hour_outputs(data_dir: &Path, hour_prefix: &str) -> Result<()> {
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

        let Some(name) = entry.file_name().to_str().map(|v| v.to_string()) else {
            continue;
        };
        if name.starts_with(hour_prefix) && is_output_product_name(&name) {
            remove_file_if_exists(&entry.path())?;
        }
    }
    Ok(())
}

// Collect outputs created for a given hour prefix.
fn collect_hour_outputs(data_dir: &Path, hour_prefix: &str) -> Result<Vec<PathBuf>> {
    let mut outputs = Vec::new();
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
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with(hour_prefix) && is_output_product_name(name) {
            outputs.push(path);
        }
    }
    outputs.sort();
    Ok(outputs)
}

// Validate required products were created.
fn validate_hour_outputs(outputs: &[PathBuf], skip_nav: bool, hour_prefix: &str) -> Result<()> {
    let has_obs = outputs.iter().any(|path| {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.ends_with(".crx.gz"))
    });
    if !has_obs {
        bail!("no observation product generated for hour prefix {hour_prefix}");
    }

    if !skip_nav {
        let has_nav = outputs.iter().any(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.contains("_MN.") && name.ends_with(".rnx.gz"))
        });
        if !has_nav {
            bail!("no navigation product generated for hour prefix {hour_prefix}");
        }
    }

    Ok(())
}

// True if file is one of the final products we archive.
fn is_output_product_name(name: &str) -> bool {
    name.ends_with(".crx.gz") || name.ends_with(".rnx.gz")
}

// Run external command and include stdout/stderr when failing.
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

// List UBX files in data_dir that belong to a UTC hour prefix (YYYYMMDD_HH...).
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

// Best-effort delete helper used by cleanup paths.
fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("removing file failed: {}", path.display())),
    }
}

// Move file into destination directory, with copy+delete fallback for cross-device moves.
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

// Truncate a DateTime to top-of-hour in UTC for deterministic hourly windowing.
fn floor_to_hour(dt: DateTime<Utc>) -> DateTime<Utc> {
    dt.with_minute(0)
        .and_then(|v| v.with_second(0))
        .and_then(|v| v.with_nanosecond(0))
        .expect("UTC floor-to-hour should always be valid")
}
