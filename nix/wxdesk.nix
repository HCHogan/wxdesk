{ lib, rustPlatform }:
rustPlatform.buildRustPackage {
  pname = "wxdesk";
  version = "0.1.0";
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [
    "-p"
    "wxdesk"
  ];
  meta.mainProgram = "wxdesk";
}
