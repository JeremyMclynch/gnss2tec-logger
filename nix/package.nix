{
  lib,
  rustPlatform,
  pkg-config,
  libudev,
  src ? lib.cleanSource ../.,
}:

rustPlatform.buildRustPackage rec {
  pname = "gnss2tec-logger";
  version = (lib.importTOML "${src}/Cargo.toml").package.version;

  inherit src;
  cargoLock.lockFile = "${src}/Cargo.lock";

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ libudev ];

  meta = with lib; {
    description = "GNSS UBX logger and hourly RINEX conversion pipeline";
    platforms = platforms.linux;
    mainProgram = "gnss2tec-logger";
  };
}
