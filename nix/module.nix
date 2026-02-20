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
    ]
    ++ lib.optional cfg.outputIonex "--output-ionex"
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

    systemd.tmpfiles.rules = [
      "d ${builtins.dirOf cfg.dataDir} 0750 root root -"
      "d ${cfg.dataDir} 0750 root root -"
      "d ${builtins.dirOf cfg.archiveDir} 0750 root root -"
      "d ${cfg.archiveDir} 0750 root root -"
    ];

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
        Type = "simple";
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
  };
}
