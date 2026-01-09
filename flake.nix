{
  description = "termui - Run graphical apps in terminal via Kitty graphics protocol";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustToolchain
            pkg-config

            # Wayland dependencies
            wayland
            wayland-protocols
            libxkbcommon
            libinput
            udev
            mesa
            libdrm
            libglvnd
            pixman

            # For smithay
            seatd

            # Image processing
            libjpeg
            libpng

            # Debug tools
            gdb
            wayland-utils

            # Test apps
            weston
            foot
          ];

          shellHook = ''
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [
              pkgs.wayland
              pkgs.libxkbcommon
              pkgs.mesa
              pkgs.libglvnd
            ]}:$LD_LIBRARY_PATH"
          '';
        };

        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "termui";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = with pkgs; [
            wayland
            wayland-protocols
            libxkbcommon
            libinput
            udev
            mesa
            libdrm
            pixman
            seatd
          ];
        };
      });
}
