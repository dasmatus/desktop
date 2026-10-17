# Flatpak, with Flathub as the remote and derisk's Store in front of it.
#
# Apps the image does not ship come from Flathub as Flatpaks: sandboxed,
# updated on their own schedule, and kept in /var/lib/flatpak on the root
# partition, so a sysupdate of /usr leaves them alone and a factory reset
# removes them with the rest of the state.
{
  config,
  ...
}:

let
  flatpak = config.services.flatpak.package;
in
{
  # Installs flatpak's D-Bus system helper, its polkit rules, the `flatpak`
  # system user and the /var/lib/flatpak/exports entries on XDG_DATA_DIRS. It
  # needs xdg.portal, which desktop.nix enables with derisk's backend.
  #
  # flatpak's own polkit rule lets an active, local wheel member install,
  # update and remove apps for the whole system without a password. That is
  # the Store's normal path; anyone else is asked for an administrator's
  # password by derisk's polkit agent, or installs for themselves (`--user`).
  services.flatpak.enable = true;

  # Flathub, added once at boot and kept by flatpak afterwards. The repo file
  # is in the image rather than fetched, so the signing key every Flathub
  # install is verified against is the one this tree pins and reviews, not
  # whatever an HTTPS fetch at first boot returns. --if-not-exists makes it a
  # no-op on every later boot, and leaves a remote the user changed alone.
  systemd.services.flatpak-flathub = {
    description = "Add the Flathub Flatpak remote";
    wantedBy = [ "multi-user.target" ];
    # The system helper and /var/lib/flatpak's tmpfiles are both in place by
    # then; adding a remote needs neither the network nor the helper.
    after = [ "systemd-tmpfiles-setup.service" ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = "${flatpak}/bin/flatpak remote-add --system --if-not-exists flathub ${./flathub.flatpakrepo}";
    };
  };

  # The store is derisk's Store (org.derisk.store), one of the desktop's
  # own apps, so nothing here installs it: it reads the catalogue flatpak
  # keeps under /var/lib/flatpak/appstream and runs `flatpak` for the rest
  # (docs/store.md). Bazaar was the store before it and is not coming back:
  # its GTK 4 window took neither scrolling nor clicks under derisk, and the
  # Store sorts Flathub into categories and shows apps' release notes on
  # its front page, which Bazaar did not.
}
