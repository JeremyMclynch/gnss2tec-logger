use crate::args::ConvertArgs;
use crate::shared::lock::LockGuard;
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, Timelike, Utc};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

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
    let doy = format!("{:03}", dt.ordinal());
    let hour_label = format!("{} {}", dt.format("%Y-%m-%d"), dt.format("%H:00"));

    // Run conversion in an isolated output workspace to avoid name-matching assumptions.
    let work_dir = create_conversion_workspace(&args.data_dir, dt)?;
    let _workspace_cleanup = WorkspaceCleanup::new(work_dir.clone());
    let data_dir_snapshot_before = snapshot_output_products(&args.data_dir)?;

    let conversion_result: Result<Vec<PathBuf>> = (|| {
        run_ubx2rinex_for_hour(args, ubx_files, &work_dir)?;

        let mut outputs = collect_output_products_in_dir(&work_dir)?;
        if outputs.is_empty() {
            // Fallback for converter layouts that still emit into data_dir.
            outputs = collect_changed_output_products(
                &data_dir_snapshot_before,
                &snapshot_output_products(&args.data_dir)?,
            );
            if !outputs.is_empty() {
                eprintln!(
                    "Converter emitted products outside workspace for {}; using changed files from {}",
                    hour_label,
                    args.data_dir.display()
                );
            }
        }

        validate_hour_outputs(&outputs, args.skip_nav, &hour_label)?;
        Ok(outputs)
    })();

    let outputs = match conversion_result {
        Ok(outputs) => outputs,
        Err(err) => return Err(err),
    };

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
fn run_ubx2rinex_for_hour(
    args: &ConvertArgs,
    ubx_files: &[PathBuf],
    output_prefix_dir: &Path,
) -> Result<()> {
    let station_name = format!("{}00", args.station);
    let output_prefix = output_prefix_dir.to_string_lossy().to_string();
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
        .arg(output_prefix)
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

// Collect final output product files in one directory.
fn collect_output_products_in_dir(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut outputs = Vec::new();
    for entry in
        fs::read_dir(dir).with_context(|| format!("reading directory failed: {}", dir.display()))?
    {
        let entry = entry.with_context(|| format!("iterating {}", dir.display()))?;
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
        if is_output_product_name(name) {
            outputs.push(path);
        }
    }
    outputs.sort();
    Ok(outputs)
}

// Validate required products were created.
fn validate_hour_outputs(outputs: &[PathBuf], skip_nav: bool, label: &str) -> Result<()> {
    let mut has_obs = false;
    let mut has_nav = false;
    let mut names = Vec::new();

    for path in outputs {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        names.push(name.to_string());
        match classify_output_name(name) {
            OutputKind::Observation => has_obs = true,
            OutputKind::Navigation => has_nav = true,
            OutputKind::Other => {}
        }
    }

    if !has_obs {
        bail!(
            "no observation product generated for {label}; collected outputs: {}",
            names.join(", ")
        );
    }

    if !skip_nav {
        if !has_nav {
            bail!(
                "no navigation product generated for {label}; collected outputs: {}",
                names.join(", ")
            );
        }
    }

    Ok(())
}

// True if file is one of the final products we archive.
fn is_output_product_name(name: &str) -> bool {
    !matches!(classify_output_name(name), OutputKind::Other)
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum OutputKind {
    Observation,
    Navigation,
    Other,
}

// Identify product kind across multiple ubx2rinex naming styles.
fn classify_output_name(name: &str) -> OutputKind {
    let lower = name.to_ascii_lowercase();

    // RINEX v3 long names.
    if lower.contains("_mn.") {
        return OutputKind::Navigation;
    }
    if lower.contains("_mo.") {
        return OutputKind::Observation;
    }

    // Compression driven extension style.
    if lower.ends_with(".crx") || lower.ends_with(".crx.gz") {
        return OutputKind::Observation;
    }
    if lower.ends_with(".rnx") || lower.ends_with(".rnx.gz") {
        // If kind is ambiguous, treat as observation to avoid false-negative failures.
        return OutputKind::Observation;
    }

    // RINEX v2 short names (e.g. ".26o", ".26d", ".26n"), optionally gzip-compressed.
    classify_rinex2_short_kind(&lower).unwrap_or(OutputKind::Other)
}

fn classify_rinex2_short_kind(lower_name: &str) -> Option<OutputKind> {
    let trimmed = lower_name.strip_suffix(".gz").unwrap_or(lower_name);
    let ext = trimmed.rsplit('.').next()?;
    let kind = ext.chars().last()?;
    match kind {
        'o' | 'd' => Some(OutputKind::Observation),
        'n' | 'g' | 'l' | 'p' | 'q' => Some(OutputKind::Navigation),
        _ => None,
    }
}

#[derive(Clone)]
struct ProductSnapshot {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
}

fn snapshot_output_products(dir: &Path) -> Result<Vec<ProductSnapshot>> {
    let mut out = Vec::new();
    for entry in
        fs::read_dir(dir).with_context(|| format!("reading directory failed: {}", dir.display()))?
    {
        let entry = entry.with_context(|| format!("iterating {}", dir.display()))?;
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
        if !is_output_product_name(name) {
            continue;
        }

        let meta = fs::metadata(&path)
            .with_context(|| format!("reading metadata failed: {}", path.display()))?;
        out.push(ProductSnapshot {
            path,
            modified: meta.modified().ok(),
            len: meta.len(),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn collect_changed_output_products(
    before: &[ProductSnapshot],
    after: &[ProductSnapshot],
) -> Vec<PathBuf> {
    let mut before_map = HashMap::new();
    for snap in before {
        before_map.insert(snap.path.clone(), (snap.modified, snap.len));
    }

    let mut changed = Vec::new();
    for snap in after {
        match before_map.get(&snap.path) {
            None => changed.push(snap.path.clone()),
            Some((prev_modified, prev_len))
                if prev_modified != &snap.modified || prev_len != &snap.len =>
            {
                changed.push(snap.path.clone());
            }
            _ => {}
        }
    }
    changed.sort();
    changed
}

fn create_conversion_workspace(data_dir: &Path, dt: DateTime<Utc>) -> Result<PathBuf> {
    let base = data_dir.join(".convert-work");
    fs::create_dir_all(&base)
        .with_context(|| format!("creating conversion workspace failed: {}", base.display()))?;
    let name = format!(
        "{}_{}_{}",
        dt.format("%Y%m%d_%H"),
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let path = base.join(name);
    fs::create_dir_all(&path)
        .with_context(|| format!("creating hour workspace failed: {}", path.display()))?;
    Ok(path)
}

struct WorkspaceCleanup {
    path: PathBuf,
}

impl WorkspaceCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for WorkspaceCleanup {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_dir_all(&self.path)
            && err.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "cleanup warning: failed to remove conversion workspace {}: {}",
                self.path.display(),
                err
            );
        }
    }
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
