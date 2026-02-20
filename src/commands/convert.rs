use crate::args::ConvertArgs;
use crate::shared::lock::LockGuard;
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, Timelike, Utc};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

// Public convert command entrypoint. This scans recent UTC hours and builds compressed archive outputs.
pub fn run_convert(args: ConvertArgs) -> Result<()> {
    // Prepare directories and enforce single-instance conversion.
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

    // Anchor on the previous full UTC hour by default (shift_hours), then walk backwards.
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

// Convert one hour of UBX files into a single observation output + optional nav output, then archive.
fn process_hour(args: &ConvertArgs, dt: DateTime<Utc>, ubx_files: &[PathBuf]) -> Result<()> {
    let year = dt.format("%Y").to_string();
    let hour = dt.format("%H").to_string();
    let doy = format!("{:03}", dt.ordinal());
    let epoch_begin = format!("{}0000", dt.format("%Y-%m-%d_%H"));

    // Output naming follows the existing station/country/hour format.
    let obs_rnx_name = format!(
        "{}00{}_R_{}{}{}00_01H_01S_MO.rnx",
        args.station, args.country, year, doy, hour
    );
    let obs_rnx_path = args.data_dir.join(&obs_rnx_name);

    // Convert each UBX chunk into an OBS fragment.
    let mut obs_parts = Vec::with_capacity(ubx_files.len());
    for ubx in ubx_files {
        let obs_part = args
            .data_dir
            .join(format!("{}.obs", sanitize_stem_for_temp(ubx)?));
        run_convbin_obs(args, ubx, &obs_part)?;
        obs_parts.push(obs_part);
    }

    // Merge to an hourly RINEX observation file, then Hatanaka+gzip compress.
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

    // Optional navigation RINEX output for the same hour.
    let nav_gz_path = if args.skip_nav {
        None
    } else {
        Some(build_nav_output(args, dt, ubx_files)?)
    };

    // Clean temporary intermediate files in data_dir.
    for obs_part in &obs_parts {
        remove_file_if_exists(obs_part)?;
    }
    remove_file_if_exists(&obs_rnx_path)?;

    if !args.keep_ubx {
        for ubx in ubx_files {
            remove_file_if_exists(ubx)?;
        }
    }

    // Move final compressed products into archive/<year>/<doy>/.
    let archive_path = args.archive_dir.join(&year).join(&doy);
    fs::create_dir_all(&archive_path)
        .with_context(|| format!("creating archive path failed: {}", archive_path.display()))?;

    move_into_dir(&obs_gz_path, &archive_path)?;
    if let Some(nav_gz) = nav_gz_path {
        move_into_dir(&nav_gz, &archive_path)?;
    }

    Ok(())
}

// Build compressed navigation file for the hour by merging UBX and running convbin in nav mode.
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

// Wrapper for convbin observation conversion.
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

// Wrapper for convbin navigation conversion (`-n` output path).
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

// Merge observation fragments into one hourly file using gfzrnx.
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

// Convert RINEX observation to Hatanaka-compressed CRX.
fn run_rnx2crx(args: &ConvertArgs, obs_rnx: &Path) -> Result<()> {
    let mut cmd = Command::new(&args.rnx2crx_path);
    cmd.arg("-f").arg(obs_rnx);
    run_checked_command(
        &mut cmd,
        &format!("rnx2crx compression ({})", obs_rnx.display()),
    )
}

// Gzip any intermediate/final artifact path.
fn gzip_file(gzip_path: &Path, input: &Path) -> Result<()> {
    let mut cmd = Command::new(gzip_path);
    cmd.arg("-f").arg(input);
    run_checked_command(&mut cmd, &format!("gzip compression ({})", input.display()))
}

// Common external command runner with structured error reporting.
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

// Ensure source stem can be safely reused for temp filenames.
fn sanitize_stem_for_temp(path: &Path) -> Result<String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("invalid UTF-8 file name: {}", path.display()))?;
    Ok(stem.replace('/', "_"))
}

// Merge several UBX files into one byte stream.
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

// Best-effort delete helper used by temp cleanup paths.
fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("removing file failed: {}", path.display())),
    }
}

// Move a file into destination directory, with copy+delete fallback for cross-device moves.
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
