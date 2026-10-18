# When something goes wrong

derisk's apps, the installer and first-boot setup show a failure in an
alert dialog: what failed as its title, red, the causes under it, what to do
about it when the app knows, and its code. A warning, something that went
only partly wrong, has the accent color instead. "Copy details" copies the
whole report as plain text, for a bug report; "Learn more" opens the section
of this page the error names; "OK" or Escape closes it.

![An error dialog: "Could not apply the theme" in red, the cause "no theme named pastel", what to do about it, the code mcsapi_theme::not_found, and the Copy details, Learn more and OK buttons](images/error-dialog.png)

Those sections are read from the copy of this documentation the image
carries, `/run/current-system/sw/share/doc/losos` (`nixos/modules/docs.nix`),
so they open without a network, and from the published site when the copy
has no such page. The installer and first-boot setup run before there is a
session or a browser, so their dialogs print the section's address on the
published site instead, for reading on another device. The dialogs are mcsapi's `ErrorDialog` drawing an
`mcsapi_ui::Error`, a miette report that knows its section; the theme colors
them like every other component.

## Files could not open or change something

Files reports what the filesystem refused: a folder that is gone, a name
that is taken, a file you are not allowed to read or change. The cause under
the message is the system's own reason, such as "Permission denied" or "Not
a directory". Files on a disk you did not mount yourself, and anything
outside your home directory, usually belong to root. Files opens a file by
handing it to `xdg-open`, so a file it could not open at all means
`xdg-open` itself did not start; a file that opens in the wrong application,
or in none, is `xdg-open`'s choice.

![Files showing "Could not open folder" with its cause, "Not a directory (os error 20)", and Learn more, Copy details and Dismiss](images/error-files.png)

## The editor could not open or save a file

The Text Editor saves to the path in its location field, which can differ
from the file it opened: that is how it saves a copy. An empty field has
nowhere to save to. A file it could not open is usually unreadable to you or
not text; a path that does not exist yet opens as a new file instead, and is
created on the first save.

## Settings could not be read or saved

Settings keeps everything in `$XDG_CONFIG_HOME/derisk/settings.conf`, or
`~/.config/derisk/settings.conf` without the variable, and writes it whole
each time you press Save, through a temporary file beside it. "No config
directory" means neither variable is set, so the changes last only until
Settings closes. A file it could not read or write is the filesystem
refusing, as in Files above; a home directory that is full or read-only
does it too.

## Settings skipped some lines

A line Settings did not understand, such as `desktop.gaps = lots`, is left
out and that setting keeps its default; the warning names the first one.

> [!WARNING]
> The next Save writes the file again from what Settings shows, without the
> skipped lines.

![Settings warning in its accent color: "Skipped 1 invalid line(s): line 1: ignored desktop.gaps = lots", and that those settings keep their defaults until the lines are fixed](images/error-settings.png)

## The default browser did not change

The browser you pick under Default apps is written to
`$XDG_CONFIG_HOME/mimeapps.list` (`~/.config/mimeapps.list`), the file every
desktop reads, for web links and HTML files. Settings saved its own file but
could not write that one, so links still open in the old browser. The cause
says why; it is the filesystem again. See [the choice screens](choice-screens.md)
for where the list of browsers comes from.

## The installer stopped

The message is the installer's backend's own, from the step that failed,
and "Show details" is its log. A disk it refused was not in its list or is
the medium the installer booted from. A download that does not verify, in a
network that re-signs TLS, is what a release disk is for. [The
installer](installer.md) describes each step.

> [!WARNING]
> Nothing is written before "Erase and install"; after it, a failed install
> leaves the disk without a working system, and the install can be run again
> from the start. tty2 has a root shell for looking around first.

![The installer's "The install failed" page: partitioning and mounting done, "Downloading and writing the system" failed with systemd-sysupdate's exit status, and Show details open on a log ending in "SSL peer certificate or SSH remote key was not OK"](images/installer-failed.png)

## Setup could not finish

First-boot setup applies its pages one by one with `localectl`,
`timedatectl` and `homectl`, and the message is the tool's. The step that
fails is usually the account: homed refuses a user name that is taken or not
a valid name, and a password its quality check rejects. Back returns to the
account page. Setup runs again on every boot
until an account exists ([First-boot setup](first-boot.md)).

## No one can administer the machine

A running system is administered only through `pm system`
([pm](pm.md#administering-the-machine)), which manages units and updates and
nothing else, and needs an administrator who can sign in, pm's system
service and polkit. When something needs more than that, or one of them is
missing, the way in is a root shell for one boot, chosen at GRUB's menu by
someone at the machine. Nothing in a running system gives one:

1. When the boot menu shows, press `c` for GRUB's prompt.
2. Type `set losos_extra=systemd.debug_shell`, Enter, then
   `export losos_extra`, Enter.
3. Press Escape to go back to the menu, and Enter to start the newest
   version.
4. Once the login screen shows, press Ctrl+Alt+F9. tty9 is a root shell,
   with no password.

From there, for instance, `homectl create <name> --member-of=wheel` makes
a new administrator, and `journalctl -u pm-system -b` says why pm's service
did not start. The arguments last one boot, and the shell stays open while
the system runs, so `systemctl reboot` when done. The shell is systemd's
debug shell, and `losos_extra` is any kernel arguments GRUB adds to the
command line the release was built with ([Boot loader and boot screen](boot-loaders.md#grubs-menu)).
On UEFI, that makes GRUB start the kernel beside the UKI, as on a BIOS,
instead of the UKI itself, which would take arguments only in place of its
whole command line.

> [!NOTE]
> Only an x86_64 PC has that kernel, so an aarch64 PC has no recovery shell
> yet ([What is not done](not-done.md)).

The shell does not open anyone's home: each is its own encrypted image,
which only its owner's password unlocks ([Where the OS lives](layout.md)). The rest of
the disk, the system and its state, is not encrypted, so the shell gives
someone at the machine nothing another operating system started from a
stick would not.
