# The folders in a home that apps expect to find rather than make. A
# missing one is not always shrugged off: Danube's sandbox binds the fonts,
# icons and derisk folders below read-only (src/danube/crates/danube-sandbox,
# browser.rs), and a Flatpak app, a toolkit or a font cache that writes into
# one of the base directories finds it there instead of failing on its
# parent.
#
# Made by the user's own systemd-tmpfiles when their manager starts, for
# whoever it is: `%h` is that user's home, since the image knows none of
# their names (homed makes them at first boot). homed's /etc/skel would not
# do, as it is copied once when a home is made and never reaches a user made
# before a folder joined the list. base.nix imports this for the OS, and
# guest/default.nix for a machine's user.
{ lib, ... }:

let
  # The XDG user directories: what xdg-user-dirs makes, in English, under
  # the names its spec reads from ~/.config/user-dirs.dirs.
  userDirs = {
    DESKTOP = "Desktop";
    DOCUMENTS = "Documents";
    DOWNLOAD = "Downloads";
    MUSIC = "Music";
    PICTURES = "Pictures";
    PUBLICSHARE = "Public";
    TEMPLATES = "Templates";
    VIDEOS = "Videos";
  };

  # In the form xdg-user-dirs writes it, which is what the spec's readers
  # (GLib, Qt, xdg-user-dir, derisk's portal) parse.
  userDirsFile = lib.concatStrings (
    lib.mapAttrsToList (name: dir: "XDG_${name}_DIR=\"$HOME/${dir}\"\\n") userDirs
  );

  folders = [
    # The XDG base directories, at their default places.
    ".config"
    ".cache"
    ".local/share"
    ".local/state"
    ".local/bin"
    # What Danube shows: the user's own fonts, font settings and icons, in
    # both the XDG and the older places, and derisk's settings and data.
    ".local/share/fonts"
    ".fonts"
    ".config/fontconfig"
    ".local/share/icons"
    ".icons"
    ".local/share/derisk"
    ".config/derisk"
  ]
  ++ lib.attrValues userDirs;
in
{
  # `f` writes the file only where there is none, so a user who moved a
  # folder elsewhere keeps their own. tmpfiles reads C escapes in it.
  systemd.user.tmpfiles.rules = map (dir: "d %h/${dir} - - - -") folders ++ [
    "f %h/.config/user-dirs.dirs - - - - ${userDirsFile}"
  ];
}
