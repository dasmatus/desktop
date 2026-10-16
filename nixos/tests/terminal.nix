# derisk-terminal (modules/desktop.nix), the image's terminal, in a VM with
# no GPU: Mesa offers only llvmpipe there, so on its own it must pick the
# software renderer, and asked for the GPU it must still draw through GL.
# Each time it runs a script printing text, Nerd Font icons, a box and
# colours, and the test reads the text back off the screen and keeps a
# screenshot of both renderers in its output.
#
# cage stands in for derisk's compositor, which needs a seat and a login to
# start; to a Wayland client both are a compositor with wl_shm and EGL.
# Only the terminal and its fonts, not the whole image, so it boots quickly
# and runs without KVM too.
{ lib, ... }:

{
  name = "losos-desktop-terminal";

  # derisk-terminal comes from the OS's overlay: the derivation the image
  # builds.
  node.pkgsReadOnly = false;
  enableOCR = true;

  nodes.machine =
    { pkgs, ... }:
    let
      demo = pkgs.writeShellScript "terminal-demo" ''
        printf '\e[1mderisk-terminal\e[0m renderer test\n\n'
        printf 'Icons:  folder   rust   git   branch\n'
        printf 'Prompt: \e[44;37m main \e[34;42m\e[30m ~/src \e[0;32m\e[0m\n'
        printf '┌──────┬──────┐\n│ left │ right│\n└──────┴──────┘\n'
        for i in 0 1 2 3 4 5 6 7; do printf "\e[4''${i}m   \e[0m"; done; echo
        printf '\e[1mbold\e[0m \e[3mitalic\e[0m \e[4munderline\e[0m\n'
        exec sleep infinity
      '';
      # The renderer the next start asks for, written by the test script;
      # `auto` until it writes one.
      start = pkgs.writeShellScript "terminal-start" ''
        renderer=$(cat /run/terminal-renderer 2>/dev/null || echo auto)
        exec ${lib.getExe pkgs.derisk.terminal} --renderer "$renderer" -e ${demo}
      '';
    in
    {
      nixpkgs.overlays = [ (import ../pkgs) ];

      users.users.alice = {
        isNormalUser = true;
        uid = 1000;
      };
      services.cage = {
        enable = true;
        user = "alice";
        program = "${start}";
        # QEMU's VGA has no 3D, so the compositor draws with pixman, as it
        # would on any machine without a GPU.
        environment.WLR_RENDERER = "pixman";
      };
      hardware.graphics.enable = true;
      fonts.packages = [ pkgs.nerd-fonts.hack ];
      virtualisation.memorySize = 2048;
    };

  testScript = ''
    def started(renderer, line):
        machine.succeed(f"echo {renderer} > /run/terminal-renderer")
        machine.succeed("systemctl restart cage-tty1.service")
        machine.wait_until_succeeds(
            f"journalctl -b -u cage-tty1.service --grep '{line}'", timeout=300
        )
        machine.wait_for_text("renderer test", timeout=300)

    machine.wait_for_unit("cage-tty1.service")

    with subtest("with no GPU it draws in software"):
        started("auto", "drawing in software")
        machine.screenshot("terminal-software")

    with subtest("asked for the GPU it draws through GL, on llvmpipe here"):
        started("gpu", "drawing on the GPU with llvmpipe")
        machine.screenshot("terminal-gpu")
  '';
}
