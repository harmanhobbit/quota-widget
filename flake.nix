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
    };
}
