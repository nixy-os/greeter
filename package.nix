{
  lib,
  rustPlatform,
  pkg-config,
  wrapGAppsHook4,
  gtk4,
}:
rustPlatform.buildRustPackage {
  pname = "nixy-greeter";
  version = "0.1.0";
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./LICENSE
      ./src
      ./assets
      ./tests
    ];
  };
  cargoHash = "sha256-4ZJy6TFH4dwmMqa+n+8yDvVuWBoqGyNKzpTBCNgkylU=";
  nativeBuildInputs = [
    pkg-config
    wrapGAppsHook4
  ];
  buildInputs = [ gtk4 ];
  doCheck = true;

  meta = {
    description = "Minimal Nixy Wayland login form for greetd";
    license = lib.licenses.mit;
    mainProgram = "nixy-greeter";
    platforms = [ "x86_64-linux" ];
  };
}
