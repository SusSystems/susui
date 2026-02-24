{
  description = "sus ui — nix build dashboard";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs@{ self, nixpkgs, flake-parts, rust-overlay, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];

      perSystem = { config, self', pkgs, lib, system, ... }:
        let
          overlays = [ (import rust-overlay) ];
          rustPkgs = import nixpkgs { inherit system overlays; };

          rustToolchain = rustPkgs.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" "rust-analyzer" ];
          };

          nativeBuildInputs = [
            rustPkgs.pkg-config
            rustPkgs.openssl
          ];

          buildInputs = [
            rustPkgs.openssl
          ] ++ lib.optionals pkgs.stdenv.isDarwin [
            pkgs.darwin.apple_sdk.frameworks.Security
            pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
          ];

          susui = rustPkgs.rustPlatform.buildRustPackage {
            pname = "susui";
            version = "0.1.0";
            src = lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;

            inherit nativeBuildInputs buildInputs;

            meta = {
              description = "sus ui — nix build dashboard";
              homepage = "https://github.com/SusSystems/susui";
              license = lib.licenses.mit;
              mainProgram = "susui";
            };
          };

          susuiWithSetup = pkgs.symlinkJoin {
            name = "susui-${susui.version}";
            paths = [ susui ];
            postBuild = ''
              install -Dm755 ${./systemd/susui-setup.sh} $out/bin/susui-setup
              install -Dm755 ${./systemd/susui-push.sh} $out/share/susui/susui-push.sh
            '';
            meta = susui.meta;
          };
        in
        {
          packages = {
            default = susuiWithSetup;
            susui = susuiWithSetup;
            unwrapped = susui;
          };

          devShells.default = rustPkgs.mkShell {
            nativeBuildInputs = nativeBuildInputs ++ [ rustToolchain ];
            inherit buildInputs;

            RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";

            shellHook = ''
              echo "╭─ sus ui dev shell ──────────╮"
              echo "│  cargo build    — build      │"
              echo "│  cargo run      — run         │"
              echo "│  cargo test     — test        │"
              echo "│  susui serve .  — dashboard   │"
              echo "╰──────────────────────────────╯"
            '';
          };

          checks = {
            inherit susui;

            clippy = rustPkgs.rustPlatform.buildRustPackage {
              pname = "susui-clippy";
              version = "0.1.0";
              src = lib.cleanSource ./.;
              cargoLock.lockFile = ./Cargo.lock;
              inherit nativeBuildInputs buildInputs;
              buildPhase = ''
                cargo clippy -- -D warnings
              '';
              installPhase = "mkdir -p $out";
            };
          };
        };
    };
}
