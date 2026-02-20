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
- UBX -> RINEX conversion:
  - `ubx2rinex` for observation products (bundled in `.deb`)
  - `convbin` (RTKLIB) for multi-constellation navigation fallback (bundled in `.deb`)

## Repository layout

- `src/main.rs`: CLI parse + command dispatch
- `src/args.rs`: all command-line argument definitions/defaults
- `src/commands/log.rs`: receiver config + UBX logging
- `src/commands/convert.rs`: hourly UBX -> RINEX conversion + archive + cleanup
- `src/commands/run.rs`: continuous mode (logging + automatic hourly conversion)
- `src/shared/lock.rs`: process lock guard
- `src/shared/signal.rs`: Ctrl-C shutdown signal handling
- `packaging/`: systemd unit, default config, Debian maintainer scripts
- `scripts/build-deb.sh`: `.deb` packager (bundles `ubx2rinex` + `convbin`)
- `flake.nix`: flake outputs for package/devShell/module
- `nix/package.nix`: reusable Nix package definition
- `nix/module.nix`: NixOS module (`services.gnss2tec-logger`)

## Default runtime paths

- config file: `/etc/gnss2tec-logger/ubx.dat`
- data directory: `/var/lib/gnss2tec-logger/data`
- archive directory: `/var/lib/gnss2tec-logger/archive`
- bundled converter paths:
  - `/usr/lib/gnss2tec-logger/bin/ubx2rinex`
  - `/usr/lib/gnss2tec-logger/bin/convbin`

## Installation

Install using a prebuilt Debian package file.

1. Confirm architecture:

```bash
dpkg --print-architecture
```

2. Install the matching package:

```bash
sudo dpkg -i gnss2tec-logger_<version>_<arch>.deb
```

3. If `dpkg` reports missing dependencies, fix them:

```bash
sudo apt-get -f install
```

4. Verify service startup:

```bash
sudo systemctl status gnss2tec-logger.service
```

Common architectures:

- `amd64` for x86_64 systems
- `arm64` for aarch64 systems

Optional: update receiver config before first run:

```bash
sudoedit /etc/gnss2tec-logger/ubx.dat
sudo systemctl restart gnss2tec-logger.service
```

Default packaged `ubx.dat` enables the NMEA sentences required for status logging:
`GSA`, `GSV`, `GNS`, `RMC`, `GBS`, `GST`.

Runtime options can be configured without editing the unit file:

```bash
sudoedit /etc/gnss2tec-logger/runtime.env
sudo systemctl restart gnss2tec-logger.service
```

The service reads this file via `EnvironmentFile` and maps variables to `gnss2tec-logger run` options.

Startup behavior:

- service waits for GNSS serial device(s) before launching the logger
- default wait pattern: `/dev/ttyACM*`
- if `GNSS2TEC_SERIAL_PORT` is set, that path is preferred
- `GNSS2TEC_SERIAL_WAIT_TIMEOUT_SECS=0` means wait forever

What the package installs:

- `/usr/bin/gnss2tec-logger`
- `/usr/lib/gnss2tec-logger/bin/ubx2rinex` (bundled, open-source)
- `/usr/lib/gnss2tec-logger/bin/convbin` (bundled RTKLIB, open-source)
- `/etc/gnss2tec-logger/ubx.dat`
- `/etc/gnss2tec-logger/runtime.env`
- `/lib/systemd/system/gnss2tec-logger.service`

## NixOS / Flake Installation

This repository now provides:

- a flake package (`packages.<system>.default`)
- a NixOS module (`nixosModules.default`)

### NixOS flake usage

In your system flake, add this repository as an input and import the module:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.11";
    gnss2tec-logger.url = "github:<owner>/gnss2tec-logger";
  };

  outputs = { self, nixpkgs, gnss2tec-logger, ... }: {
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
      system = "aarch64-linux";
      modules = [
        gnss2tec-logger.nixosModules.default
        {
          services.gnss2tec-logger = {
            enable = true;
            serialPort = "/dev/ttyACM0";
            # Optional: override converter path if needed.
            # ubx2rinexPath = "/run/current-system/sw/bin/ubx2rinex";
            # convbinPath = "/run/current-system/sw/bin/convbin";
          };
        }
      ];
    };
  };
}
```

Then deploy:

```bash
sudo nixos-rebuild switch --flake .#my-host
```

### Complete NixOS host example

```nix
{
  description = "NixOS host with gnss2tec-logger";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.11";
    gnss2tec-logger.url = "github:<owner>/gnss2tec-logger";
  };

  outputs = { self, nixpkgs, gnss2tec-logger, ... }:
  let
    system = "aarch64-linux";
  in
  {
    nixosConfigurations.gnss-node = nixpkgs.lib.nixosSystem {
      inherit system;
      modules = [
        ./hardware-configuration.nix
        gnss2tec-logger.nixosModules.default
        ({ pkgs, ... }: {
          services.gnss2tec-logger = {
            enable = true;
            serialPort = "/dev/ttyACM0";
            baudRate = 115200;
            dataDir = "/var/lib/gnss2tec-logger/data";
            archiveDir = "/var/lib/gnss2tec-logger/archive";
            ubx2rinexPath = "${pkgs.ubx2rinex}/bin/ubx2rinex";
            convbinPath = "${pkgs.rtklib}/bin/convbin";
          };
        })
      ];
    };
  };
}
```

Apply it:

```bash
sudo nixos-rebuild switch --flake .#gnss-node
```

### Standalone package build via flake

```bash
nix build .#gnss2tec-logger
```

or:

```bash
nix build .#default
```

### NixOS module defaults

- service user/group: `root`
- serial port: `/dev/ttyACM0`
- serial wait glob: `/dev/ttyACM*`
- serial wait timeout: `0` (wait forever)
- data dir: `/var/lib/gnss2tec-logger/data`
- archive dir: `/var/lib/gnss2tec-logger/archive`
- config file: `/etc/gnss2tec-logger/ubx.dat` (generated from module `configText` by default)
- `ubx2rinex` path: `pkgs.ubx2rinex` when available, otherwise `ubx2rinex` from `PATH`
- `convbin` path: `pkgs.rtklib` when available, otherwise `convbin` from `PATH`

Note: the Rust binary falls back to `ubx2rinex` and `convbin` from `PATH` if configured absolute paths do not exist.

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

Runtime config file (packaged install):

- `/etc/gnss2tec-logger/runtime.env`
- example keys: `GNSS2TEC_SERIAL_PORT`, `GNSS2TEC_SERIAL_WAIT_GLOB`, `GNSS2TEC_SERIAL_WAIT_TIMEOUT_SECS`, `GNSS2TEC_BAUD_RATE`, `GNSS2TEC_STATS_INTERVAL_SECS`, `GNSS2TEC_NMEA_LOG_INTERVAL_SECS`, `GNSS2TEC_NMEA_LOG_FORMAT`, `GNSS2TEC_DATA_DIR`, `GNSS2TEC_ARCHIVE_DIR`, `GNSS2TEC_UBX2RINEX_PATH`, `GNSS2TEC_CONVBIN_PATH`

Throughput log output:

- logger emits periodic `[STAT]` lines with cumulative bytes and current `bps`
- interval is controlled by `GNSS2TEC_STATS_INTERVAL_SECS` (set `0` to disable)

NMEA status output:

- logger scans incoming serial bytes for NMEA sentences and watches `GSA`, `GSV`, `GNS`, `RMC`, `GBS`, `GST`
- logger emits periodic `[NMEA:<TYPE>]` lines for newly observed watched sentences
- interval is controlled by `GNSS2TEC_NMEA_LOG_INTERVAL_SECS` (set `0` to disable)
- format is controlled by `GNSS2TEC_NMEA_LOG_FORMAT`:
  - `raw`: raw NMEA sentence
  - `plain`: parsed plain-English summary
  - `both`: log both raw and plain lines

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
-> optional `check convbin availability`
-> `for each target hour in window`
-> `find hour UBX files`
-> if no UBX files for that hour: `skip hour`
-> if UBX files exist: `call ubx2rinex` for observations
-> if UBX files exist and NAV enabled: `call convbin` for NAV (fallback to ubx2rinex NAV if convbin unavailable)
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
-> `start background conversion worker`
-> optional startup catch-up enqueue
-> `open current hour file`
-> `MAIN LOOP`

`MAIN LOOP` does:

- read serial bytes and write to active `.ubx`
- periodic flush
- on UTC hour rollover: close previous hour file and rotate immediately
- on UTC hour rollover: enqueue just-closed hour to conversion worker
- conversion worker runs conversion pipeline (`ubx2rinex` + optional `convbin`) in parallel and logs errors without blocking logging
- stop on signal

Then:

`final flush -> EXIT`

### 5) Shared utilities

- `src/shared/lock.rs`: file-based exclusive lock guard for single-instance protection
- `src/shared/signal.rs`: installs Ctrl-C handler and exposes shared run flag for graceful shutdown

## Operational notes

- Device default is `/dev/ttyACM0`; override with `--serial-port` if needed.
- Hour boundaries are based on UTC.
- Bundled conversion tools are open source:
  - `ubx2rinex` built from crates.io source.
  - `convbin` built from RTKLIB source.
- For unattended production use, prefer `systemd` service + `.deb` install.
