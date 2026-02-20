{
  description = "gnss2tec-logger: GNSS UBX logger and hourly RINEX pipeline";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.11";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { self, nixpkgs, flake-utils }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      module = import ./nix/module.nix;
    in
    (flake-utils.lib.eachSystem supportedSystems (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        package = pkgs.callPackage ./nix/package.nix { src = self; };
      in
      {
        packages = {
          default = package;
          gnss2tec-logger = package;
        };

        apps.default = {
          type = "app";
          program = "${package}/bin/gnss2tec-logger";
        };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            pkg-config
            libudev
          ];
        };
      }
    )))
    // {
      nixosModules.default = module;
      nixosModules.gnss2tec-logger = module;
    };
}
