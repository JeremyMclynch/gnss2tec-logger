# gnss2tec-logger

Rust-based GNSS data logger and converter pipeline for u-blox receivers.

This program:

- sends UBX configuration commands (from `ubx.dat`) to a receiver on a serial port
- logs raw UBX binary data into hourly files
- converts closed hours into compressed RINEX products (`.crx.gz`, optional nav `.rnx.gz`)
- archives products by `year/day-of-year`
- can run continuously as a `systemd` service at boot

## Why this exists

The goal is to replace shell-script orchestration with a single Rust application that is easier to deploy and maintain, while keeping the same practical outcome:

- continuous UBX logging
- hourly RINEX conversion
- compression + archive organization

## Current architecture

- CLI + orchestration: Rust (`clap`, `anyhow`, `chrono`)
- serial access: `serialport`
- UBX packet building from config commands: `ublox` crate
- lock files to prevent duplicate instances: `fs2` file locks
- UBX -> RINEX conversion: `ubx2rinex` executable (open source, bundled in `.deb`)

## Repository layout

- `src/main.rs`: CLI parse + command dispatch
- `src/args.rs`: all command-line argument definitions/defaults
- `src/commands/log.rs`: receiver config + UBX logging
- `src/commands/convert.rs`: hourly UBX -> RINEX conversion + archive + cleanup
- `src/commands/run.rs`: continuous mode (logging + automatic hourly conversion)
- `src/shared/lock.rs`: process lock guard
- `src/shared/signal.rs`: Ctrl-C shutdown signal handling
- `packaging/`: systemd unit, default config, Debian maintainer scripts
- `scripts/build-deb.sh`: `.deb` packager (also bundles `ubx2rinex`)

## Default runtime paths

- config file: `/etc/gnss2tec-logger/ubx.dat`
- data directory: `/var/lib/gnss2tec-logger/data`
- archive directory: `/var/lib/gnss2tec-logger/archive`
- bundled converter path: `/usr/lib/gnss2tec-logger/bin/ubx2rinex`

## Installation

### Option 1: Build and run from source

1. Build:

```bash
cargo build --release
```

2. Run continuously:

```bash
./target/release/gnss2tec-logger run
```

Note: in source mode, ensure `ubx2rinex` exists and pass `--ubx2rinex-path` if needed.

### Option 2: Build a Debian package (`.deb`)

Build package:

```bash
./scripts/build-deb.sh
```

Result:

- `dist/gnss2tec-logger_<version>_amd64.deb` (on x86_64 hosts)

ARM64 package build:

```bash
./scripts/build-deb.sh --target aarch64-unknown-linux-gnu --deb-arch arm64
```

If cross-building from x86_64, install linker first:

```bash
sudo apt install gcc-aarch64-linux-gnu
```

Install package:

```bash
sudo dpkg -i dist/gnss2tec-logger_<version>_<arch>.deb
```

What the package installs:

- `/usr/bin/gnss2tec-logger`
- `/usr/lib/gnss2tec-logger/bin/ubx2rinex` (bundled, open-source)
- `/etc/gnss2tec-logger/ubx.dat`
- `/lib/systemd/system/gnss2tec-logger.service`

## systemd service (automatic startup)

Service name: `gnss2tec-logger.service`

- runs as `root`
- starts at boot (`multi-user.target`)
- always restarts on failure

Useful commands:

```bash
sudo systemctl status gnss2tec-logger.service
sudo journalctl -u gnss2tec-logger.service -f
sudo systemctl restart gnss2tec-logger.service
```

## Data retention and uninstall behavior

Runtime data is intentionally stored under `/var/lib/gnss2tec-logger` so it is not treated like temporary/cache content.

- `dpkg -r gnss2tec-logger`: removes package/service, keeps data
- `dpkg --purge gnss2tec-logger`: purges package config, still keeps data

## CLI modes

- `log`: configure receiver + log UBX only
- `convert`: convert existing UBX files into archived RINEX products
- `run`: single-process continuous mode (recommended), does both logging and hourly conversion

See available options:

```bash
gnss2tec-logger --help
gnss2tec-logger run --help
```

## Simplified execution state machines

### 1) App entry (`src/main.rs`)

`START -> parse CLI -> dispatch command -> command loop/exit -> END`

- `log` dispatches to `run_log`
- `convert` dispatches to `run_convert`
- `run` dispatches to `run_mode`

### 2) Log command (`src/commands/log.rs`)

`INIT`
-> `create data dir`
-> `acquire lock`
-> `parse ubx.dat`
-> `open serial port`
-> `send UBX config packets`
-> `open current hour file`
-> `READ LOOP`

`READ LOOP` does:

- read serial bytes
- append to active `.ubx`
- periodic flush
- detect UTC hour rollover -> flush + rotate to new file
- stop on signal

Then:

`final flush -> release lock -> EXIT`

### 3) Convert command (`src/commands/convert.rs`)

`INIT`
-> `create data/archive dirs`
-> `acquire lock`
-> `check ubx2rinex availability`
-> `for each target hour in window`
-> `find hour UBX files`
-> if no UBX files for that hour: `skip hour`
-> if UBX files exist: `clear stale outputs`
-> if UBX files exist: `call ubx2rinex`
-> if UBX files exist: `validate outputs (.crx.gz, optional nav)`
-> if UBX files exist: `archive outputs to archive/<year>/<doy>/`
-> if UBX files exist: `delete source .ubx (unless --keep-ubx)`

Then:

`release lock -> EXIT`

### 4) Run command (`src/commands/run.rs`) (recommended)

`INIT`
-> `create data/archive dirs`
-> `parse ubx.dat`
-> `open serial`
-> `send UBX config packets`
-> `check converter availability`
-> optional startup catch-up conversion
-> `open current hour file`
-> `MAIN LOOP`

`MAIN LOOP` does:

- read serial bytes and write to active `.ubx`
- periodic flush
- on UTC hour rollover: close previous hour file
- on UTC hour rollover: attempt conversion of the just-closed hour
- on UTC hour rollover: if conversion fails, log error and continue logging
- on UTC hour rollover: open new hour file
- stop on signal

Then:

`final flush -> EXIT`

### 5) Shared utilities

- `src/shared/lock.rs`: file-based exclusive lock guard for single-instance protection
- `src/shared/signal.rs`: installs Ctrl-C handler and exposes shared run flag for graceful shutdown

## Operational notes

- Device default is `/dev/ttyACM0`; override with `--serial-port` if needed.
- Hour boundaries are based on UTC.
- The bundled `ubx2rinex` is open source and built from crates.io source during package build.
- For unattended production use, prefer `systemd` service + `.deb` install.
