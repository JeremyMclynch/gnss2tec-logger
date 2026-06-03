{ config, lib, pkgs, ... }:

let
  cfg = config.services.gnss2tec-logger;

  defaultConfigText = builtins.readFile ../packaging/config/ubx.dat;
  defaultConvbinPath =
    if pkgs ? rtklib then
      "${pkgs.rtklib}/bin/convbin"
    else
      "convbin";
  defaultRnx2crxPath =
    if pkgs ? rnxcmp then
      "${pkgs.rnxcmp}/bin/rnx2crx"
    else
      "rnx2crx";

  cmdArgs =
    [
      "run"
      "--serial-port"
      cfg.serialPort
      "--baud-rate"
      (toString cfg.baudRate)
      "--config-file"
      cfg.configFile
      "--data-dir"
      cfg.dataDir
      "--archive-dir"
      cfg.archiveDir
      "--convbin-path"
      cfg.convbinPath
      "--rnx2crx-path"
      cfg.rnx2crxPath
      "--nav-output-format"
      cfg.navOutputFormat
      "--obs-output-format"
      cfg.obsOutputFormat
      "--min-free-disk-mb"
      (toString cfg.minFreeDiskMb)
    ]
    ++ lib.optional cfg.outputIonex "--output-ionex"
    ++ lib.optional cfg.gpsTimeSync "--gps-time-sync"
    ++ cfg.extraArgs;
in
{
  options.services.gnss2tec-logger = {
    enable = lib.mkEnableOption "GNSS UBX logger and hourly RINEX conversion service";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      description = "Package providing the gnss2tec-logger binary.";
    };

    serialPort = lib.mkOption {
      type = lib.types.str;
      default = "/dev/ttyACM0";
      description = "Serial port connected to the GNSS receiver.";
    };

    baudRate = lib.mkOption {
      type = lib.types.int;
      default = 115200;
      description = "Serial baud rate.";
    };

    serialWaitGlob = lib.mkOption {
      type = lib.types.str;
      default = "/dev/ttyACM*";
      description = "Glob pattern of serial devices to wait for before startup.";
    };

    serialWaitTimeoutSecs = lib.mkOption {
      type = lib.types.int;
      default = 0;
      description = "Seconds to wait for serial device; 0 waits forever.";
    };

    dataDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/gnss2tec-logger/data";
      description = "Directory where raw UBX files are written.";
    };

    archiveDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/gnss2tec-logger/archive";
      description = "Directory where converted RINEX products are archived.";
    };

    configFile = lib.mkOption {
      type = lib.types.str;
      default = "/etc/gnss2tec-logger/ubx.dat";
      description = "Path to UBX configuration file passed to the logger.";
    };

    configText = lib.mkOption {
      type = lib.types.lines;
      default = defaultConfigText;
      description = "UBX configuration text written to /etc/gnss2tec-logger/ubx.dat.";
    };

    convbinPath = lib.mkOption {
      type = lib.types.str;
      default = defaultConvbinPath;
      description = ''
        Path to the convbin executable (RTKLIB).
        If nixpkgs exposes pkgs.rtklib, that path is used automatically.
        Otherwise defaults to "convbin" and relies on PATH lookup.
      '';
      example = "/run/current-system/sw/bin/convbin";
    };

    rnx2crxPath = lib.mkOption {
      type = lib.types.str;
      default = defaultRnx2crxPath;
      description = ''
        Path to the rnx2crx executable (RNXCMP).
        If nixpkgs exposes pkgs.rnxcmp, that path is used automatically.
        Otherwise defaults to "rnx2crx" and relies on PATH lookup.
      '';
      example = "/run/current-system/sw/bin/rnx2crx";
    };

    navOutputFormat = lib.mkOption {
      type = lib.types.enum [
        "mixed"
        "individual-tar-gz"
      ];
      default = "individual-tar-gz";
      description = "Navigation output format.";
    };

    obsOutputFormat = lib.mkOption {
      type = lib.types.enum [
        "rinex"
        "hatanaka"
      ];
      default = "rinex";
      description = "Observation output format.";
    };

    outputIonex = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Generate optional IONEX products from observation RINEX files.";
    };

    minFreeDiskMb = lib.mkOption {
      type = lib.types.int;
      default = 500;
      description = ''
        Prune the oldest archived day directories when free space on the archive
        filesystem drops below this many MB, so logging never stops on a full
        disk. Set to 0 to disable pruning.
      '';
    };

    hardwareWatchdog = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Drive the board's hardware watchdog via systemd so the device reboots
        itself if the kernel or systemd hangs. Requires a /dev/watchdog device.
        Disabled by default.
      '';
    };

    hardwareWatchdogSec = lib.mkOption {
      type = lib.types.int;
      default = 30;
      description = "Hardware watchdog timeout in seconds (used when hardwareWatchdog is enabled).";
    };

    gpsTimeSync = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Discipline the system clock from the receiver's GPS time for offline
        operation. The logger feeds parsed UTC into chrony via the NTP shared-
        memory refclock; this option enables chrony and configures refclock SHM 0.
      '';
    };

    udevArdusimple = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Install a udev rule that creates a /dev/tty_Ardusimple symlink
        for ArduSimple GNSS receivers (VID 1546, PID 01a9).
      '';
    };

    usbGadgetConsole = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Enable a USB CDC ACM gadget (virtual serial port) on the OTG port
        and spawn a getty login console on it. Allows serial access over USB.
      '';
    };

    extraArgs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "Extra arguments appended to `gnss2tec-logger run`.";
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "root";
      description = "User account for the systemd service.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "root";
      description = "Group account for the systemd service.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.etc = lib.mkIf (cfg.configFile == "/etc/gnss2tec-logger/ubx.dat") {
      "gnss2tec-logger/ubx.dat".text = cfg.configText;
    };

    services.udev.extraRules = lib.mkIf cfg.udevArdusimple ''
      KERNEL=="ttyACM[0-9]*", ATTRS{idVendor}=="1546", ATTRS{idProduct}=="01a9", SYMLINK+="tty_Ardusimple", GROUP="dialout", MODE="0666"
    '';

    systemd.tmpfiles.rules = [
      "d ${builtins.dirOf cfg.dataDir} 0750 root root -"
      "d ${cfg.dataDir} 0750 root root -"
      "d ${builtins.dirOf cfg.archiveDir} 0750 root root -"
      "d ${cfg.archiveDir} 0750 root root -"
    ];

    boot.kernelModules = lib.mkIf cfg.usbGadgetConsole [ "libcomposite" ];

    systemd.services.gnss2tec-usb-gadget = lib.mkIf cfg.usbGadgetConsole {
      description = "USB CDC ACM gadget (virtual serial port)";
      wantedBy = [ "multi-user.target" ];
      after = [ "local-fs.target" "sys-kernel-config.mount" ];
      # Only run when a USB Device Controller is present (gadget-capable hardware).
      unitConfig.ConditionPathExistsGlob = "/sys/class/udc/*";
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        # libcomposite must be loaded before /sys/kernel/config/usb_gadget exists.
        ExecStartPre = "-${pkgs.kmod}/bin/modprobe libcomposite";
        ExecStart = pkgs.writeShellScript "usb-gadget-start" ''
          GADGET=/sys/kernel/config/usb_gadget/gnss2tec
          mkdir -p "$GADGET"
          echo 0x1d6b > "$GADGET/idVendor"
          echo 0x0104 > "$GADGET/idProduct"
          echo 0x0100 > "$GADGET/bcdDevice"
          echo 0x0200 > "$GADGET/bcdUSB"
          mkdir -p "$GADGET/strings/0x409"
          echo "gnss2tec-logger" > "$GADGET/strings/0x409/manufacturer"
          echo "GNSS Serial Console" > "$GADGET/strings/0x409/product"
          echo "0" > "$GADGET/strings/0x409/serialnumber"
          mkdir -p "$GADGET/configs/c.1/strings/0x409"
          echo "CDC ACM" > "$GADGET/configs/c.1/strings/0x409/configuration"
          echo 250 > "$GADGET/configs/c.1/MaxPower"
          mkdir -p "$GADGET/functions/acm.usb0"
          ln -sf "$GADGET/functions/acm.usb0" "$GADGET/configs/c.1/acm.usb0"
          UDC="$(ls /sys/class/udc | head -n1)"
          echo "$UDC" > "$GADGET/UDC"
        '';
        ExecStop = pkgs.writeShellScript "usb-gadget-stop" ''
          GADGET=/sys/kernel/config/usb_gadget/gnss2tec
          if [ -d "$GADGET" ]; then
            echo "" > "$GADGET/UDC" 2>/dev/null || true
            rm -f "$GADGET/configs/c.1/acm.usb0" 2>/dev/null || true
            rmdir "$GADGET/configs/c.1/strings/0x409" 2>/dev/null || true
            rmdir "$GADGET/configs/c.1" 2>/dev/null || true
            rmdir "$GADGET/functions/acm.usb0" 2>/dev/null || true
            rmdir "$GADGET/strings/0x409" 2>/dev/null || true
            rmdir "$GADGET" 2>/dev/null || true
          fi
        '';
      };
    };

    systemd.services.gnss2tec-usb-getty = lib.mkIf cfg.usbGadgetConsole {
      description = "Getty on USB gadget serial console";
      wantedBy = [ "multi-user.target" ];
      after = [ "gnss2tec-usb-gadget.service" ];
      requires = [ "gnss2tec-usb-gadget.service" ];
      unitConfig.ConditionPathExists = "/dev/ttyGS0";
      serviceConfig = {
        Type = "simple";
        ExecStart = "${pkgs.util-linux}/bin/agetty -o '-p -f -- \\\\u' 115200 ttyGS0 linux";
        Restart = "always";
        RestartSec = 2;
      };
    };

    systemd.services.gnss2tec-logger = {
      description = "GNSS UBX logger and RINEX conversion pipeline";
      wantedBy = [ "multi-user.target" ];
      after = [ "local-fs.target" ];
      wants = [ "local-fs.target" ];
      path = builtins.filter (x: x != null) [
        (if pkgs ? rtklib then pkgs.rtklib else null)
        (if pkgs ? rnxcmp then pkgs.rnxcmp else null)
      ];
      preStart = ''
        wait_glob="${cfg.serialWaitGlob}"
        timeout="${toString cfg.serialWaitTimeoutSecs}"
        start=$(date +%s)
        while true; do
          for dev in $wait_glob; do
            if [ -e "$dev" ]; then
              exit 0
            fi
          done
          if [ "$timeout" -gt 0 ] && [ $(( $(date +%s) - start )) -ge "$timeout" ]; then
            echo "Timed out waiting for serial device(s): $wait_glob" >&2
            exit 1
          fi
          sleep 1
        done
      '';
      serviceConfig = {
        # notify + WatchdogSec form the software watchdog: the logger pets the
        # watchdog from its main loop, so a hang (without exit) is restarted.
        Type = "notify";
        WatchdogSec = "60s";
        User = cfg.user;
        Group = cfg.group;
        WorkingDirectory = builtins.dirOf cfg.dataDir;
        ExecStart = "${cfg.package}/bin/gnss2tec-logger ${lib.escapeShellArgs cmdArgs}";
        Restart = "always";
        RestartSec = 5;
        TimeoutStartSec = 0;
        UMask = "0027";
      };
    };

    # Optional hardware watchdog: systemd opens /dev/watchdog and reboots the
    # board if the kernel or systemd itself hangs.
    systemd.watchdog.runtimeTime = lib.mkIf cfg.hardwareWatchdog "${toString cfg.hardwareWatchdogSec}s";

    # Optional GPS time sync: chrony reads the logger's GPS time via NTP SHM and
    # disciplines the system clock without any network reference.
    services.chrony = lib.mkIf cfg.gpsTimeSync {
      enable = true;
      extraConfig = ''
        refclock SHM 0 refid GPS precision 1e-2 offset 0.0
        makestep 1 -1
      '';
    };
  };
}
