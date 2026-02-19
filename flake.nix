{
  description = "sus ui — nix build dashboard";

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
        pkgs = import nixpkgs { inherit system overlays; };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };

        nativeBuildInputs = with pkgs; [
          rustToolchain
          pkg-config
          openssl
        ];

        buildInputs = with pkgs; [
          openssl
        ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
          pkgs.darwin.apple_sdk.frameworks.Security
          pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
        ];
      in
      {
        packages = {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "susui";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            inherit nativeBuildInputs buildInputs;

            meta = with pkgs.lib; {
              description = "sus ui — nix build dashboard";
              homepage = "https://github.com/SusSystems/susui";
              license = licenses.mit;
              mainProgram = "susui";
            };
          };
        };

        devShells.default = pkgs.mkShell {
          inherit nativeBuildInputs buildInputs;

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
          clippy = pkgs.rustPlatform.buildRustPackage {
            pname = "susui-clippy";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            inherit nativeBuildInputs buildInputs;
            buildPhase = ''
              cargo clippy -- -D warnings
            '';
            installPhase = "mkdir -p $out";
          };
        };
      }
    );
}
