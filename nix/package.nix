# Builds the Tauri app without the tauri CLI: vite builds dist/ (embedded into
# the binary by tauri's build script via frontendDist), then a plain cargo
# build of the src-tauri crate. No bundle step — just the binary, a .desktop
# entry, and icons.
{ lib
, rustPlatform
, fetchNpmDeps
, npmHooks
, nodejs
, pkg-config
, wrapGAppsHook3
, glib
, gtk3
, webkitgtk_4_1
, libsoup_3
, libayatana-appindicator
, openssh
}:

rustPlatform.buildRustPackage rec {
  pname = "quota-widget";
  # Single source of truth: the workspace Cargo.toml. tauri.conf.json and
  # package.json derive from it too, so a bump is a one-line edit there.
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  src = lib.cleanSource ../.;

  cargoLock.lockFile = ../Cargo.lock;

  npmDeps = fetchNpmDeps {
    name = "${pname}-${version}-npm-deps";
    src = lib.cleanSource ../.;
    hash = "sha256-XqkPzGXTWiJU3l0M2YvNOFNw29nCDhc2HEI7zR6HY34=";
  };

  nativeBuildInputs = [
    nodejs
    npmHooks.npmConfigHook
    pkg-config
    wrapGAppsHook3
  ];

  buildInputs = [
    glib
    gtk3
    webkitgtk_4_1
    libsoup_3
    libayatana-appindicator
  ];

  # Vite must run before cargo: tauri-build embeds ../dist at compile time.
  preBuild = ''
    npm run build
  '';

  buildAndTestSubdir = "src-tauri";

  # Serve the embedded frontend, not the vite dev-server URL.
  buildFeatures = [ "custom-protocol" ];

  # quota-core's tests run in CI; the app crate has none.
  doCheck = false;

  postInstall = ''
    # staticlib/cdylib byproducts of the tauri crate-type list — not needed
    rm -rf $out/lib
    install -Dm644 src-tauri/icons/128x128.png \
      $out/share/icons/hicolor/128x128/apps/quota-widget.png
    install -Dm644 src-tauri/icons/32x32.png \
      $out/share/icons/hicolor/32x32/apps/quota-widget.png
    mkdir -p $out/share/applications
    cat > $out/share/applications/quota-widget.desktop <<EOF
    [Desktop Entry]
    Name=Quota Widget
    Comment=AI provider usage and credits in the tray
    Exec=quota-widget
    Icon=quota-widget
    Type=Application
    Categories=Utility;
    EOF
  '';

  preFixup = ''
    gappsWrapperArgs+=(
      # The tray icon dlopens libayatana-appindicator at runtime.
      --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath [ libayatana-appindicator ]}
      # The Hermes remote source shells out to `ssh`.
      --prefix PATH : ${lib.makeBinPath [ openssh ]}
    )
  '';

  meta = {
    description = "System-tray widget showing AI provider usage and credits";
    homepage = "https://github.com/harmanhobbit/quota-widget";
    license = lib.licenses.mit;
    mainProgram = "quota-widget";
    platforms = lib.platforms.linux;
  };
}
