{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, utils, rust-overlay }:
    utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "clippy" "rustfmt" ];
          targets = [ "x86_64-unknown-linux-musl" ];
        };

        # musl pkgs for static linking
        pkgsMusl = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
          crossSystem = {
            config = "x86_64-unknown-linux-musl";
            isStatic = true;
          };
        };
      in
      {
        packages.default = pkgsMusl.rustPlatform.buildRustPackage {
          pname = "x-phone-rust";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = with pkgsMusl; [ cmake pkg-config ];
          buildInputs = with pkgsMusl; [ libopus openssl.dev ];

          CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
          CARGO_BUILD_RUSTFLAGS = [ "-C" "target-feature=+crt-static" ];
        };

        devShells.default = pkgs.mkShell {
          buildInputs = [
            rustToolchain
            pkgs.cmake
            pkgs.pkg-config
            pkgs.libopus
            pkgs.openssl.dev
            pkgs.alsa-lib

            # testing
            pkgs.sipp
          ];
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
        };

        devShells.musl =
          let
            musltarget = "x86_64-unknown-linux-musl";
            muslcc = "${pkgsMusl.stdenv.cc}/bin/${musltarget}-cc";
            muslcxx = "${pkgsMusl.stdenv.cc}/bin/${musltarget}-c++";
          in
          pkgs.mkShell {
            buildInputs = [
              rustToolchain
              # native host tools
              pkgs.cmake
              pkgs.pkg-config
              # musl C compiler and static libopus
              pkgsMusl.stdenv.cc
              pkgsMusl.libopus
              pkgsMusl.openssl.dev
            ];
            RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
            CARGO_BUILD_TARGET = musltarget;
            CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";
            # point cargo's linker and C compiler at the musl cross toolchain
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER" = muslcc;
            CC_x86_64_unknown_linux_musl = muslcc;
            CXX_x86_64_unknown_linux_musl = muslcxx;
            # pkg-config for the musl libopus
            PKG_CONFIG_ALLOW_CROSS = "1";
            PKG_CONFIG_ALL_STATIC = "1";
            PKG_CONFIG_PATH = "${pkgsMusl.libopus}/lib/pkgconfig:${pkgsMusl.openssl.dev}/lib/pkgconfig";
          };
      }
    );
}

