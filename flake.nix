{
  description = "leaves - A visual disk usage analyzer for terminal";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    gitignore-src.url = "github:hercules-ci/gitignore.nix";
  };

  outputs = { self, nixpkgs, flake-utils, gitignore-src }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        gitignore = pkgs.callPackage gitignore-src {};

        packageVariants =
          packagePkgs:
          let
            callLeaves =
              args:
              packagePkgs.callPackage ./package.nix ({
                inherit gitignore;
                # Native and cross compilers are the same pinned Rust release.
                inherit (pkgs.rustPlatform) rustLibSrc rustVendorSrc;
              } // args);
          in
          {
            default = callLeaves {};
            mini = callLeaves {
              buildType = "mini";
            };
            nano = callLeaves {
              buildType = "mini";
              buildStd = true;
              cargoBuildFlags = [ "--no-default-features" ];
            };
          };

        nativePackages = packageVariants pkgs;
        targetPackages = {
          macos =
            if pkgs.stdenv.hostPlatform.isDarwin
            then nativePackages
            else packageVariants pkgs.pkgsCross.aarch64-darwin;
          linux = {
            gnu =
              if pkgs.stdenv.hostPlatform.isLinux && pkgs.stdenv.hostPlatform.isGnu
              then nativePackages
              else packageVariants pkgs.pkgsCross.gnu64;
            musl =
              if pkgs.stdenv.hostPlatform.isLinux && pkgs.stdenv.hostPlatform.isMusl
              then nativePackages
              else packageVariants pkgs.pkgsCross.musl64;
          };
        };

        multiPlatformPackage =
          name:
          let
            macos = targetPackages.macos.${name};
            linux = {
              gnu = targetPackages.linux.gnu.${name};
              musl = targetPackages.linux.musl.${name};
            };
          in
          pkgs.runCommand "leaves-${name}-macos-linux" {
            passthru = { inherit macos linux; };
          } ''
            mkdir -p $out/macos/bin $out/linux/gnu/bin $out/linux/musl/bin
            ln -s ${macos}/bin/leaves $out/macos/bin/leaves
            ln -s ${linux.gnu}/bin/leaves $out/linux/gnu/bin/leaves
            ln -s ${linux.musl}/bin/leaves $out/linux/musl/bin/leaves
          '';
      in
      {
        packages = pkgs.lib.genAttrs (pkgs.lib.attrNames nativePackages) multiPlatformPackage;
        devShells.default = pkgs.mkShell {
          inputsFrom = [ nativePackages.default ];
          packages = with pkgs; [ cargo rustc rust-analyzer clippy ];
        };
      }
    );
}
