# Sonne, the code editor (nixos/pkgs/sonne.nix, docs/sonne.md), on the PC
# image, which is the desktop and tablet image alike. The Halium image goes
# without: it is one image for every Treble phone, and Sonne's window, a
# desktop editor with no touch input, does not fit a screen under 600
# logical pixels on its short side.
{ lib, pkgs, ... }:

{
  environment.systemPackages = [ pkgs.sonne ];

  # What a text or source file opens in from Files, the palette and
  # xdg-open, all of which read mimeapps.list. derisk's own Text Editor is
  # built into derisk and stays, but it never claimed these: its desktop
  # entry lives inside the binary and names no types. Source files are
  # listed by their own types as well as text/plain because xdg-open
  # outside GNOME and KDE looks a type up as it is, not through the
  # text/plain it is a subclass of. zed: is the scheme derisk's palette
  # opens Sonne's agent with ("Ask Sonne's agent").
  xdg.mime.defaultApplications = lib.genAttrs [
    "text/plain"
    "application/x-zerosize"
    "text/markdown"
    "text/x-csrc"
    "text/x-chdr"
    "text/x-c++src"
    "text/x-c++hdr"
    "text/x-rust"
    "text/x-python"
    "text/x-go"
    "text/x-java"
    "text/x-makefile"
    "text/x-cmake"
    "text/css"
    "text/javascript"
    "application/javascript"
    "application/json"
    "application/toml"
    "application/x-yaml"
    "application/xml"
    "application/x-shellscript"
    "text/x-log"
    "x-scheme-handler/zed"
  ] (_: "io.github.dasmatus.Sonne.desktop");
}
