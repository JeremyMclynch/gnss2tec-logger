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
- `flake.nix`: flake outputs for package/devShell/module
- `nix/package.nix`: reusable Nix package definition
- `nix/module.nix`: NixOS module (`services.gnss2tec-logger`)

## Default runtime paths

- config file: `/etc/gnss2tec-logger/ubx.dat`
- data directory: `/var/lib/gnss2tec-logger/data`
- archive directory: `/var/lib/gnss2tec-logger/archive`
- bundled converter path: `/usr/lib/gnss2tec-logger/bin/ubx2rinex`

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

What the package installs:

- `/usr/bin/gnss2tec-logger`
- `/usr/lib/gnss2tec-logger/bin/ubx2rinex` (bundled, open-source)
- `/etc/gnss2tec-logger/ubx.dat`
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
- data dir: `/var/lib/gnss2tec-logger/data`
- archive dir: `/var/lib/gnss2tec-logger/archive`
- config file: `/etc/gnss2tec-logger/ubx.dat` (generated from module `configText` by default)
- converter path: `pkgs.ubx2rinex` when available, otherwise `ubx2rinex` from `PATH`

Note: the Rust binary now falls back to `ubx2rinex` from `PATH` if the configured absolute converter path does not exist.

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
