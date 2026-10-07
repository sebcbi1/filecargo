# Build / run environment for the GUI on NixOS (other systems install the same libraries with
# their package manager, see the README's "Build from source"):
#   nix-shell --run "cargo test -p filecargo-gui"
{ pkgs ? import <nixpkgs> { } }:
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [ pkg-config cmake clang ];
  buildInputs = with pkgs; [
    fontconfig freetype wayland libxkbcommon libxcb libx11
    xorg.libXcursor xorg.libXi xorg.libXrandr vulkan-loader libGL alsa-lib openssl
  ];
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (with pkgs; [
    wayland libxkbcommon libxcb libx11 vulkan-loader libGL fontconfig freetype
  ]);
}
