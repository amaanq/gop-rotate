{
  inputs = {
    nixpkgs.url = "https://channels.nixos.org/nixpkgs-unstable/nixexprs.tar.xz";
    fenix.url = "github:nix-community/fenix";
    fenix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    { self, ... }@inputs:
    let
      inherit (inputs) nixpkgs fenix;
      inherit (nixpkgs) lib;
      forAllSystems = lib.genAttrs lib.systems.doubles.linux;
      pkgsFor = system: nixpkgs.legacyPackages.${system};

      toolchainFor =
        system:
        let
          toolchains = fenix.packages.${system};
        in
        toolchains.combine [
          toolchains.stable.cargo
          toolchains.stable.clippy
          toolchains.stable.rustc
          toolchains.targets.x86_64-unknown-uefi.stable.rust-std
        ];
    in
    {
      packages = forAllSystems (
        system:
        let
          gop-rotate = (pkgsFor system).callPackage ./nix/package.nix {
            toolchain = toolchainFor system;
          };
        in
        {
          inherit gop-rotate;
          default = gop-rotate;
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = pkgs.mkShell {
            packages = [
              (toolchainFor system)
              fenix.packages.${system}.latest.rustfmt
              pkgs.tombi
              pkgs.qemu
            ];
            OVMF = "${pkgs.OVMF.fd}/FV";
            SYSTEMD_BOOT = "${pkgs.systemd}/lib/systemd/boot/efi/systemd-bootx64.efi";
          };
        }
      );

      formatter = forAllSystems (system: (pkgsFor system).nixfmt);
    };
}
