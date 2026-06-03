# gnss2tec-logger

Rust-based GNSS data logger and converter pipeline for u-blox receivers.

***The work at the University of Scranton and New Jersey Institute of Technology was supported by the National Science Foundation under grants AGS-2432821 and AGS-2432823, respectively. The design of this system was inspired and based on a sample from the EclipseNB project provided to the New Jersey Institute of Technology by the University of New Brunswick by Anton Kashcheyev which was redesigned for ease of installation and configuration. Opensource dependencies include RTKlib among others which will be credited properly as development continues before the 1.0 release.***

### NOTE: This program is still in active development, I have not released a 1.0 Version so I cannot guarantee stability or performance yet. As such only install at your own risk. I have not decided on a specific open source license yet either as I am referencing some other projects so I am still in the progress of figuring out the requirements I must meet under the respective dependencies. Please contact me at jmm277@njit.edu or contact@jmclynch.org with any questions. 

This program:

- sends UBX configuration commands (from `ubx.dat`) to a receiver on a serial port
- logs raw UBX binary data into hourly files
- converts closed hours into compressed RINEX products (OBS `.rnx.gz` by default, optional NAV products, optional IONEX)
- archives products by `year/day-of-year`
- can run continuously as a `systemd` service at boot

## Why this exists

This is a project which will allow citizen scientists, or student researchers to build their own GNSS based TEC monitoring system at a *relativly* low cost. I am currently in the process of creating documentation and a build guide to describe how to build one of these systems yourself. Currently this system is designed for Ublox based recievers **specifically ZED-FP9 based recievers**, however, I am in the process of supporting more reciever formats. I am planning on adding support for Septentrio Mossaic based recievers next, please contact me to let me know if you would like support for other reciever types.

Currently I am only logging raw GNSS observables, I am working on adding native support for conversion into the IGS IONEX standard but I have added temporary support for the conversion into IONEX format via. the nav-solutions IONEX repo (I have not confirmed the IONEX output data is accurate yet), but I plan to replace this in the future. We are also working to verify the quality of the output data in comparison to our reference Septentrio PolaRx5S reciever, although the design we based our testing system off of was verified to be accurate by Kashcheyev et al. (2025) [https://doi.org/10.1029/2024SW004194].

- continuous UBX logging
- hourly RINEX conversion
- compression + archive organization

## Current architecture

- CLI + orchestration: Rust (`clap`, `anyhow`, `chrono`)
- serial access: `serialport`
- UBX packet building from config commands: `ublox` crate
- lock files to prevent duplicate instances: `fs2` file locks
- UBX -> RINEX conversion:
  - `convbin` (RTKLIB) for observation and multi-constellation navigation products
  - optional Hatanaka observation compression via `rnx2crx` (RNXCMP)
- optional IONEX generation: `rinex` (nav-solutions repo) + `ionex` crates

IONEX note: current output is a compatibility-oriented file derived from OBS metadata (not a calibrated global TEC solution).

## Repository layout

- `src/main.rs`: CLI parse + command dispatch
- `src/args.rs`: all command-line argument definitions/defaults
- `src/commands/log.rs`: receiver config + UBX logging
- `src/commands/convert.rs`: hourly UBX -> RINEX conversion + archive + cleanup
- `src/commands/run.rs`: continuous mode (logging + automatic hourly conversion)
- `src/shared/lock.rs`: process lock guard
- `src/shared/signal.rs`: Ctrl-C shutdown signal handling
- `packaging/`: systemd units (logger + optional USB gadget/getty), udev rule, default config, Debian maintainer scripts
- `scripts/build-deb.sh`: `.deb` packager (bundles `convbin` + `rnx2crx`)
- `flake.nix`: flake outputs for package/devShell/module
- `nix/package.nix`: reusable Nix package definition
- `nix/module.nix`: NixOS module (`services.gnss2tec-logger`)

## Default runtime paths

- config file: `/etc/gnss2tec-logger/ubx.dat`
- data directory: `/var/lib/gnss2tec-logger/data`
- archive directory: `/var/lib/gnss2tec-logger/archive`
- bundled converter paths:
  - `/usr/lib/gnss2tec-logger/bin/convbin`
  - `/usr/lib/gnss2tec-logger/bin/rnx2crx`

## Building from source

Most users should install a prebuilt `.deb` (see below) or use the Nix flake.
Build from source if you are developing, targeting an unsupported architecture,
or want to produce your own package.

### Prerequisites

- A stable Rust toolchain (install via [rustup](https://rustup.rs)).
- System packages (Debian/Ubuntu names):

```bash
sudo apt-get install -y build-essential pkg-config libudev-dev curl git
```

`build-essential`, `curl`, and `git` are needed by `scripts/build-deb.sh` to
fetch and compile the bundled `convbin` (RTKLIB) and `rnx2crx` (RNXCMP) tools.
A plain `cargo build` only needs `pkg-config` and `libudev-dev`.

One dependency (`ionex`) pulls in a git submodule declared with an SSH URL. For
non-interactive builds without an SSH key, rewrite those URLs to HTTPS first:

```bash
export CARGO_NET_GIT_FETCH_WITH_CLI=true
git config --global url."https://github.com/".insteadOf "git@github.com:"
```

(`scripts/build-deb.sh` applies this rewrite automatically, so it is only needed
for a direct `cargo build`.)

### Build the binary

```bash
cargo build --release          # produces target/release/gnss2tec-logger
cargo check                    # type-check only
cargo clippy                   # lint
```

There is no test suite.

### Build a Debian package

`scripts/build-deb.sh` builds the logger, fetches and compiles `convbin` and
`rnx2crx`, and assembles a `.deb` under `dist/`.

Native build (architecture inferred from the host):

```bash
bash scripts/build-deb.sh
```

Cross-compile for arm64 from an x86_64 host (requires the cross toolchain and
the target's `libudev`):

```bash
sudo apt-get install -y gcc-aarch64-linux-gnu libc6-dev-arm64-cross
sudo dpkg --add-architecture arm64 && sudo apt-get update
sudo apt-get install -y libudev-dev:arm64
rustup target add aarch64-unknown-linux-gnu

bash scripts/build-deb.sh --target aarch64-unknown-linux-gnu --deb-arch arm64
```

For a native build on an arm64 host, just run `bash scripts/build-deb.sh
--deb-arch arm64` (no cross toolchain needed). Run `bash scripts/build-deb.sh
--help` for all options.

### Build via Nix

```bash
nix build .#gnss2tec-logger    # or: nix build .#default
```

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
- `/usr/lib/gnss2tec-logger/bin/convbin` (bundled RTKLIB, open-source)
- `/usr/lib/gnss2tec-logger/bin/rnx2crx` (bundled RNXCMP, open-source)
- `/etc/gnss2tec-logger/ubx.dat`
- `/etc/gnss2tec-logger/runtime.env`
- `/lib/systemd/system/gnss2tec-logger.service`
- `/lib/systemd/system/gnss2tec-usb-gadget.service` (enabled only when `GNSS2TEC_USB_GADGET_CONSOLE=true`)
- `/lib/systemd/system/gnss2tec-usb-getty.service` (enabled only when `GNSS2TEC_USB_GADGET_CONSOLE=true`)
- `/usr/share/gnss2tec-logger/udev/99-gnss2tec-logger.rules` (installed to `/etc/udev/rules.d/` only when `GNSS2TEC_UDEV_ARDUSIMPLE=true`)
- `/lib/systemd/system/gnss2tec-hdd-backup.service` + `.timer` (enabled only when `GNSS2TEC_HDD_BACKUP=true`)
- `/usr/lib/gnss2tec-logger/bin/gnss2tec-backup.sh` (external-drive backup helper)
- `/usr/share/gnss2tec-logger/autofs/` (auto-mount templates, installed to `/etc/` only when `GNSS2TEC_HDD_BACKUP=true`)
- `/usr/share/doc/gnss2tec-logger/RTKLIB_README.txt`
- `/usr/share/doc/gnss2tec-logger/RNXCMP_README.txt`

## Configuration (`runtime.env`)

Runtime behavior is configured through `/etc/gnss2tec-logger/runtime.env`. Every
setting is shipped commented out, so the program's built-in defaults apply until
you uncomment a line and set a value. After editing, apply changes with:

```bash
sudoedit /etc/gnss2tec-logger/runtime.env
sudo systemctl restart gnss2tec-logger.service
```

The service loads this file via `EnvironmentFile`, and each `GNSS2TEC_*`
variable maps to the matching `gnss2tec-logger run` option. This file is marked
as a Debian conffile, so your edits are preserved across package upgrades.

### Optional features

These toggles change how the system is provisioned, not just how the logger
runs. They are applied by the package's post-install script, so after changing
one you must re-run the installer or `sudo dpkg-reconfigure gnss2tec-logger`
(a plain service restart is not enough).

| Variable | Default | Description |
| --- | --- | --- |
| `GNSS2TEC_UDEV_ARDUSIMPLE` | `false` | When `true`, installs a udev rule that creates a stable `/dev/tty_Ardusimple` symlink for ArduSimple receivers (USB VID `1546`, PID `01a9`). This gives the receiver a predictable device name regardless of which `/dev/ttyACM*` number the kernel assigns. When `false`, the rule is removed. |
| `GNSS2TEC_USB_GADGET_CONSOLE` | `false` | When `true`, configures the board's USB OTG port as a CDC ACM virtual serial port and starts a login console (getty) on it. A host plugged into the USB port sees a serial device and can open a console at 115200 baud — no network or SSH required. Loads the `libcomposite` kernel module at boot and enables the `gnss2tec-usb-gadget` and `gnss2tec-usb-getty` services. When `false`, those services and the module-load entry are removed. Requires gadget-capable hardware (a USB Device Controller under `/sys/class/udc/`). |
| `GNSS2TEC_HARDWARE_WATCHDOG` | `false` | When `true`, installs a systemd manager drop-in (`RuntimeWatchdogSec`) so systemd drives the board's hardware watchdog and reboots the device if the kernel or systemd hangs. Requires a `/dev/watchdog` device. When `false`, the drop-in is removed. See [Reliability and unattended recovery](#reliability-and-unattended-recovery). |
| `GNSS2TEC_HARDWARE_WATCHDOG_SEC` | `30` | Hardware watchdog timeout in seconds (used only when `GNSS2TEC_HARDWARE_WATCHDOG=true`). |
| `GNSS2TEC_GPS_TIME_SYNC` | `false` | When `true`, the logger feeds the receiver's GPS UTC time to `chrony` via the NTP shared-memory refclock so the system clock stays correct offline. Requires the `chrony` package (Recommended, not a hard dependency); the package writes `/etc/chrony/conf.d/gnss2tec-gps.conf` and restarts chrony. See [Reliability and unattended recovery](#reliability-and-unattended-recovery). |
| `GNSS2TEC_HDD_BACKUP` | `false` | When `true`, auto-mounts a labelled external USB drive (via autofs at `/mnt/<label>`) and copies each completed archive day to it once per day. Requires `autofs` + `rsync` (Recommended). See [External-drive backup](#external-drive-backup). |
| `GNSS2TEC_BACKUP_LABEL` | `rinexbackup` | Partition label of the backup drive — the autofs key and rsync target `/mnt/<label>`. |
| `GNSS2TEC_BACKUP_DELETE_MODE` | `rotate` | `rotate` keeps an internal cache and culls oldest *collected* days when free space is low; `immediate` deletes the internal copy right after it is collected. |
| `GNSS2TEC_BACKUP_MIN_FREE_MB` | `2048` | rotate-mode floor: cull collected days when internal free space drops below this (MB). Keep above `GNSS2TEC_MIN_FREE_DISK_MB`. |

### Serial receiver settings

| Variable | Default | Description |
| --- | --- | --- |
| `GNSS2TEC_SERIAL_PORT` | `/dev/ttyACM0` | Serial device the receiver is connected to. Set this to `/dev/tty_Ardusimple` when using the ArduSimple udev rule above. |
| `GNSS2TEC_BAUD_RATE` | `115200` | Serial baud rate. |
| `GNSS2TEC_READ_TIMEOUT_MS` | `250` | Per-read serial timeout in milliseconds. |
| `GNSS2TEC_READ_BUFFER_BYTES` | `8192` | Size of the serial read buffer. |
| `GNSS2TEC_COMMAND_GAP_MS` | `50` | Delay between UBX configuration packets sent at startup. |
| `GNSS2TEC_SERIAL_WAIT_GLOB` | `/dev/ttyACM*` | Glob of device(s) the service waits for before launching the logger. |
| `GNSS2TEC_SERIAL_WAIT_TIMEOUT_SECS` | `0` | Seconds to wait for the serial device; `0` waits forever. |

### Logging and conversion behavior

| Variable | Default | Description |
| --- | --- | --- |
| `GNSS2TEC_FLUSH_INTERVAL_SECS` | `5` | How often buffered UBX data is flushed to disk. |
| `GNSS2TEC_STATS_INTERVAL_SECS` | `5` | Interval for `[STAT]` throughput lines; `0` disables them. |
| `GNSS2TEC_NMEA_LOG_INTERVAL_SECS` | `30` | Interval for `[NMEA:*]` status lines (GSA/GSV/GNS/RMC/GBS/GST); `0` disables them. |
| `GNSS2TEC_NMEA_LOG_FORMAT` | `plain` | NMEA log format: `raw`, `plain` (parsed summary), or `both`. |
| `GNSS2TEC_SHIFT_HOURS` | `1` | Hour offset applied when selecting which closed hour to convert. |
| `GNSS2TEC_MAX_DAYS_BACK` | `3` | How many days back the startup catch-up scan looks for unconverted hours. |
| `GNSS2TEC_NAV_OUTPUT_FORMAT` | `individual-tar-gz` | Navigation output: `mixed` (one file) or `individual-tar-gz` (per-constellation, bundled). |
| `GNSS2TEC_OBS_OUTPUT_FORMAT` | `rinex` | Observation output: `rinex` or `hatanaka` (CRINEX). |
| `GNSS2TEC_OUTPUT_IONEX` | `false` | When `true`, also generate an IONEX product from OBS metadata. |
| `GNSS2TEC_OBS_SAMPLING_SECS` | `1` | Observation sampling interval in seconds. |
| `GNSS2TEC_SKIP_NAV` | `false` | When `true`, skip navigation product generation. |
| `GNSS2TEC_KEEP_UBX` | `false` | When `true`, keep source `.ubx` files after conversion instead of deleting them. |
| `GNSS2TEC_MIN_FREE_DISK_MB` | `500` | When free space on the archive filesystem drops below this many MB, prune the oldest archived day directories so logging never stops on a full disk. `0` disables pruning. |

### Paths

| Variable | Default | Description |
| --- | --- | --- |
| `GNSS2TEC_CONFIG_FILE` | `/etc/gnss2tec-logger/ubx.dat` | UBX command file sent to the receiver at startup. |
| `GNSS2TEC_DATA_DIR` | `/var/lib/gnss2tec-logger/data` | Where raw hourly `.ubx` files are written. |
| `GNSS2TEC_ARCHIVE_DIR` | `/var/lib/gnss2tec-logger/archive` | Where converted RINEX products are archived by `year/day-of-year`. |
| `GNSS2TEC_CONVBIN_PATH` | `/usr/lib/gnss2tec-logger/bin/convbin` | Path to the bundled `convbin` binary. |
| `GNSS2TEC_RNX2CRX_PATH` | `/usr/lib/gnss2tec-logger/bin/rnx2crx` | Path to the bundled `rnx2crx` binary. |

### Station metadata

These values are written into the RINEX headers and the output filenames. The
defaults are neutral placeholders — set them to describe your own station.

| Variable | Default | Description |
| --- | --- | --- |
| `GNSS2TEC_STATION` | `STAT` | 4-character station/marker name used in the RINEX long filename. |
| `GNSS2TEC_COUNTRY` | `XXX` | 3-letter ISO country code used in the RINEX long filename. |
| `GNSS2TEC_RECEIVER_TYPE` | `u-blox ZED-F9P` | Receiver model recorded in the RINEX header. |
| `GNSS2TEC_ANTENNA_TYPE` | `Unknown` | Antenna model recorded in the RINEX header. |
| `GNSS2TEC_OBSERVER` | `Unknown` | Observer/agency recorded in the RINEX header. |

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
            # convbinPath = "/run/current-system/sw/bin/convbin";
            # rnx2crxPath = "/run/current-system/sw/bin/rnx2crx";
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
            convbinPath = "${pkgs.rtklib}/bin/convbin";
            rnx2crxPath = "${pkgs.rnxcmp}/bin/rnx2crx";
            navOutputFormat = "individual-tar-gz";
            obsOutputFormat = "rinex";
            outputIonex = false;
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
- `convbin` path: `pkgs.rtklib` when available, otherwise `convbin` from `PATH`
- `rnx2crx` path: `pkgs.rnxcmp` when available, otherwise `rnx2crx` from `PATH`
- NAV output format: `individual-tar-gz` (default) or `mixed`
- OBS output format: `rinex` (default) or `hatanaka`
- optional IONEX output: `outputIonex = true`

Note: the Rust binary falls back to `convbin` / `rnx2crx` from `PATH` if configured absolute paths do not exist.

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
- example keys: `GNSS2TEC_SERIAL_PORT`, `GNSS2TEC_SERIAL_WAIT_GLOB`, `GNSS2TEC_SERIAL_WAIT_TIMEOUT_SECS`, `GNSS2TEC_BAUD_RATE`, `GNSS2TEC_STATS_INTERVAL_SECS`, `GNSS2TEC_NMEA_LOG_INTERVAL_SECS`, `GNSS2TEC_NMEA_LOG_FORMAT`, `GNSS2TEC_DATA_DIR`, `GNSS2TEC_ARCHIVE_DIR`, `GNSS2TEC_CONVBIN_PATH`, `GNSS2TEC_RNX2CRX_PATH`, `GNSS2TEC_NAV_OUTPUT_FORMAT`, `GNSS2TEC_OBS_OUTPUT_FORMAT`, `GNSS2TEC_OUTPUT_IONEX`, `GNSS2TEC_OBS_SAMPLING_SECS`, `GNSS2TEC_MIN_FREE_DISK_MB`, `GNSS2TEC_UDEV_ARDUSIMPLE`, `GNSS2TEC_USB_GADGET_CONSOLE`, `GNSS2TEC_HARDWARE_WATCHDOG`, `GNSS2TEC_HARDWARE_WATCHDOG_SEC`, `GNSS2TEC_GPS_TIME_SYNC`, `GNSS2TEC_HDD_BACKUP`, `GNSS2TEC_BACKUP_LABEL`, `GNSS2TEC_BACKUP_DELETE_MODE`, `GNSS2TEC_BACKUP_MIN_FREE_MB`
- see the [Configuration (`runtime.env`)](#configuration-runtimeenv) section for the full list of variables and their defaults

Throughput log output:

- logger emits periodic `[STAT]` lines with cumulative bytes and current `bps`
- interval is controlled by `GNSS2TEC_STATS_INTERVAL_SECS` (set `0` to disable)

NMEA status output:

- logger scans incoming serial bytes for NMEA sentences and watches `GSA`, `GSV`, `GNS`, `RMC`, `GBS`, `GST`
- logger emits periodic `[NMEA:<TYPE>]` lines for newly observed watched sentences
- interval is controlled by `GNSS2TEC_NMEA_LOG_INTERVAL_SECS` (set `0` to disable)
- format is controlled by `GNSS2TEC_NMEA_LOG_FORMAT` (default: `plain`):
  - `raw`: raw NMEA sentence
  - `plain`: parsed plain-English summary
  - `both`: log both raw and plain lines

## Reliability and unattended recovery

This program is intended to run unmanned at a remote site, often without network
access, so it is designed to recover from failures on its own. The recovery
model is **crash-and-restart**: on most failures the process exits and `systemd`
relaunches it, rather than trying to repair its own state in place.

### Protections currently in place

- **Automatic restart on exit.** The `systemd` unit sets `Restart=always` with
  `RestartSec=5`, so the service is relaunched a few seconds after any exit —
  whether a clean error, a panic, or an unexpected crash.
- **No restart give-up.** `StartLimitIntervalSec=0` disables systemd's start-rate
  limiter. By default systemd stops retrying a unit that restarts too many times
  in a short window (entering a permanent `failed` state); disabling that means
  the logger keeps retrying indefinitely, which is what an unattended node needs.
- **Serial loss triggers recovery.** A serial read error (for example the USB
  receiver being unplugged) is propagated and the process exits, which hands
  control back to systemd for a restart.
- **Wait-for-device on (re)start.** Before each launch, `ExecStartPre` blocks
  until the serial device appears (matching `GNSS2TEC_SERIAL_WAIT_GLOB`, default
  `/dev/ttyACM*`; `GNSS2TEC_SERIAL_WAIT_TIMEOUT_SECS=0` waits forever). So if the
  receiver disconnects and later reconnects, the restarted process simply waits
  for it and resumes — no manual intervention.
- **A silent receiver does not hang the loop.** The serial port is opened with a
  read timeout (`GNSS2TEC_READ_TIMEOUT_MS`, default 250 ms). If no data arrives,
  the read returns a timeout and the loop continues instead of blocking forever.
- **Software watchdog for hangs.** The unit runs with `Type=notify` and
  `WatchdogSec=60`. The logger sends `READY=1` once it is logging and then pets
  the watchdog from its main loop. If the process *hangs* without exiting (a
  deadlock, or a blocked serial read or disk write), the heartbeat stops and
  systemd kills and restarts it — `Restart=always` alone cannot catch a hang
  because it only fires on process exit.
- **Conversion never stops logging, even on a panic.** Hourly RINEX conversion
  runs in a separate worker thread, and each conversion is run inside
  `catch_unwind`. Conversion errors are logged and skipped, and a panic no longer
  kills the worker — it stays alive and keeps converting later hours. The main
  logging loop is never blocked by conversion regardless.
- **Disk-full safeguard.** After each conversion the worker checks free space on
  the archive filesystem; if it drops below `GNSS2TEC_MIN_FREE_DISK_MB`
  (default 500 MB), the oldest `archive/<year>/<doy>/` directories are pruned one
  at a time until enough space is reclaimed. The newest products are always kept,
  so the device keeps logging instead of wedging on a full disk. Set the value to
  `0` to disable pruning.
- **Missed hours are caught up on restart.** On startup the logger scans recent
  hours (back `GNSS2TEC_MAX_DAYS_BACK` days, default 3) and converts any closed
  hours that were not yet processed, so a restart does not lose conversions.
- **Single-instance lock.** A file lock prevents a second instance from starting
  and corrupting data if a restart overlaps a still-exiting process.
- **Bounded data loss on crash.** UBX data is written to append-mode hourly files
  and flushed every `GNSS2TEC_FLUSH_INTERVAL_SECS` (default 5 s), so at most a few
  seconds of buffered data is at risk if the process is killed.

### Optional: hardware watchdog

The software watchdog above recovers a hung *process*, but cannot help if the
whole kernel or `systemd` (PID 1) freezes. For that, enable the board's hardware
watchdog by setting `GNSS2TEC_HARDWARE_WATCHDOG=true` (and optionally
`GNSS2TEC_HARDWARE_WATCHDOG_SEC`, default 30) in `runtime.env`, then reinstall or
run `sudo dpkg-reconfigure gnss2tec-logger`. This installs a systemd manager
drop-in setting `RuntimeWatchdogSec`, so systemd opens `/dev/watchdog` and the
board reboots itself if systemd stops petting it. It is **disabled by default**
because it requires a working `/dev/watchdog` device (the RK3588 on the Orange Pi
5 Plus has one). On NixOS, set `services.gnss2tec-logger.hardwareWatchdog = true;`.

### Optional: GPS time sync (offline clock discipline)

With no network NTP, UTC hour boundaries and file names rely on the board's RTC,
which can drift or be unset at boot. (Only file naming/rotation is affected — the
observation timestamps themselves come from the receiver inside the UBX stream.)

Enable `GNSS2TEC_GPS_TIME_SYNC=true` in `runtime.env` to discipline the system
clock from the receiver's own GPS time. The logger already sees every NMEA
sentence, so it parses the UTC time from RMC and feeds it to `chrony` through the
standard NTP shared-memory refclock (the same SHM interface `gpsd` uses);
`chrony` then steers the clock. `gpsd` is **not** used — the logger holds the
receiver's serial port exclusively, so a separate daemon could not read it.

- Requires the `chrony` package. The `.deb` only **Recommends** it (so an offline
  install never fails); install `chrony` and re-run `sudo dpkg-reconfigure
  gnss2tec-logger`. When the toggle is on and `chrony` is present, the package
  drops `/etc/chrony/conf.d/gnss2tec-gps.conf` (`refclock SHM 0` + `makestep`) and
  restarts `chrony`.
- `makestep` lets `chrony` jump a wildly-wrong RTC at startup, then discipline it.
- Accuracy is ~tens of ms (NMEA over USB) — far more than enough for hourly file
  naming. For sub-ms time you would wire the receiver's PPS output to a GPIO and
  add a `chrony` PPS refclock; that is a hardware change, out of scope here.
- On NixOS, set `services.gnss2tec-logger.gpsTimeSync = true;` (enables `chrony`
  and the refclock automatically).

## External-drive backup

An optional feature (`GNSS2TEC_HDD_BACKUP=true`) copies archived products to a
removable USB drive so a visiting scientist can collect data by simply swapping
the drive — no need to interact with the computer.

**Seamless mount/unmount (autofs).** When enabled, the package installs autofs
maps so that touching `/mnt/<label>` mounts the matching drive on demand, and the
mount auto-releases after 10 s of inactivity — so the drive is safe to unplug
without manually unmounting. (Improved over a plain mount: `nodev,nosuid,noexec`.)

**Collect-once backup.** A daily systemd timer (`gnss2tec-hdd-backup.timer`) runs
a script that, for each *completed* archive day (`archive/<YYYY>/<DDD>/`, today's
in-progress day excluded), rsyncs it to `/mnt/<label>/archive/...` and records it
in a persistent internal log (`/var/lib/gnss2tec-logger/backup/transferred.log`).
That log is the key to safe drive-swapping: a day already in the log is **never
re-pushed** to a freshly-inserted empty drive, so swapping drives to collect data
never causes duplicate copies. If the drive isn't present, the run is a clean skip.
rsync runs filesystem-agnostically (no Unix perms/ownership), so the drive can be
ext4, exFAT, or NTFS.

**Reclaiming internal space** — `GNSS2TEC_BACKUP_DELETE_MODE`:
- `rotate` (default): keep a recent cache on the internal disk and cull the oldest
  **already-collected** days when internal free space drops below
  `GNSS2TEC_BACKUP_MIN_FREE_MB`. Data lives in two places until disk pressure.
- `immediate`: delete each day's internal copy right after it is collected (keeps
  the internal disk near-empty — the original behaviour).

**Relationship to the internal retention floor.** The logger's
`GNSS2TEC_MIN_FREE_DISK_MB` is an unconditional last-resort floor that can delete
*un-backed-up* data to keep logging alive. Keep `GNSS2TEC_BACKUP_MIN_FREE_MB`
**above** it so the backup-aware cull (which only removes collected days) runs
first; the logger floor then only bites if the drive has been absent so long the
disk is critically full.

**Requirements & setup.** Needs `autofs` and `rsync` (the `.deb` Recommends them;
install never fails offline). Set the toggle + `GNSS2TEC_BACKUP_LABEL` in
`runtime.env`, then reinstall or run `sudo dpkg-reconfigure gnss2tec-logger`. The
transfer log lives under `/var/lib/gnss2tec-logger/backup` and is preserved across
package removal. On NixOS, set `services.gnss2tec-logger.hddBackup = true;` (plus
`backupLabel`, `backupDeleteMode`, `backupMinFreeMb`).

> If you previously hand-configured autofs, remove any old map that also manages
> `/mnt` (e.g. `/etc/auto.master.d/partlabel.autofs` + `/etc/auto.partlabel`)
> before enabling this — two maps on `/mnt` conflict. The package warns if it
> detects the old `partlabel.autofs`.

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
-> `check convbin availability`
-> if `obs-output-format=hatanaka`: `check rnx2crx availability`
-> `for each target hour in window`
-> `find hour UBX files`
-> if no UBX files for that hour: `skip hour`
-> if UBX files exist: `merge hour UBX files`
-> if UBX files exist: `call convbin` for observations
-> if `obs-output-format=hatanaka`: `call rnx2crx` then gzip
-> if UBX files exist and NAV enabled:
  - `mixed`: one mixed NAV file
  - `individual-tar-gz` (default): per-constellation NAV files packed into one `.tar.gz`
-> if UBX files exist: `validate outputs (obs + optional nav according to selected formats)`
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
- conversion worker runs conversion pipeline (`convbin`, optional `rnx2crx`) in parallel and logs errors without blocking logging
- stop on signal

Then:

`final flush -> EXIT`

### 5) Shared utilities

- `src/shared/lock.rs`: file-based exclusive lock guard for single-instance protection
- `src/shared/signal.rs`: installs Ctrl-C handler and exposes shared run flag for graceful shutdown

## Operational notes

- Device default is `/dev/ttyACM0`; override with `--serial-port` if needed.
- Hour boundaries are based on UTC.
- OBS output format defaults to standard RINEX (`rinex`); set `hatanaka` to emit CRINEX.
- NAV output format defaults to `individual-tar-gz`; set `mixed` for one mixed NAV file.
- Bundled conversion tools are open source:
  - `convbin` built from RTKLIB source.
  - `rnx2crx` built from RNXCMP source.
- For unattended production use, prefer `systemd` service + `.deb` install.
