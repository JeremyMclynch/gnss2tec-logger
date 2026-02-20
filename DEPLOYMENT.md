# Deployment Notes

## Build a `.deb` package

```bash
./scripts/build-deb.sh
```

For ARM64:

```bash
./scripts/build-deb.sh --target aarch64-unknown-linux-gnu --deb-arch arm64
```

If you are cross-building from x86_64, install the linker first:

```bash
sudo apt install gcc-aarch64-linux-gnu
```

Or build natively on an ARM64 host.

The builder compiles and bundles:

- `/usr/bin/gnss2tec-logger`
- `/usr/lib/gnss2tec-logger/bin/ubx2rinex` (from crates.io source)
- `/etc/gnss2tec-logger/ubx.dat`
- `/lib/systemd/system/gnss2tec-logger.service`

## Install

```bash
sudo dpkg -i dist/gnss2tec-logger_<version>_<arch>.deb
```

The package `postinst` script:

- creates `/var/lib/gnss2tec-logger/data`
- creates `/var/lib/gnss2tec-logger/archive`
- enables and starts `gnss2tec-logger.service`

## Service behavior

The service runs as `root` and starts automatically on boot:

```bash
systemctl status gnss2tec-logger.service
journalctl -u gnss2tec-logger.service -f
```

## Data retention on uninstall

Package removal does **not** delete `/var/lib/gnss2tec-logger`.

- `dpkg -r gnss2tec-logger`: service removed, data preserved
- `dpkg --purge gnss2tec-logger`: config purged by dpkg rules, data still preserved
