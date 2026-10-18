# Windows app support's containers (windows-apps.nix, docs/windows-apps.md):
# a person in systemd-homed's UID range gets rootless Podman from the
# image's subordinate IDs, and runs a container from a compose file through
# the `pm` WinBoat and its setup run, which signs the image's podman plugin
# for itself the first time. Root gets neither Podman nor pm's containers.
#
# Windows itself needs KVM and Microsoft's download, so the container here
# is a small image built offline, loaded into the person's own store.
{ ... }:

{
  name = "losos-desktop-windows-apps";

  # pm, its plugins and windows-app-support from the OS's overlay: the
  # derivations the image builds.
  node.pkgsReadOnly = false;

  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ ../modules/windows-apps.nix ];
      nixpkgs.overlays = [ (import ../pkgs) ];

      # A person with homed's first UID, as the image's people have. Made by
      # NixOS here, which would give them subordinate IDs of its own beside
      # the module's.
      users.users.alice = {
        isNormalUser = true;
        uid = 60001;
        autoSubUidGidRange = false;
        linger = true;
      };

      environment.etc."windows-apps-test/image.tar".source = pkgs.dockerTools.buildImage {
        name = "localhost/test";
        tag = "latest";
        copyToRoot = [ pkgs.busybox ];
        config.Cmd = [
          "sleep"
          "infinity"
        ];
      };
      # The pm WinBoat and its setup run.
      environment.etc."windows-apps-test/pm".source = "${pkgs.windows-app-support.pm}/bin/pm";
      environment.etc."windows-apps-test/compose.yml".text = ''
        services:
          test:
            image: localhost/test:latest
            container_name: test
            network_mode: none
      '';

      virtualisation.memorySize = 2048;
    };

  testScript =
    let
      pm = "/etc/windows-apps-test/pm";
    in
    ''
      def alice(command):
          return machine.succeed(
              f"su -l alice -c 'XDG_RUNTIME_DIR=/run/user/60001 {command}'"
          )

      machine.wait_for_unit("multi-user.target")
      machine.wait_for_unit("user@60001.service")

      with subtest("root has no Podman"):
          out = machine.fail("podman info 2>&1")
          assert "rootless only" in out, out
          machine.fail("systemctl is-active podman.socket")

      with subtest("a person's Podman maps their containers onto their subordinate IDs"):
          uid_map = alice("podman unshare cat /proc/self/uid_map")
          assert "100000" in uid_map and "65536" in uid_map, uid_map

      with subtest("pm signs the podman plugin for itself and runs a container"):
          alice("podman load --quiet < /etc/windows-apps-test/image.tar")
          versions = alice("${pm} container version")
          assert "podman version" in versions, versions
          assert "podman-compose version" in versions, versions
          machine.succeed(
              "test -s /home/alice/.local/state/windows-app-support/config/pm/plugins/podman.wasm"
          )
          alice("${pm} container up /etc/windows-apps-test/compose.yml")
          assert alice("${pm} container list").split() == ["test"]
          assert alice("${pm} container status test").strip() == "running"
          alice("${pm} container stop test")
          assert alice("${pm} container status test").strip() == "exited"
          alice("${pm} container down /etc/windows-apps-test/compose.yml")
          assert alice("${pm} container list").split() == []

      with subtest("root's requests are refused"):
          out = machine.fail("${pm} container list 2>&1")
          assert "rootless" in out, out
    '';
}
