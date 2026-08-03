{
  pkgs,
  gitignore,
  buildType ? "release",
  buildStd ? false,
  cargoBuildFlags ? [ ],
  rustLibSrc ? pkgs.rustPlatform.rustLibSrc,
  rustVendorSrc ? pkgs.rustPlatform.rustVendorSrc,
}:
let
  lib = pkgs.lib;
  manifest = (pkgs.lib.importTOML ./Cargo.toml).package;

  # Cargo discovers build-std sources inside the compiler sysroot.
  rustc = pkgs.rustPlatform.rust.rustc;
  rustcSysroot = pkgs.symlinkJoin {
    name = "rustc-with-libsrc-sysroot";
    paths = [ rustc.unwrapped ];
    postBuild = ''
      mkdir -p $out/lib/rustlib/src/rust
      ln -s ${rustLibSrc} $out/lib/rustlib/src/rust/library
    '';
  };
  rustcWithLibSrc = rustc.override { sysroot = rustcSysroot; };

  buildRustPackage =
    if buildStd
    then pkgs.rustPlatform.buildRustPackage.override { rustc = rustcWithLibSrc; }
    else pkgs.rustPlatform.buildRustPackage;

  baseCargoDeps = pkgs.rustPlatform.importCargoLock {
    lockFile = ./Cargo.lock;
  };
  cargoDeps =
    if buildStd
    then pkgs.symlinkJoin {
      name = "cargo-vendor-dir";
      paths = [
        baseCargoDeps
        rustVendorSrc
      ];
    }
    else baseCargoDeps;

  buildStdFlags = lib.optionals buildStd [
    "-Zunstable-options"
    "-Zbuild-std=std,panic_abort"
    "-Zbuild-std-features=optimize_for_size"
  ];
in
buildRustPackage {
  pname = manifest.name;
  version = manifest.version;

  src = gitignore.gitignoreSource ./.;

  # Keep the checked-in manifest compatible with stable Cargo.
  postPatch = lib.optionalString buildStd ''
    sed -i '1i cargo-features = ["panic-immediate-abort"]\n' Cargo.toml
  '';

  inherit buildType cargoDeps;
  cargoBuildFlags = cargoBuildFlags ++ buildStdFlags;
  RUSTFLAGS = lib.optionals buildStd [
    "-Cforce-frame-pointers=no"
    "-Cforce-unwind-tables=no"
  ];

  env = lib.optionalAttrs buildStd {
    RUSTC_BOOTSTRAP = 1;
    RUSTC = "${rustcWithLibSrc}/bin/rustc";
    CARGO_PROFILE_MINI_PANIC = "immediate-abort";
  };

  stripAllList = lib.optionals (buildType == "mini") [ "bin" ];
  stripAllFlags = lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ "-x" ];
}
