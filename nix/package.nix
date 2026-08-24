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
, openssh
, tailscale
}:

let
  # nixpkgs' npmConfigHook still hands npm three config names npm dropped:
  # `nodedir`, `platform` and `arch`. npm 11 warns "Unknown env config ...
  # This will stop working in the next major version of npm" for each, once
  # per npm invocation (so twice per build: `npm ci`/`npm rebuild` inside the
  # hook, then `npm run build`), and npm 12 will make them errors — which
  # would break `nix build` for everyone installing from this flake.
  #
  # As of this nixpkgs pin the hook on nixpkgs master still exports the old
  # names, so there is no newer revision to move to; carry the translation
  # here instead, using the mechanisms npm and node-gyp document today:
  #
  #   npm_config_nodedir  -> npm_package_config_node_gyp_nodedir
  #        node-gyp's own env namespace, added for precisely this deprecation
  #        (nodejs/node-gyp#3156) and read in preference to npm_config_* by
  #        the node-gyp bundled with this nixpkgs' nodejs.
  #   npm_config_arch     -> npm_config_cpu + npm_package_config_node_gyp_arch
  #        the old key served two consumers: npm selects platform-specific
  #        optional deps by `cpu`, node-gyp derives `target_arch` from `arch`.
  #        Both keep their value, so native addons still build for the target.
  #   npm_config_platform -> npm_config_os
  #        npm's supported name for optional-dependency OS selection.
  #
  # `npm_config_node_gyp` is a live npm config and is left alone.
  #
  # Remove this whole override once the nixpkgs pin carries a hook that no
  # longer exports the deprecated names — `--replace-fail` turns that day into
  # a loud build failure rather than a silent no-op.
  npmConfigHookCompat = npmHooks.npmConfigHook.overrideAttrs (old: {
    buildCommand = old.buildCommand + ''
      substituteInPlace $out/nix-support/setup-hook \
        --replace-fail 'export npm_config_nodedir=' \
                       'export npm_package_config_node_gyp_nodedir=' \
        --replace-fail 'export npm_config_arch=' \
                       'export npm_config_cpu=' \
        --replace-fail 'export npm_config_platform=' \
                       'export npm_package_config_node_gyp_arch="$npm_config_cpu"
    export npm_config_os='
    '';
  });
in

rustPlatform.buildRustPackage rec {
  pname = "quota-widget";
  # Single source of truth: the workspace Cargo.toml. tauri.conf.json and
  # package.json derive from it too, so a bump is a one-line edit there.
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  # `lib.cleanSource` only drops VCS and editor cruft — it keeps target/, which
  # is ~7.6 GB in a working checkout and was being copied into the store on
  # every evaluation. Filter to the inputs the build actually reads.
  src =
    let
      keep = [
        "Cargo.toml"
        "Cargo.lock"
        "package.json"
        "package-lock.json"
        "vite.config.js"
        "index.html"
        "crates"
        "src"
        "src-tauri"
      ];
      root = ../.;
    in
    lib.fileset.toSource {
      inherit root;
      fileset = lib.fileset.unions (map (p: root + "/${p}") keep);
    };

  cargoLock.lockFile = ../Cargo.lock;

  # Rust 1.97's LLVM can overflow rustc's default worker-thread stack while
  # doing this app's fat-LTO link. Keep LTO's smaller binary rather than
  # weakening the release profile; rustc recommends this larger stack.
  RUST_MIN_STACK = "16777216";

  # Only the two lockfile-ish files decide the npm closure, so scoping this
  # separately keeps the fixed-output hash stable when app source changes.
  npmDeps = fetchNpmDeps {
    name = "${pname}-${version}-npm-deps";
    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [ ../package.json ../package-lock.json ];
    };
    # Regenerate whenever package.json/package-lock.json changes (a
    # mismatched `got:` is printed by `nix build .#quota-widget`, or compute
    # it directly with `prefetch-npm-deps package-lock.json`).
    hash = "sha256-4AvIty9QzZEuDXNQ/O26AXQK2I791M+XkmI1jPhob30=";
  };

  nativeBuildInputs = [
    nodejs
    npmConfigHookCompat
    pkg-config
    wrapGAppsHook3
  ];

  buildInputs = [
    glib
    gtk3
    webkitgtk_4_1
    libsoup_3
  ];

  # Vite must run before cargo: tauri-build embeds ../dist at compile time.
  preBuild = ''
    # The npm hook's `npm rebuild` runs esbuild's install script (the only
    # lifecycle script in the Linux dependency tree — see package.json's
    # `allowScripts`), which is what puts the platform binary in place. A
    # rebuild that quietly no-ops would leave vite to fail much later with a
    # confusing esbuild error, so assert the binary is here first.
    node_modules/.bin/esbuild --version > /dev/null \
      || { echo "ERROR: esbuild's platform binary is unusable after npm rebuild"; exit 1; }

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
    # Launch under XWayland: native Wayland has no always-on-top protocol
    # (xdg-shell lacks it; tao#1134), so the popup would sink behind other
    # windows. Harmless on X11, where GDK_BACKEND=x11 is already the case.
    cat > $out/share/applications/quota-widget.desktop <<EOF
    [Desktop Entry]
    Name=Quota Widget
    Comment=AI provider usage and credits in the tray
    Exec=env GDK_BACKEND=x11 quota-widget
    Icon=quota-widget
    Type=Application
    Categories=Utility;
    EOF
  '';

  preFixup = ''
    gappsWrapperArgs+=(
      # The Hermes remote source shells out to `ssh` or `tailscale ssh`.
      --prefix PATH : ${lib.makeBinPath [ openssh tailscale ]}
    )
  '';

  meta = {
    description = "System-tray widget showing AI provider usage and credits";
    homepage = "https://github.com/harmanhobbit/quota-widget";
    license = lib.licenses.asl20;
    mainProgram = "quota-widget";
    platforms = lib.platforms.linux;
  };
}
