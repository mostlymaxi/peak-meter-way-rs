{
  lib,
  rustPlatform,
  pkg-config,
  pipewire,
}:

let
  cargo = lib.importTOML ./Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = cargo.package.name;
  inherit (cargo.package) version;

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  # pipewire-sys generates its bindings with bindgen against the PipeWire headers.
  nativeBuildInputs = [
    pkg-config
    rustPlatform.bindgenHook
  ];
  buildInputs = [ pipewire ];

  meta = {
    inherit (cargo.package) description;
    license = lib.licenses.mit;
    mainProgram = "peak-meter-way";
    platforms = lib.platforms.linux;
  };
}
