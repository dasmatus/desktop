# More than one person

One PC can have an account for everyone who uses it, and each person's
desktop keeps running while someone else uses the screen. Setup makes the
first account ([First-boot setup](first-boot.md)); everyone after that is
added in Settings, Users. Every account is a systemd-homed record with its
own encrypted home area, so one person's files stay locked while they are
logged out, whoever else is logged in.

## Settings, Users

The page lists everyone `userdbctl` knows, with their full name, user name
and role. Each change is a `homectl` command:

| Change | Command |
| --- | --- |
| Add someone | `homectl create <user> --real-name=… --member-of=…`, the password in `NEWPASSWORD` |
| Rename | `homectl update <user> --real-name=…` |
| Administrator or standard | `homectl update <user> --member-of=…`, with `wheel` added or taken out |
| Remove | `homectl remove <user>`, which deletes their home area and every file in it |

An administrator is a member of `wheel`, which is how run0, polkit and
Flatpak know who administers the machine; first-boot setup's account is
one. A standard user can use apps and change their own settings. homed asks
polkit before it makes, removes or changes anyone's account, and polkit asks
for an administrator's password through derisk's dialog
([polkit and run0](sign-in.md#polkit-and-run0)), so a standard user can open
the page and have an administrator type theirs. The page keeps one
administrator: the last one cannot be made standard or removed, and nobody
removes their own account from it.

Someone else's home area is locked while they are logged out, and homed
cannot write their record into it. Their change goes to homed's own copy of
the record (`homectl update --offline`), which is signed by the machine and
newer than the one in their home area, so homed writes it there at their
next login. A new account joins the same device groups as setup's
(`wpa_supplicant`, for Wi-Fi), and the display manager applies the password
age policy at its first login ([Password age](sign-in.md#password-age)).

The new password is checked as setup checks it: Settings asks libpwquality
while it is typed and keeps "Add" off until homed would take it. If homed
refuses it anyway, the page answers homectl's question for another one with
cancel, the same as setup, and shows homed's reason. The Sign-in page does
the same for a new password of one's own.

## The login screen

The login screen lists everyone by full name, with their initial, and marks
who is signed in. Picking someone who is not goes to their password. Picking
someone who is goes back to their desktop instead of logging them in a second
time: the login screen asks logind to make that session the seat's active
one (`loginctl activate`, which polkit lets the active session on a seat do),
and that desktop's own lock screen asks for the password. A typed user name
does the same. With one account and nobody signed in, the list is left out
and the screen goes straight to the password, as before.

## Switch user

Switch user locks the desktop and shows the login screen on another VT,
leaving the session running behind its lock screen. It is a button on the
lock screen, a row in the palette, `Super+X U`, and something the assistant
understands ("switch user"). The session connects to
`/run/derisk-dm/switch.sock`; connecting is the whole request. Anyone can
connect, but the display manager acts only for the owner of the session that
is on the screen, so a program in a session in the background cannot pull
the screen away from whoever is using it.

`derisk display-manager` runs every session on a VT of its own. A login
screen takes the first VT from 1 that nothing has open; the display manager
holds that VT open until the session has it, because logind starts a getty
on a VT that comes to the front with nobody on it. The session started from
that login screen keeps its VT. The login screen comes back when nobody is
logged in, when the session on the screen ends, and on Switch user; a
session ending on another VT changes nothing on the one in use.
