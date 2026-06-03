# Orange Pi 5 Plus — Armbian + NVMe Boot Setup

This guide prepares an Orange Pi 5 Plus (Rockchip RK3588) to run Armbian from an
NVMe SSD. There are two parts:

1. **Flash Armbian** onto the NVMe SSD (from a Linux PC, via a USB↔NVMe adapter).
2. **Write a bootloader to the board's SPI flash** so the RK3588 can boot from the
   NVMe. (Out of the box the board does not boot from NVMe; the SPI bootloader adds
   that capability.)

> ⚠️ **`dd` and `rkdeveloptool` write directly to block devices and flash.**
> Double-check every device path (`/dev/sdX`) before running a command — writing to
> the wrong disk will destroy data on it.

---

## 1. Downloads

| File | Where | Notes |
| --- | --- | --- |
| Armbian server image, e.g. `Armbian_25.5.1_Orangepi5-plus_noble_current_6.12.28.img.xz` | <https://www.armbian.com/orange-pi-5-plus/> | Pick the latest **server** image (Noble/Bookworm). |
| `rkspi_loader.img` | <https://drive.google.com/drive/folders/19SMZHj1Y8l_Vvr6_SMDHYdJHi41hMgsI> | The SPI bootloader image written to the board's SPI flash. |
| `rk3588_spl_loader_vXXX.bin` | <https://docs.radxa.com/en/som/cm/cm3/low-level-dev/rkdevtool> | DDR init / U-Boot SPL used during flashing. Note the exact version in the filename. |
| *(optional)* RKDevTool GUI | same Radxa page | Windows-only GUI alternative to `rkdeveloptool`. |

---

## 2. Install `rkdeveloptool` (on a Debian/Ubuntu PC)

This is the CLI used to talk to the board in maskrom mode.

```bash
sudo apt-get update
sudo apt-get install -y \
    build-essential git wget pkg-config dh-autoreconf \
    libudev-dev libusb-1.0-0 libusb-1.0-0-dev

git clone https://github.com/rockchip-linux/rkdeveloptool
cd rkdeveloptool
autoreconf -i
./configure
make -j "$(nproc)"
sudo cp rkdeveloptool /usr/local/sbin/
```

Verify it's on your `PATH`:

```bash
sudo rkdeveloptool -v
```

---

## 3. Flash Armbian to the NVMe SSD

1. Put the NVMe SSD into a USB↔NVMe adapter and plug it into the PC.

2. Identify the device node — **note the size to be sure it's the SSD, not your
   system disk**:

   ```bash
   lsblk -o NAME,SIZE,MODEL,TRAN
   ```

   Assume it shows up as `/dev/sdX` below (e.g. `/dev/sdb`).

3. Decompress the Armbian image (`-k` keeps the `.xz`, `-T0` uses all cores):

   ```bash
   xz -T0 -dk Armbian_*.img.xz
   ```

4. Write the image to the SSD:

   ```bash
   sudo dd if=Armbian_*.img of=/dev/sdX bs=4M status=progress oflag=sync
   sync
   ```

---

## 4. Expand the root partition (CLI — replaces GParted)

`dd` leaves the root partition at the image's original size, so the rest of the
SSD is unused. Grow the partition and its filesystem to fill the disk.

Install `growpart` (once):

```bash
sudo apt-get install -y cloud-guest-utils
```

The Armbian root partition is partition **1**. Grow it, check it, then resize the
ext4 filesystem (re-run `lsblk` first to confirm the exact node):

```bash
sudo growpart /dev/sdX 1          # grow partition 1 to fill the disk
sudo e2fsck -f /dev/sdX1          # required before an offline resize
sudo resize2fs /dev/sdX1          # grow the ext4 filesystem
```

> ℹ️ Armbian also auto-expands the root filesystem on first boot, so this step is
> optional — but doing it now confirms the SSD is healthy and fully usable.

Flush and remove the SSD:

```bash
sync
```

Then unplug the USB adapter and take the NVMe out.

---

## 5. Install the NVMe in the Orange Pi

Insert the NVMe SSD into the M.2 slot on the underside of the Orange Pi 5 Plus and
secure it.

---

## 6. Write the SPI bootloader (maskrom mode)

This makes the board boot from the NVMe.

1. **Enter maskrom mode:** connect the board's data USB‑C port to your PC, then
   **press and hold the MASKROM button** while applying power. Release once
   powered.

2. **Confirm the board is in maskrom mode** (should list a device as `Maskrom`):

   ```bash
   sudo rkdeveloptool ld
   ```

3. **Load the SPL** (initializes DRAM so the next steps can access SPI flash) —
   use the exact filename you downloaded:

   ```bash
   sudo rkdeveloptool db rk3588_spl_loader_vXXX.bin
   ```

4. **Erase the SPI flash:**

   ```bash
   sudo rkdeveloptool ef
   ```

5. **Write the SPI bootloader** to the start of flash:

   ```bash
   sudo rkdeveloptool wl 0 rkspi_loader.img
   ```

6. **Reboot the board:**

   ```bash
   sudo rkdeveloptool rd
   ```

Disconnect the data USB‑C cable. The Orange Pi will now boot from the NVMe.

---

## 7. First boot

1. Connect a monitor + keyboard, or Ethernet for SSH. Armbian's first boot
   finishes resizing the filesystem and may reboot once.
2. Log in (default Armbian: user `root`, password `1234`). You'll be prompted to
   set a new root password and create a normal user.
3. Set a hostname / timezone / locale as prompted.

To find the board on the network for SSH:

```bash
# from the PC
ssh <user>@<board-ip>
```

---

## 8. Install gnss2tec-logger

Once Armbian is up, follow the main project README to install the `.deb` package
and configure the logger:

- [README — Installation](README.md#installation)
- [README — Configuration (`runtime.env`)](README.md#configuration-runtimeenv)

---

## Troubleshooting

- **`rkdeveloptool ld` shows nothing / "not found":** use a known-good *data*
  USB‑C cable (some are charge-only), make sure you held MASKROM while powering on,
  and run the command with `sudo`. Try a different USB port on the PC.
- **Permission denied on `wl`/`ef`/`rd`:** run every `rkdeveloptool` command with
  `sudo` (or add a udev rule for the Rockchip USB VID `2207`).
- **Unsure which `/dev/sdX` is the SSD:** re-run `lsblk -o NAME,SIZE,MODEL,TRAN`;
  the USB adapter shows `TRAN=usb` and the SSD's real size.
- **Board still doesn't boot from NVMe:** confirm step 6 completed without errors
  and that the NVMe is seated; re-enter maskrom and re-run `ef` then `wl 0`.
