{
  _workspace ? import ./.,
  pkgs ? _workspace.pkgs,
  libraries ? _workspace.libraries,
  rust-toolchain ? _workspace.rust-toolchain,
}:
let basePackages = libraries ++ [ rust-toolchain ];
in {
  default = pkgs.mkShell {
    LD_LIBRARY_PATH = "${pkgs.lib.makeLibraryPath libraries}";
    packages = basePackages;
  };

  perf = pkgs.mkShell {
    LD_LIBRARY_PATH = "${pkgs.lib.makeLibraryPath libraries}";
    packages = with pkgs; [
      perf
      cargo-flamegraph
    ] ++ basePackages;
  };
}
