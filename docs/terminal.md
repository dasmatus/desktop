# The terminal

The desktop's terminal is derisk's own, `derisk-terminal`
(`crates/derisk-terminal` in derisk), and the image ships no other: it
replaced foot, which the image carried only because derisk had none. The
virtual machine template's guest ships it too ([hypervisor.md](hypervisor.md)).

![derisk-terminal in a derisk session, showing Nerd Font icons, a powerline prompt, box drawing, the 256 colours and text styles](images/terminal.png)

- **Nerd Font icons.** It draws the theme's monospace font through that
  font's Nerd Font build when one is installed, and takes what a font lacks
  from Symbols Nerd Font, then from the usual monospace fonts. The image
  installs Hack Nerd Font, Hack being the theme's default, and the
  symbols-only Nerd Font, which gives the icons to a theme naming a font
  with no Nerd Font build (`nixos/modules/desktop.nix`). An icon followed by
  a blank cell may take both, as Nerd Font icons are drawn to. Box drawing
  and block characters are drawn to the cell, not taken from the font, so
  lines meet across cells in any font.
- **On the GPU, or in software.** It draws each frame on the GPU through
  OpenGL ES 3 (or OpenGL 3.3) in one draw call, from an atlas of the glyphs
  it has drawn before. Where there is no GPU, or OpenGL is itself drawn in
  software (llvmpipe), it draws the same frame on the CPU into shared memory
  instead, which is faster than llvmpipe. `--renderer gpu` or
  `--renderer software` picks one by hand.
- **Deferred.** What the shell prints only changes the grid; a frame is
  built from the grid when the compositor asks for one, so a command that
  prints a lot draws once per frame, not once per line.
- **The desktop's theme.** Colours and the font follow derisk's theme, and a
  change in Settings reaches a running terminal the next time it is focused.

It takes foot's command line (a bare command or `-e`, `-a`/`--app-id`,
`-T`/`--title`, `-D`), so a desktop entry written for foot runs in it with
only the program's name changed.

`checks.x86_64-linux.terminal` (`nixos/tests/terminal.nix`) starts it under
cage in a VM with no GPU, where it has to choose software, and again with
`--renderer gpu`, where it has to draw through llvmpipe, and reads its text
off the screen both times. The image's own boot test checks it, Hack Nerd
Font and the symbols font are installed and foot is not.

## The tools in the shell

The commands a person types are the Rust versions wherever one takes the
GNU tool's options, on the host and in the guest template alike
(`nixos/modules/userland.nix`):

| Command | From |
|---|---|
| `ls`, `cp`, `cat` and the rest of coreutils | [uutils coreutils](https://github.com/uutils/coreutils) |
| `find`, `xargs`, `locate`, `updatedb` | [uutils findutils](https://github.com/uutils/findutils) |
| `su` | [sudo-rs](https://github.com/trifectatechfoundation/sudo-rs) |
| `sudo` | run0's sudo shim, in Rust, over `run0` ([systemd.md](systemd.md)) |
| `gpg`, `gpgv`, `gpgconf` | [Sequoia's GnuPG chameleon](https://gitlab.com/sequoia-pgp/sequoia-chameleon-gnupg), with `sq` beside it |

The chameleon reads and writes GnuPG's own `~/.gnupg`, so git's commit
signing and anything else that runs `gpg` keep working, and secret keys stay
with GnuPG's `gpg-agent`. Only `chcon` and `runcon` still come from GNU
coreutils, as uutils leaves out the two that only mean something under
SELinux.

Still GNU, because the Rust version does not yet take the same options:
`diff` (uutils' has no `-r`, `diff3` or `sdiff`), `sed` (uutils' ignores
`-z`, `\U` and `\b`, and gets `sed 'n;d'` wrong), `ps` and the rest of procps
(uutils' rejects `ps aux`), util-linux and tar (uutils has only work in
progress), and grep, awk, less and gzip, which have no Rust version that
takes their options. Services and NixOS's own scripts run
GNU throughout: their `PATH` is set by NixOS, not the profile, and they
were written against GNU. So does `systemd-pull`'s check of an update's
signature, which passes gpg `--import-options=merge-only`, an option the
chameleon refuses.

The image's boot test checks which of these answers `ls`, `find`, `su` and
`gpg` (`nixos/tests/boot.nix`).
