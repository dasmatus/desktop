# First-boot setup

A fresh install has no user. `derisk setup` (`nixos/modules/setup.nix`) runs
on tty1 as a logind session of its own, after homed and before the display
manager, and draws derisk's pages on the seat: language, keyboard layout,
time zone, network, then the first account (full name, user name, password).
It saves them with `localectl set-locale`, `localectl set-x11-keymap`,
`timedatectl set-timezone` and `homectl create --member-of=wheel`, the
password passed in `NEWPASSWORD` and never on a command line, and exits.
Everyone after the first account is added in Settings, Users
([More than one person](users.md)).
The keyboard layout switches live as it is picked, and the display manager
hands localed's saved layout to the login screen and the session as
xkbcommon's defaults.

homed checks the password's quality and refuses a weak one, such as a short
password or a dictionary word. The account page asks the same check while
the password is typed: it loads libpwquality as systemd does, with the same
configuration, says why a password would be refused under the field, and
keeps "Set up" off until it would not be. Should homed refuse it anyway,
homectl asks for another, and with no terminal to ask on it asks a password
agent with no time limit, which nothing on the seat answers. Setup watches
`/run/systemd/ask-password` for a question from the command it runs and
answers it with cancel, so homectl exits with homed's reason, which the
failure dialog shows before Back returns to the account page
([Setup could not finish](troubleshooting.md#setup-could-not-finish)).

![First-boot setup's Time zone, Network and Your account pages](images/first-boot-pages.png)

It runs on every boot and exits at once when `userdbctl` lists a regular
user, so a machine switched off halfway through asks again. If derisk cannot
draw at all, homed's console wizard asks for the user instead. homed's own
first-boot unit is kept for `home.create.*` credentials, without its prompt.
The languages offered are the locales built into the archive, the thirty-odd
derisk has names for, and `i18n.imperativeLocale` lets localed's choices
stick. On a phone-sized screen the pages fill the screen and the on-screen
keyboard opens under a focused field; on a PC the card floats and the
keyboard is a button in the corner.

## The first login

The person who just chose the password is not asked for it again a minute
later. Setup leaves a ticket, the user name and password in
`/run/derisk-first-login` (root's, mode 0600, on a tmpfs so it never reaches
a disk), and the display manager takes it before it starts the login
screen: it deletes the file first, then logs that user in through the same
`derisk-login` PAM stack, so homed unlocks the home area as usual. A ticket
that is not root's, not private, a symlink or older than ten minutes is
deleted and ignored. Only that first login skips the login screen; every
later boot and every log out shows it. The session then opens on a short
tour of the desktop (the top bar, the window buttons, search, the overview,
and the keys for each), which leaves a mark in `~/.local/state/derisk` once
finished or skipped; "Take the Tour" in the command palette shows it again.
The boot VM test (`nixos/tests/boot.nix`) writes a ticket by hand and checks
the session starts without the login screen and the ticket is gone.
