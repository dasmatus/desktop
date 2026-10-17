# The virtual machine desktop's host and its template (quickemu.nix,
# machines.nix, docs/hypervisor.md): pm makes a machine from the template
# the image ships, quickemu boots it and it takes an address on the
# machines' network, its user pings the host and past it and
# NetworkManager finds it online, its guest agent lists its apps, and
# opening one sends its window to the host over vsock from the address its
# QEMU was given, which is how the desktop tells whose window it is. The
# machine looks .local names up through the host, so it finds a box on the
# host's network that its own multicast could never reach past the host's
# NAT. A LosOS boxes machine finds that box through the host's resolved and
# offers it as an app, and the first machine's connections to the box go
# through it. Then a Windows machine, as far as quickemu goes without
# Windows: made with the image's drivers CD and started with its TPM.
#
# The desktop's own half, the gate that hands that connection to the
# machine's waypipe client and the compositor that labels the window, is
# derisk's and is tested there; here a listener on the same port stands in
# for it and records who connected.
#
# Only the host's modules, not the whole image. The machine runs under TCG:
# the test VM's KVM modules are kept out, so quickemu emulates rather than
# relying on nested virtualisation the runner may not give. The driver runs
# as root, so the machines are root's, under its own home, and run as
# system services rather than a user's (pm's quickemu plugin).
{ lib, ... }:

{
  name = "losos-desktop-machines";

  # The modules' packages, the guest template's among them, come from the
  # OS's overlay: the same derivations the image builds.
  node.pkgsReadOnly = false;

  # A LosOS box on the host's LAN, as far as finding one goes: it answers
  # for its name and announces `_losos._tcp` over mDNS, with Avahi as boxes
  # before systemd-resolved did, and serves its admin pages and the
  # /setup/state.json every box has (nixos/tests/losos-boxes.nix has the
  # same stand-in).
  nodes.lanbox =
    { config, ... }:
    {
      services.avahi = {
        enable = true;
        openFirewall = true;
        publish = {
          enable = true;
          addresses = true;
        };
        extraServiceFiles.losos = ''
          <?xml version="1.0" standalone='no'?>
          <!DOCTYPE service-group SYSTEM "avahi-service.dtd">
          <service-group>
            <name replace-wildcards="yes">%h</name>
            <service>
              <type>_losos._tcp</type>
              <port>80</port>
              <txt-record>version=v0.4.0</txt-record>
            </service>
          </service-group>
        '';
      };
      services.nginx = {
        enable = true;
        virtualHosts.front = {
          default = true;
          locations."/".return = "200 'admin pages'";
        };
      };
      networking.firewall.allowedTCPPorts = [ 80 ];
    };

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
        ../modules/quickemu.nix
        ../modules/machines.nix
        ../modules/pm.nix
        # The LosOS boxes machine is the only one that talks to boxes: the
        # host has neither losos nor losos-boxes' service of its own.
        ../modules/losos-boxes.nix
      ];
      nixpkgs.overlays = [ (import ../pkgs) ];

      # network.nix's packet filter, which the guests' DHCP has to get past.
      networking.nftables.enable = true;
      # The machines' dnsmasq forwards their questions to the host's
      # resolver, which asks the stand-in internet.
      networking.nameservers = [ nodes.internet.networking.primaryIPAddress ];

      # And its resolver: networkd and resolved, mDNS on the LAN's link and
      # in resolved, 5353 open. The machines' dnsmasq passes their queries
      # to resolved's stub, as on the image.
      networking.useNetworkd = true;
      networking.useDHCP = false;
      systemd.network.networks."40-eth1".networkConfig.MulticastDNS = true;
      services.resolved = {
        enable = true;
        settings.Resolve.MulticastDNS = true;
      };
      networking.firewall.allowedUDPPorts = [ 5353 ];

      boot.blacklistedKernelModules = [
        "kvm_intel"
        "kvm_amd"
      ];

      virtualisation = {
        cores = 4;
        # The two machines it runs at once, and itself.
        memorySize = 6144;
        # The template is copied into the machines' folder and each
        # machine's disk grows on top of it.
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

      # The guest agent's protocol, as derisk speaks it on the socket in the
      # machine's folder: a sync whose answer is read past anything an
      # earlier connection left, then the command. Prints what it returned.
      environment.etc."qga.py".text = ''
        import json, random, socket, sys
        path, command = sys.argv[1], sys.argv[2]
        agent = socket.socket(socket.AF_UNIX)
        agent.settimeout(10)
        agent.connect(path)
        sync = random.randrange(1, 2**31)
        agent.sendall(b"\xff" + json.dumps({"execute": "guest-sync-delimited", "arguments": {"id": sync}}).encode() + b"\n")
        replies = agent.makefile("rb")
        while True:
            line = replies.readline()
            if not line:
                sys.exit(1)
            try:
                if json.loads(line.split(b"\xff")[-1]).get("return") == sync:
                    break
            except ValueError:
                pass
        agent.sendall(command.encode() + b"\n")
        reply = json.loads(replies.readline())
        if "error" in reply:
            sys.exit(f"{command}: {reply['error']}")
        print(json.dumps(reply["return"]))
      '';

      environment.systemPackages = [ pkgs.python3 ];
    };

  testScript = ''
    import base64
    import json
    import time

    machines = "/root/.local/share/pm/machines"
    guest_run = "/run/current-system/sw/bin/losos-guest-run"

    def agent(command, name="work"):
        reply = machine.succeed(
            f"python3 /etc/qga.py {machines}/{name}/{name}-agent.sock '{json.dumps(command)}'"
        )
        return json.loads(reply)

    def in_guest(path, *args, name="work"):
        """Runs a program in the machine through its agent, as the desktop does."""
        pid = agent({
            "execute": "guest-exec",
            "arguments": {"path": path, "arg": list(args), "capture-output": True},
        }, name)["pid"]
        for _ in range(120):
            status = agent({"execute": "guest-exec-status", "arguments": {"pid": pid}}, name)
            if status["exited"]:
                out = base64.b64decode(status.get("out-data", "")).decode()
                err = base64.b64decode(status.get("err-data", "")).decode()
                assert status["exitcode"] == 0, f"{path} {args} failed: {err}"
                return out
            time.sleep(1)
        raise Exception(f"{path} {args} did not finish")

    def record(name):
        return json.loads(machine.succeed(f"cat {machines}/{name}/machine.json"))

    def address(name):
        """The address the machines' DHCP gave the machine, by its MAC."""
        mac = machine.succeed(
            f"sed -n \"s/^macaddr='\\\\(.*\\\\)'$/\\\\1/p\" {machines}/{name}.conf"
        ).strip()
        assert mac, f"{name}.conf has no macaddr"
        leases = "/var/lib/private/losos-machines-dns/leases"
        machine.wait_until_succeeds(f"grep -qi ' {mac} 192.168.122.' {leases}", timeout=300)
        return machine.succeed(f"grep -i ' {mac} ' {leases} | cut -d' ' -f3").strip()

    def started(name):
        machine.succeed(f"pm machine start {name}")
        machine.succeed(f"systemctl is-active pm-machine-{name}.service")

    start_all()
    machine.wait_for_unit("losos-machines-dns.service")

    with subtest("the host has the machines' network, vsock and quickemu"):
        machine.succeed("ip -4 addr show virbr0 | grep -q 192.168.122.1/24")
        machine.succeed("test -u /run/wrappers/bin/qemu-bridge-helper")
        machine.succeed("grep -qx 'allow virbr0' /etc/qemu/bridge.conf")
        machine.succeed("command -v quickemu remote-viewer waypipe")
        machine.succeed("test -c /dev/vhost-vsock")
        machine.succeed("test -f /run/current-system/sw/share/losos/machines/losos-guest.qcow2")
        machine.fail("test -e /dev/kvm")

    with subtest("pm makes a machine from the shipped template"):
        machine.succeed(
            "mkdir -p /root/.config/pm/plugins",
            "cp /run/current-system/sw/share/pm/plugins/quickemu.wasm /root/.config/pm/plugins/",
            "pm sign /root/.config/pm/plugins/quickemu.wasm",
        )
        machine.succeed(
            "pm machine create work"
            " --image /run/current-system/sw/share/losos/machines/losos-guest.qcow2"
            " --label blue --memory 1536 --cpus 2 --disk 8"
        )
        work = record("work")
        assert work["label"] == "blue", work
        assert work["template"]["name"] == "losos-guest", work
        # The template copied in once, read-only, which every machine made
        # from it shares, and the machine's disk over it.
        machine.succeed(f"ls {machines}/templates | grep -q '^losos-guest-'")
        machine.succeed(f"qemu-img info {machines}/work/disk.qcow2 | grep -q 'backing file: {machines}/templates/losos-guest-'")
        machine.succeed(f"grep -qx \"network='virbr0'\" {machines}/work.conf")

    with subtest("the machine boots, grows its disk and takes an address"):
        started("work")
        machine.wait_until_succeeds(
            f"python3 /etc/qga.py {machines}/work/work-agent.sock '{{\"execute\":\"guest-ping\"}}'",
            timeout=900,
        )
        size = int(in_guest("/run/current-system/sw/bin/df", "--output=size", "-B1G", "/").split()[-1])
        assert size >= 7, f"the root is {size} GiB, so it did not grow into the 8 GiB disk"
        address("work")

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

    with subtest("the machine looks .local names up through the host"):
        lanbox.wait_for_unit("avahi-daemon.service")
        lan_address = lanbox.succeed("ip -4 -o addr show eth1 | awk '{print $4}' | cut -d/ -f1").strip()
        machine.wait_until_succeeds(f"resolvectl query lanbox.local | grep -q {lan_address}", timeout=120)
        status = in_guest("/run/current-system/sw/bin/resolvectl", "status")
        print(status)
        assert "-mDNS" in status and "+mDNS" not in status, "the machine multicasts itself"
        for _ in range(30):
            try:
                found = in_guest("/run/current-system/sw/bin/getent", "hosts", "lanbox.local")
                break
            except Exception:
                time.sleep(2)
        else:
            raise Exception("the machine cannot look up lanbox.local")
        assert found.split()[0] == lan_address, f"lanbox.local is {found}, not {lan_address}"
        # The address the host gave is one the machine reaches through NAT.
        in_guest("/run/current-system/sw/bin/ping", "-c", "1", "-W", "5", "lanbox.local")

    with subtest("the host finds a machine by the name it gave DHCP"):
        hostname = in_guest("/run/current-system/sw/bin/hostname").strip()
        machine.wait_until_succeeds(f"resolvectl query {hostname}.machines | grep -q 192.168.122.", timeout=120)

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
        cid = str(record("work")["cid"])
        machine.succeed(f"grep -q 'guest-cid={cid}' /proc/$(cat {machines}/work/work.pid)/cmdline")
        machine.succeed("systemd-run --unit=vsock-listen python3 /etc/vsock-listen.py /tmp/peer")
        machine.wait_until_succeeds("ss -l --vsock | grep -q 7350")
        in_guest(guest_run, "org.derisk.terminal")
        machine.wait_until_succeeds("test -s /tmp/peer", timeout=120)
        peer = machine.succeed("cat /tmp/peer").strip()
        assert peer == cid, f"the connection came from {peer}, the machine is {cid}"

    with subtest("a LosOS boxes machine finds the box through the host and offers it"):
        machine.fail("command -v losos")
        machine.wait_for_unit("losos-boxes-relay.service")
        machine.succeed("systemctl cat losos-machines-dns.service | grep -q -- --hostsdir=/run/losos-boxes/hosts")
        machine.succeed(
            "pm machine create boxes"
            " --image /run/current-system/sw/share/losos/machines/losos-boxes.qcow2"
            " --label green --memory 1536 --cpus 2 --disk 8",
        )
        started("boxes")
        machine.wait_until_succeeds(
            f"python3 /etc/qga.py {machines}/boxes/boxes-agent.sock '{{\"execute\":\"guest-ping\"}}'",
            timeout=900,
        )
        boxes_address = address("boxes")
        for _ in range(90):
            apps = in_guest(guest_run, "--list", name="boxes")
            if "losos-box-lanbox.local\tlanbox" in apps:
                break
            time.sleep(2)
        else:
            print(machine.succeed("journalctl -u losos-boxes-relay --no-pager"))
            raise Exception(f"the LosOS boxes machine offers no lanbox: {apps}")
        print(apps)
        entry = in_guest(
            "/run/current-system/sw/bin/cat",
            "/home/user/.local/share/applications/losos-box-lanbox.local.desktop",
            name="boxes",
        )
        print(entry)
        assert "\nExec=derisk-terminal --app-id=org.losos.Boxes --title=lanbox losos-boxes terminal http://lanbox.local\n" in entry
        assert "\nComment=LosOS v0.4.0 box at lanbox.local\n" in entry
        # The machine's own resolved does no mDNS: what it found came
        # through the host's.
        status = in_guest("/run/current-system/sw/bin/resolvectl", "status", name="boxes")
        assert "+mDNS" not in status, status

    with subtest("the other machines reach the box through the LosOS boxes machine"):
        # Its hosts file is under its owner's uid and its name: root's here.
        machine.wait_until_succeeds(
            f"grep -qx '{boxes_address} lanbox.local' /run/losos-boxes/hosts/0-boxes", timeout=120
        )
        for _ in range(30):
            found = in_guest("/run/current-system/sw/bin/getent", "hosts", "lanbox.local")
            if found.split()[0] == boxes_address:
                break
            time.sleep(2)
        else:
            raise Exception(f"lanbox.local is {found} for the machine, not {boxes_address}")
        request = (
            "exec 3<>/dev/tcp/{host}/80;"
            " printf 'GET / HTTP/1.1\\r\\nHost: {name}\\r\\nConnection: close\\r\\n\\r\\n' >&3;"
            " cat <&3"
        )
        answer = in_guest(
            "/run/current-system/sw/bin/bash", "-c",
            request.format(host="lanbox.local", name="lanbox.local"),
        )
        assert answer.rstrip().endswith("admin pages"), answer
        # The box saw the connection come from the host's LAN address, which
        # is where the LosOS boxes machine's traffic leaves the host's NAT.
        lanbox.succeed("grep -q 'GET / ' /var/log/nginx/access.log")
        # A name that is not a box gets nothing passed on.
        refused = in_guest(
            "/run/current-system/sw/bin/bash", "-c",
            request.format(host=boxes_address, name="printer.local") + " || true",
        )
        assert "admin pages" not in refused, refused

    with subtest("a stopped LosOS boxes machine routes nothing"):
        machine.succeed("pm machine remove boxes")
        machine.fail("systemctl is-active pm-machine-boxes.service")
        machine.wait_until_fails("test -e /run/losos-boxes/hosts/0-boxes", timeout=60)

    with subtest("pm removes it and keeps the template"):
        machine.succeed("pm machine remove work")
        machine.fail(f"test -e {machines}/work")
        machine.fail(f"test -e {machines}/work.conf")
        machine.succeed(f"ls {machines}/templates | grep -q '^losos-guest-'")

    with subtest("the image carries the CDs of guest tools where pm looks"):
        tools = "/run/current-system/sw/share/pm/machines"
        machine.succeed(f"test -s {tools}/windows-tools.iso")
        machine.succeed(f"test -s {tools}/linux-tools.iso")
        # pm finds them through the session's XDG_DATA_DIRS, so a login
        # shell's environment, not the test driver's.
        machine.succeed("bash -lc 'echo \"$XDG_DATA_DIRS\" | grep -q /run/current-system/sw/share'")

    with subtest("a Windows machine gets the drivers CD, a TPM and a screen, and starts"):
        # Windows itself cannot be installed here, so its CD is a stand-in:
        # what is checked is that quickemu takes the machine pm makes for it
        # and starts it, swtpm and all, with the host's drivers CD beside it.
        machine.succeed("head -c 1048576 /dev/zero > /tmp/win11.iso")
        machine.succeed(
            "bash -lc 'pm machine create win --installer /tmp/win11.iso"
            " --system windows --label red --memory 512 --disk 4'"
        )
        conf = machine.succeed(f"cat {machines}/win.conf")
        for expected in [
            "guest_os='windows'",
            "tpm='on'",
            "display='spice'",
            "iso='" + machines + "/cds/win11-",
            "fixed_iso='" + machines + "/cds/windows-tools-",
        ]:
            assert expected in conf, f"{expected} is not in {conf}"
        assert "vhost-vsock" not in conf, conf
        assert record("win")["screen"], record("win")
        started("win")
        machine.wait_until_succeeds("pgrep -x swtpm", timeout=60)
        # Its screen, on quickemu's SPICE socket, where derisk's
        # remote-viewer finds it.
        machine.wait_until_succeeds(f"test -S {machines}/win/win.sock", timeout=60)

    with subtest("removing it keeps the CDs another machine may share"):
        machine.succeed("pm machine remove win")
        machine.fail("systemctl is-active pm-machine-win.service")
        machine.fail(f"test -e {machines}/win")
        machine.succeed(f"ls {machines}/cds | grep -q '^windows-tools-'")
        machine.succeed(f"ls {machines}/cds | grep -q '^win11-'")
  '';
}
