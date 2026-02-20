{ config, lib, pkgs, ... }:

let
  cfg = config.services.gnss2tec-logger;

  defaultConfigText = builtins.readFile ../packaging/config/ubx.dat;
  defaultUbx2rinexPath =
    if pkgs ? ubx2rinex then
      "${pkgs.ubx2rinex}/bin/ubx2rinex"
    else
      "ubx2rinex";

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
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      path = lib.optional (pkgs ? ubx2rinex) pkgs.ubx2rinex;
      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        WorkingDirectory = builtins.dirOf cfg.dataDir;
        ExecStart = "${cfg.package}/bin/gnss2tec-logger ${lib.escapeShellArgs cmdArgs}";
        Restart = "always";
        RestartSec = 5;
        UMask = "0027";
      };
    };
  };
}
