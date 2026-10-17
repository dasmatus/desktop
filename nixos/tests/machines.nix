# The virtual machine desktop's host and its template (libvirt.nix,
# machines.nix, docs/hypervisor.md): pm makes a machine from the template
# the image ships, the machine boots and takes an address on libvirt's
# network, its user pings the host and past it and NetworkManager finds it
# online, its guest agent lists its apps, and opening one sends its window
# to the host over vsock from the address libvirt gave that machine, which
# is how the desktop tells whose window it is. Then a Windows machine, as
# far as libvirt goes without Windows: defined with the image's drivers CD
# and started with its TPM.
#
# The desktop's own half, the gate that hands that connection to the
# machine's waypipe client and the compositor that labels the window, is
# derisk's and is tested there; here a listener on the same port stands in
# for it and records who connected.
#
# Only the host's modules, not the whole image. The machine runs under TCG:
# the test VM's KVM modules are kept out, so pm defines it to emulate rather
# than rely on nested virtualisation the runner may not give.
{ lib, ... }:

{
  name = "losos-desktop-machines";

  # The modules' packages, the guest template's among them, come from the
  # OS's overlay: the same derivations the image builds.
  node.pkgsReadOnly = false;

  # The internet as far as a machine can tell: the resolver the host asks,
  # which places GNOME's connectivity check here, and that check's page.
  nodes.internet =
    { config, pkgs, ... }:
    {
      networking.firewall.enable = false;
      services.dnsmasq = {
        enable = true;
        resolveLocalQueries = false;
        settings.address = "/nmcheck.gnome.org/${config.networking.primaryIPAddress}";
      };
      systemd.services.nmcheck = {
        wantedBy = [ "multi-user.target" ];
        script = ''
          mkdir -p /srv
          echo "NetworkManager is online" > /srv/check_network_status.txt
          exec ${pkgs.python3}/bin/python3 -m http.server --directory /srv 80
        '';
      };
    };

  nodes.machine =
    { nodes, pkgs, ... }:
    {
      imports = [
        ../modules/libvirt.nix
        ../modules/machines.nix
        ../modules/pm.nix
      ];
      nixpkgs.overlays = [ (import ../pkgs) ];

      # network.nix's packet filter, which the guests' DHCP has to get past.
      networking.nftables.enable = true;
      # libvirt's dnsmasq forwards the machines' questions to the host's
      # resolver, which asks the stand-in internet.
      networking.nameservers = [ nodes.internet.networking.primaryIPAddress ];

      boot.blacklistedKernelModules = [
        "kvm_intel"
        "kvm_amd"
      ];

      virtualisation = {
        cores = 4;
        memorySize = 4096;
        # The template is uploaded into libvirt's pool and the machine's
        # disk grows on top of it.
        diskSize = 12 * 1024;
      };

      # What the desktop's gate does first: accept on vsock port 7350 and
      # learn the peer's address, which the kernel stamps on the connection.
      environment.etc."vsock-listen.py".text = ''
        import socket, sys
        listener = socket.socket(socket.AF_VSOCK, socket.SOCK_STREAM)
        listener.bind((socket.VMADDR_CID_ANY, 7350))
        listener.listen()
        connection, (cid, port) = listener.accept()
        with open(sys.argv[1], "w") as peer:
            peer.write(str(cid))
      '';

      environment.systemPackages = [ pkgs.python3 ];
    };

  testScript = ''
    import base64
    import json
    import re
    import time

    virsh = "virsh -c qemu:///system"
    guest_run = "/run/current-system/sw/bin/losos-guest-run"

    def agent(command):
        reply = machine.succeed(f"{virsh} qemu-agent-command work '{json.dumps(command)}'")
        return json.loads(reply)["return"]

    def in_guest(path, *args):
        """Runs a program in the machine through its agent, as the desktop does."""
        pid = agent({
            "execute": "guest-exec",
            "arguments": {"path": path, "arg": list(args), "capture-output": True},
        })["pid"]
        for _ in range(120):
            status = agent({"execute": "guest-exec-status", "arguments": {"pid": pid}})
            if status["exited"]:
                out = base64.b64decode(status.get("out-data", "")).decode()
                err = base64.b64decode(status.get("err-data", "")).decode()
                assert status["exitcode"] == 0, f"{path} {args} failed: {err}"
                return out
            time.sleep(1)
        raise Exception(f"{path} {args} did not finish")

    machine.wait_for_unit("libvirt-defaults.service")

    with subtest("libvirt is the host's control plane"):
        machine.succeed("systemctl is-active libvirtd.service")
        machine.succeed(f"{virsh} pool-info default | grep -E '^State: +running'")
        machine.succeed(f"{virsh} pool-info default | grep -E '^Autostart: +yes'")
        machine.succeed(f"{virsh} net-info default | grep -E '^Active: +yes'")
        machine.succeed(f"{virsh} net-info default | grep -E '^Autostart: +yes'")
        machine.succeed("bash -lc 'test \"$LIBVIRT_DEFAULT_URI\" = qemu:///system'")
        machine.succeed("test -c /dev/vhost-vsock")
        machine.succeed("test -f /run/current-system/sw/share/losos/machines/losos-guest.qcow2")
        machine.fail("test -e /dev/kvm")

    with subtest("pm makes a machine from the shipped template"):
        machine.succeed(
            "mkdir -p /root/.config/pm/plugins",
            "cp /run/current-system/sw/share/pm/plugins/libvirt.wasm /root/.config/pm/plugins/",
            "pm sign /root/.config/pm/plugins/libvirt.wasm",
        )
        machine.succeed(
            "LIBVIRT_DEFAULT_URI=qemu:///system pm machine create work"
            " --image /run/current-system/sw/share/losos/machines/losos-guest.qcow2"
            " --label blue --memory 1536 --cpus 2 --disk 8"
        )
        metadata = machine.succeed(
            f"{virsh} metadata work https://losos-project.github.io/xmlns/pm-machine/1"
        )
        assert re.search(r"<\w*:?label>blue</", metadata), metadata
        # The template as a volume of its own, which every machine made from
        # it shares, and the machine's disk over it.
        machine.succeed(f"{virsh} vol-list default | grep -q 'pm-template-losos-guest-'")
        machine.succeed(f"{virsh} vol-info --pool default pm-machine-work.qcow2")

    with subtest("the machine boots, grows its disk and takes an address"):
        machine.succeed("LIBVIRT_DEFAULT_URI=qemu:///system pm machine start work")
        machine.wait_until_succeeds(
            f"{virsh} qemu-agent-command work '{{\"execute\":\"guest-ping\"}}'", timeout=900
        )
        size = int(in_guest("/run/current-system/sw/bin/df", "--output=size", "-B1G", "/").split()[-1])
        assert size >= 7, f"the root is {size} GiB, so it did not grow into the 8 GiB disk"
        machine.wait_until_succeeds(f"{virsh} domifaddr work | grep -q 192.168.122.", timeout=300)

    with subtest("the machine pings the host and past it, and is online"):
        # As the machine's user, from the login shell its apps get, the way
        # someone types it into the machine's Terminal.
        as_user = [
            "/run/current-system/sw/bin/systemd-run", "--quiet", "--user",
            "--machine=user@.host", "--wait", "--pipe", "--",
            "/run/current-system/sw/bin/bash", "-lc",
        ]
        in_guest(*as_user, "ping -c1 -W10 192.168.122.1")
        in_guest(*as_user, "ping -c1 -W10 nmcheck.gnome.org")
        # NetworkManager's connectivity check, which apps in the machine ask
        # before they go online.
        for _ in range(60):
            if in_guest("/run/current-system/sw/bin/nmcli", "-t", "-f", "CONNECTIVITY", "general").strip() == "full":
                break
            time.sleep(5)
        else:
            raise Exception("NetworkManager never found the machine online")

    with subtest("its user has the folders apps expect"):
        # home-folders.nix, made by the user's tmpfiles when its manager
        # started, which lingering does at boot.
        for folder in [".config/derisk", ".local/share/fonts", ".icons", ".local/state", "Documents"]:
            in_guest("/run/current-system/sw/bin/test", "-d", f"/home/user/{folder}")
        dirs = in_guest("/run/current-system/sw/bin/cat", "/home/user/.config/user-dirs.dirs")
        assert 'XDG_PICTURES_DIR="$HOME/Pictures"' in dirs, dirs

    with subtest("its agent lists the apps it can open"):
        apps = {}
        for line in in_guest(guest_run, "--list").splitlines():
            app, _, name = line.partition("\t")
            apps[app] = name
        for app in ["org.derisk.terminal", "org.losos.Danube", "org.derisk.calculator"]:
            assert app in apps, f"{app} is not in {sorted(apps)}"

    with subtest("an app's window reaches the host from the machine's own address"):
        xml = machine.succeed(f"{virsh} dumpxml work")
        found = re.search(r"<cid auto=.yes. address=.(\d+).", xml)
        assert found, f"the running machine has no vsock address: {xml}"
        cid = found.group(1)
        machine.succeed("systemd-run --unit=vsock-listen python3 /etc/vsock-listen.py /tmp/peer")
        machine.wait_until_succeeds("ss -l --vsock | grep -q 7350")
        in_guest(guest_run, "org.derisk.terminal")
        machine.wait_until_succeeds("test -s /tmp/peer", timeout=120)
        peer = machine.succeed("cat /tmp/peer").strip()
        assert peer == cid, f"the connection came from {peer}, the machine is {cid}"

    with subtest("pm removes it and keeps the template"):
        machine.succeed("LIBVIRT_DEFAULT_URI=qemu:///system pm machine remove work")
        machine.fail(f"{virsh} dominfo work")
        machine.fail(f"{virsh} vol-info --pool default pm-machine-work.qcow2")
        machine.succeed(f"{virsh} vol-list default | grep -q 'pm-template-losos-guest-'")

    with subtest("the image carries the CDs of guest tools where pm looks"):
        tools = "/run/current-system/sw/share/pm/machines"
        machine.succeed(f"test -s {tools}/windows-tools.iso")
        machine.succeed(f"test -s {tools}/linux-tools.iso")
        # pm finds them through the session's XDG_DATA_DIRS, so a login
        # shell's environment, not the test driver's.
        machine.succeed("bash -lc 'echo \"$XDG_DATA_DIRS\" | grep -q /run/current-system/sw/share'")

    with subtest("a Windows machine gets the drivers CD, a TPM and a screen, and starts"):
        # Windows itself cannot be installed here, so its CD is a stand-in:
        # what is checked is that libvirt takes the machine pm defines for it
        # and starts it, swtpm and all, with the host's drivers CD beside it.
        machine.succeed("head -c 1048576 /dev/zero > /tmp/win11.iso")
        machine.succeed(
            "bash -lc 'pm machine create win --installer /tmp/win11.iso"
            " --system windows --label red --memory 512 --disk 4'"
        )
        xml = machine.succeed(f"{virsh} dumpxml --inactive win")
        for expected in [
            "<tpm model='tpm-crb'>",
            "<graphics type='spice'>",
            "<clock offset='localtime'>",
            "pm-media-win11-",
            "pm-media-windows-tools-",
        ]:
            assert expected in xml, f"{expected} is not in {xml}"
        assert "<vsock" not in xml, xml
        machine.succeed("LIBVIRT_DEFAULT_URI=qemu:///system pm machine start win")
        machine.wait_until_succeeds(f"{virsh} domstate win | grep -qx running", timeout=60)
        machine.succeed("pgrep -x swtpm")

    with subtest("removing it keeps the CDs another machine may share"):
        machine.succeed("LIBVIRT_DEFAULT_URI=qemu:///system pm machine remove win")
        machine.fail(f"{virsh} dominfo win")
        machine.fail(f"{virsh} vol-info --pool default pm-machine-win.qcow2")
        machine.succeed(f"{virsh} vol-list default | grep -q 'pm-media-windows-tools-'")
        machine.succeed(f"{virsh} vol-list default | grep -q 'pm-media-win11-'")
  '';
}
