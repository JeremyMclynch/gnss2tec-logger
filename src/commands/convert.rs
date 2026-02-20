use crate::args::{ConvertArgs, NavOutputFormat, ObsOutputFormat};
use crate::shared::lock::LockGuard;
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, Timelike, Utc};
use flate2::Compression;
use flate2::write::GzEncoder;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
use tar::Builder;

// Public convert command entrypoint.
// This scans recent UTC hours, runs conversion, and archives hourly outputs.
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
    let nav_requested = !args.skip_nav;

    // Run conversion in an isolated output workspace to avoid name-matching assumptions.
    let work_dir = create_conversion_workspace(&args.data_dir, dt)?;
    let _workspace_cleanup = WorkspaceCleanup::new(work_dir.clone());
    let data_dir_snapshot_before = snapshot_output_products(&args.data_dir)?;

    let conversion_result: Result<Vec<PathBuf>> = (|| {
        let merged_ubx = work_dir.join(format!("merged_{}.ubx", dt.format("%Y%m%d_%H")));
        concat_ubx_files(ubx_files, &merged_ubx)?;

        run_convbin_obs_for_hour(args, dt, &merged_ubx, &work_dir)?;
        if nav_requested {
            run_convbin_nav_for_hour(args, dt, &merged_ubx, &work_dir)?;
        }

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

        normalize_long_output_names_for_target_hour(&mut outputs, dt)?;
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

// Verify required converter binaries exist and can be executed.
pub(crate) fn ensure_converter_available(args: &ConvertArgs) -> Result<()> {
    if args.obs_sampling_secs == 0 {
        bail!("obs_sampling_secs must be greater than zero");
    }

    if !matches!(args.obs_output_format, ObsOutputFormat::Rinex) {
        bail!(
            "unsupported observation output format {:?}; convbin pipeline supports only `rinex`",
            args.obs_output_format
        );
    }

    let (program, used_path_fallback) = resolve_convbin_program(&args.convbin_path);
    let mut cmd = Command::new(&program);
    cmd.arg("-h");
    run_checked_command(
        &mut cmd,
        &if used_path_fallback {
            format!(
                "convbin availability check (requested {} not found; used PATH lookup)",
                args.convbin_path.display()
            )
        } else {
            format!(
                "convbin availability check ({})",
                args.convbin_path.display()
            )
        },
    )
}

// Resolve convbin executable path.
// If configured absolute path is missing, fall back to PATH lookup.
fn resolve_convbin_program(configured_path: &Path) -> (OsString, bool) {
    if configured_path.exists() {
        return (configured_path.as_os_str().to_owned(), false);
    }
    (OsString::from("convbin"), true)
}

#[derive(Clone, Copy)]
struct NavSystemSpec {
    suffix: &'static str,
    exclude: &'static [char],
}

const NAV_SYSTEM_SPECS: [NavSystemSpec; 5] = [
    NavSystemSpec {
        suffix: "GN",
        exclude: &['R', 'E', 'J', 'S', 'C'],
    },
    NavSystemSpec {
        suffix: "RN",
        exclude: &['G', 'E', 'J', 'S', 'C'],
    },
    NavSystemSpec {
        suffix: "EN",
        exclude: &['G', 'R', 'J', 'S', 'C'],
    },
    NavSystemSpec {
        suffix: "CN",
        exclude: &['G', 'R', 'E', 'J', 'S'],
    },
    NavSystemSpec {
        suffix: "JN",
        exclude: &['G', 'R', 'E', 'S', 'C'],
    },
];

fn run_convbin_obs_for_hour(
    args: &ConvertArgs,
    dt: DateTime<Utc>,
    merged_ubx: &Path,
    output_dir: &Path,
) -> Result<()> {
    if args.obs_sampling_secs == 0 {
        bail!("obs_sampling_secs must be greater than zero");
    }

    let (program, used_path_fallback) = resolve_convbin_program(&args.convbin_path);
    let prefix = format!(
        "{}00{}_R_{}{:03}{}_01H_{}_MO",
        args.station,
        args.country,
        dt.format("%Y"),
        dt.ordinal(),
        dt.format("%H"),
        sampling_token_from_seconds(args.obs_sampling_secs)
    );
    let obs_rnx = output_dir.join(format!("{prefix}.rnx"));

    let mut cmd = Command::new(&program);
    cmd.arg("-r")
        .arg("ubx")
        .arg("-v")
        .arg("3.04")
        .arg("-ti")
        .arg(args.obs_sampling_secs.to_string())
        .arg("-hm")
        .arg(format!("{}00", args.station))
        .arg("-ho")
        .arg(format!("{}/{}", args.observer, args.country))
        .arg("-hr")
        .arg(format!("NA/{}/NA", args.receiver_type))
        .arg("-ha")
        .arg(format!("NA/{}", args.antenna_type))
        .arg("-o")
        .arg(&obs_rnx)
        .arg(merged_ubx);

    let label = if used_path_fallback {
        format!(
            "convbin observation conversion (requested {} not found; used PATH lookup)",
            args.convbin_path.display()
        )
    } else {
        "convbin observation conversion".to_string()
    };

    run_checked_command(&mut cmd, &label)?;

    if !file_exists_and_nonempty(&obs_rnx) {
        bail!(
            "convbin finished but expected observation file was not generated: {}",
            obs_rnx.display()
        );
    }

    let _ = gzip_file(obs_rnx)?;
    Ok(())
}

fn run_convbin_nav_for_hour(
    args: &ConvertArgs,
    dt: DateTime<Utc>,
    merged_ubx: &Path,
    output_dir: &Path,
) -> Result<()> {
    let (program, used_path_fallback) = resolve_convbin_program(&args.convbin_path);
    let prefix = format!(
        "{}00{}_R_{}{:03}{}_01H",
        args.station,
        args.country,
        dt.format("%Y"),
        dt.ordinal(),
        dt.format("%H")
    );

    match args.nav_output_format {
        NavOutputFormat::Mixed => {
            let nav_rnx = output_dir.join(format!("{prefix}_MN.rnx"));
            run_convbin_nav_command(
                args,
                &program,
                used_path_fallback,
                &merged_ubx,
                &nav_rnx,
                &[],
                "mixed",
            )?;

            if !file_exists_and_nonempty(&nav_rnx) {
                bail!(
                    "convbin finished but expected mixed NAV file was not generated: {}",
                    nav_rnx.display()
                );
            }
            let _ = gzip_file(nav_rnx)?;
        }
        NavOutputFormat::IndividualTarGz => {
            let mut produced = Vec::new();

            for spec in NAV_SYSTEM_SPECS {
                let nav_rnx = output_dir.join(format!("{prefix}_{}.rnx", spec.suffix));
                let label = format!("constellation {}", spec.suffix);
                if let Err(err) = run_convbin_nav_command(
                    args,
                    &program,
                    used_path_fallback,
                    &merged_ubx,
                    &nav_rnx,
                    spec.exclude,
                    &label,
                ) {
                    eprintln!(
                        "convbin NAV generation skipped for {}: {err:#}",
                        spec.suffix
                    );
                    remove_file_if_exists(&nav_rnx)?;
                    continue;
                }

                if file_exists_and_nonempty(&nav_rnx) {
                    produced.push(nav_rnx);
                } else {
                    remove_file_if_exists(&nav_rnx)?;
                }
            }

            if produced.is_empty() {
                bail!(
                    "no per-constellation NAV files were generated for hour {}",
                    dt.format("%Y-%m-%d %H:00")
                );
            }

            let archive = output_dir.join(format!("{prefix}_NAVSET.tar.gz"));
            bundle_files_into_tar_gz(&produced, &archive)?;
            for path in produced {
                remove_file_if_exists(&path)?;
            }
        }
    }

    Ok(())
}

fn run_convbin_nav_command(
    args: &ConvertArgs,
    program: &OsString,
    used_path_fallback: bool,
    merged_ubx: &Path,
    output_nav: &Path,
    exclude_systems: &[char],
    mode_label: &str,
) -> Result<()> {
    let mut cmd = Command::new(program);
    cmd.arg("-r")
        .arg("ubx")
        .arg("-v")
        .arg("3.04")
        .arg("-hm")
        .arg(format!("{}00", args.station))
        .arg("-ho")
        .arg(format!("{}/{}", args.observer, args.country))
        .arg("-hr")
        .arg(format!("NA/{}/NA", args.receiver_type))
        .arg("-ha")
        .arg(format!("NA/{}", args.antenna_type));

    for sys in exclude_systems {
        cmd.arg("-y").arg(sys.to_string());
    }

    cmd.arg("-n").arg(output_nav).arg(merged_ubx);

    let label = if used_path_fallback {
        format!(
            "convbin navigation conversion ({mode_label}, requested {} not found; used PATH lookup)",
            args.convbin_path.display()
        )
    } else {
        format!("convbin navigation conversion ({mode_label})")
    };

    run_checked_command(&mut cmd, &label)
}

fn file_exists_and_nonempty(path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.len() > 0,
        Err(_) => false,
    }
}

fn concat_ubx_files(inputs: &[PathBuf], output: &Path) -> Result<()> {
    let mut writer = BufWriter::new(File::create(output).with_context(|| {
        format!(
            "creating temporary UBX merge file failed: {}",
            output.display()
        )
    })?);

    for input in inputs {
        let mut reader = BufReader::new(
            File::open(input)
                .with_context(|| format!("opening UBX input failed: {}", input.display()))?,
        );
        io::copy(&mut reader, &mut writer).with_context(|| {
            format!(
                "appending UBX input into temporary merge file failed: {}",
                input.display()
            )
        })?;
    }
    writer.flush().with_context(|| {
        format!(
            "flushing temporary UBX merge file failed: {}",
            output.display()
        )
    })?;
    Ok(())
}

fn gzip_file(path: PathBuf) -> Result<PathBuf> {
    let gz_path = PathBuf::from(format!("{}.gz", path.display()));
    let mut input = BufReader::new(
        File::open(&path)
            .with_context(|| format!("opening file for gzip failed: {}", path.display()))?,
    );
    let out_file = File::create(&gz_path)
        .with_context(|| format!("creating gzip output failed: {}", gz_path.display()))?;
    let writer = BufWriter::new(out_file);
    let mut encoder = GzEncoder::new(writer, Compression::default());
    io::copy(&mut input, &mut encoder)
        .with_context(|| format!("gzip compression failed: {}", path.display()))?;
    let mut writer = encoder
        .finish()
        .with_context(|| format!("finalizing gzip output failed: {}", gz_path.display()))?;
    writer
        .flush()
        .with_context(|| format!("flushing gzip output failed: {}", gz_path.display()))?;
    remove_file_if_exists(&path)?;
    Ok(gz_path)
}

fn sampling_token_from_seconds(seconds: u32) -> String {
    if seconds < 100 {
        format!("{seconds:02}S")
    } else {
        format!("{seconds}S")
    }
}

fn bundle_files_into_tar_gz(files: &[PathBuf], archive_path: &Path) -> Result<()> {
    let out = File::create(archive_path).with_context(|| {
        format!(
            "creating navigation archive failed: {}",
            archive_path.display()
        )
    })?;
    let writer = BufWriter::new(out);
    let encoder = GzEncoder::new(writer, Compression::default());
    let mut tar = Builder::new(encoder);

    for path in files {
        let Some(name) = path.file_name() else {
            bail!("missing file name for NAV file: {}", path.display());
        };
        tar.append_path_with_name(path, Path::new(name))
            .with_context(|| {
                format!("adding NAV file to tar archive failed: {}", path.display())
            })?;
    }

    let encoder = tar
        .into_inner()
        .with_context(|| format!("finalizing tar stream failed: {}", archive_path.display()))?;
    let mut writer = encoder
        .finish()
        .with_context(|| format!("finalizing gzip stream failed: {}", archive_path.display()))?;
    writer
        .flush()
        .with_context(|| format!("flushing archive failed: {}", archive_path.display()))?;
    Ok(())
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

// Identify product kind across multiple RINEX naming styles.
fn classify_output_name(name: &str) -> OutputKind {
    let lower = name.to_ascii_lowercase();

    if lower.contains("_navset.tar.gz") {
        return OutputKind::Navigation;
    }

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

// Some converter outputs can emit long-name epoch tokens with HHMM fixed to 0000.
// Normalize those product names to the target conversion hour to avoid archive collisions.
fn normalize_long_output_names_for_target_hour(
    outputs: &mut Vec<PathBuf>,
    dt: DateTime<Utc>,
) -> Result<()> {
    let target_epoch = format!(
        "{}{:03}{}00",
        dt.format("%Y"),
        dt.ordinal(),
        dt.format("%H")
    );

    for path in outputs.iter_mut() {
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(normalized_name) = rewrite_long_name_epoch(file_name, &target_epoch) else {
            continue;
        };

        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("missing parent directory for {}", path.display()))?;
        let destination = unique_destination_path(parent, OsStr::new(&normalized_name));
        let source = path.clone();

        fs::rename(&source, &destination).with_context(|| {
            format!(
                "renaming output product failed: {} -> {}",
                source.display(),
                destination.display()
            )
        })?;
        *path = destination;
    }

    outputs.sort();
    Ok(())
}

// Rewrite long-name `_R_YYYYDOYHHMM_` epoch segment to a specific hour.
fn rewrite_long_name_epoch(file_name: &str, target_epoch: &str) -> Option<String> {
    let marker = "_R_";
    let start = file_name.find(marker)? + marker.len();
    let remaining = &file_name[start..];
    let epoch_end_rel = remaining.find('_')?;
    let epoch = &remaining[..epoch_end_rel];

    if epoch.len() != 11 || !epoch.chars().all(|c| c.is_ascii_digit()) || epoch == target_epoch {
        return None;
    }

    let mut rewritten = String::with_capacity(file_name.len());
    rewritten.push_str(&file_name[..start]);
    rewritten.push_str(target_epoch);
    rewritten.push_str(&remaining[epoch_end_rel..]);
    Some(rewritten)
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

// Return a non-colliding destination path within one directory.
fn unique_destination_path(dst_dir: &Path, file_name: &OsStr) -> PathBuf {
    let first_try = dst_dir.join(file_name);
    if !first_try.exists() {
        return first_try;
    }

    let base = file_name.to_string_lossy();
    for idx in 1.. {
        let candidate = dst_dir.join(format!("{base}.dup{idx}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    unreachable!("duplicate suffix search should always find an unused path");
}

// Move file into destination directory, with copy+delete fallback for cross-device moves.
fn move_into_dir(src: &Path, dst_dir: &Path) -> Result<PathBuf> {
    let file_name = src
        .file_name()
        .ok_or_else(|| anyhow!("missing file name for source: {}", src.display()))?;
    let dst = unique_destination_path(dst_dir, file_name);

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
