{
  lib,
  makeRustPlatform,
  toolchain,
  rotation ? 90,
}:
let
  manifest = (lib.importTOML ../Cargo.toml).package;
  rustPlatform = makeRustPlatform {
    cargo = toolchain;
    rustc = toolchain;
  };
in
rustPlatform.buildRustPackage {
  pname = manifest.name;
  inherit (manifest) version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../.cargo
      ../src
      ../Cargo.toml
      ../Cargo.lock
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  # cargo-auditable embeds an ELF section, which rust-lld rejects in a PE image.
  auditable = false;

  env.GOP_ROTATE_DEGREES = toString rotation;

  buildPhase = ''
    runHook preBuild
    cargo build --release --offline
    runHook postBuild
  '';

  # systemd-boot expects driver filenames to end in <arch>.efi.
  installPhase = ''
    runHook preInstall
    install -Dm644 target/x86_64-unknown-uefi/release/gop-rotate.efi $out/gop-rotate-x64.efi
    runHook postInstall
  '';

  doCheck = false;
  dontFixup = true;

  meta = {
    inherit (manifest) description;
    homepage = manifest.repository;
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
