{ config, lib, pkgs, ... }:

let
  cfg = config.services.gnss2tec-logger;

  defaultConfigText = builtins.readFile ../packaging/config/ubx.dat;
  defaultUbx2rinexPath =
    if pkgs ? ubx2rinex then
      "${pkgs.ubx2rinex}/bin/ubx2rinex"
    else
      "ubx2rinex";
  defaultConvbinPath =
    if pkgs ? rtklib then
      "${pkgs.rtklib}/bin/convbin"
    else
      "convbin";

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
      "--ubx2rinex-path"
      cfg.ubx2rinexPath
      "--convbin-path"
      cfg.convbinPath
      "--nav-output-format"
      cfg.navOutputFormat
      "--obs-output-format"
      cfg.obsOutputFormat
    ]
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

    ubx2rinexPath = lib.mkOption {
      type = lib.types.str;
      default = defaultUbx2rinexPath;
      description = ''
        Path to the ubx2rinex executable.
        If nixpkgs exposes pkgs.ubx2rinex, that path is used automatically.
        Otherwise defaults to "ubx2rinex" and relies on PATH lookup.
      '';
      example = "/run/current-system/sw/bin/ubx2rinex";
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
        (if pkgs ? ubx2rinex then pkgs.ubx2rinex else null)
        (if pkgs ? rtklib then pkgs.rtklib else null)
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
