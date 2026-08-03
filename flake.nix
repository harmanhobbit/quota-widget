{
  description = "System-tray widget showing AI provider usage and credits";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        quota-widget = pkgs.callPackage ./nix/package.nix { };
        default = quota-widget;
      });

      # For device flakes: `nixpkgs.overlays = [ quota-widget.overlays.default ];`
      # then `environment.systemPackages = [ pkgs.quota-widget ];`
      overlays.default = final: prev: {
        quota-widget = final.callPackage ./nix/package.nix { };
      };

      # `nix develop`, or automatically via direnv (.envrc). Everything needed
      # to build and run the app from a checkout — which is the cheap loop:
      # GitHub's Windows runner bills at a 2x minute multiplier, local Linux
      # builds bill nothing.
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          # cargo/rustc/rust-analyzer come from the same nixpkgs pin the
          # package build uses, so a local build and `nix build` agree.
          nativeBuildInputs = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            nodejs
            pkg-config
            # `cargo tauri dev/build`. The package build deliberately avoids
            # the CLI (see nix/package.nix), but it is the ergonomic path
            # interactively.
            cargo-tauri
          ];

          # Mirrors nix/package.nix's buildInputs: without these, gdk-sys's
          # build script fails at `pkg-config --libs gdk-3.0` and the whole
          # src-tauri crate is uncompilable.
          buildInputs = with pkgs; [
            glib
            gtk3
            webkitgtk_4_1
            libsoup_3
            openssh
            tailscale
          ];

          # The tray needs libappindicator resolvable at *run* time, and
          # webkitgtk's loader is not on the default path in a dev shell.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (with pkgs; [
            libayatana-appindicator
            webkitgtk_4_1
            gtk3
            glib
          ]);

          shellHook = ''
            # Native Wayland has no always-on-top protocol (tao#1134), so the
            # popup sinks behind other windows. The .desktop entry sets this
            # for installed builds; do the same for `cargo tauri dev`.
            export GDK_BACKEND=x11

            # `npm run` puts node_modules/.bin on PATH for its child; `cargo
            # tauri dev` does not — it runs beforeDevCommand through a bare
            # `sh -c`, which fails with "vite: command not found". Adding it
            # here makes both entry points work.
            export PATH="$PWD/node_modules/.bin:$PATH"

            [ -d node_modules ] || echo "note: run 'npm ci' first — node_modules is missing"
            echo "quota-widget dev shell — cargo test -p quota-core | npm run build | npm run tauri dev"
          '';
        };
      });
    };
}
