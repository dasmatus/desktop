# The guest's user's home, through Home Manager: the folders apps expect to
# find in it (modules/home-folders.nix), the same ones the OS's users get
# from tmpfiles (modules/accounts.nix). The guest knows its one user's name,
# so its home is a Home Manager configuration, built with the template and
# activated by home-manager-user.service at every boot, before the user's
# manager starts the waypipe server and the apps.
#
# The flake imports this beside Home Manager's NixOS module
# (losos.machines.guestModules), since nothing in nixos/ can reach the
# flake's inputs.
{ config, ... }:

let
  # The account nixos/guest/default.nix makes.
  user = "user";
  home = config.users.users.${user}.home;
in
{
  # Home Manager's activation starts by asking Nix to build an empty
  # expression (its activation-init.sh) and stops when it cannot, so the
  # guest gets the Nix daemon that guest/default.nix otherwise leaves out.
  nix.enable = true;

  home-manager = {
    useGlobalPkgs = true;
    useUserPackages = true;
    users.${user} =
      { lib, ... }:
      {
        home.stateVersion = "26.05";
        home.activation.folders = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
          run mkdir -p ${
            lib.escapeShellArgs (map (dir: "${home}/${dir}") (import ../modules/home-folders.nix))
          }
        '';
      };
  };
}
