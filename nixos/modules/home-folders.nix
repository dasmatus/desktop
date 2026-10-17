# The folders in a home that apps expect to find rather than make, relative
# to it. A missing one is not always shrugged off: Danube's sandbox binds
# each of the last seven read-only (src/danube/crates/danube-sandbox,
# browser.rs), and a Flatpak app, a GTK or Qt toolkit or a font cache that
# writes into one of the base directories finds it there instead of failing
# on its parent. accounts.nix makes them for every user of the OS when their
# session starts, and guest/home.nix for a machine's user through Home
# Manager.
[
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
