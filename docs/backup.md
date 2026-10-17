# Backups to a LosOS box

A [LosOS](https://losos.dasmat.us) box, the project's home server, runs
LosOS cloud (Nextcloud, at `/nextcloud`) and LosOS Git (Forgejo, at
`/forgejo/`). LosOS Desktop backs each person's home up to one on the local
network, once a day:

- every Git repository in the home goes to LosOS Git, as a private
  repository of the person's own;
- the configs, everything in `~/.config` and the dotfiles in the home
  itself, go to LosOS Git too, as the history of one private repository,
  `<this computer's name>-config`;
- everything else goes to their LosOS cloud, under
  `Backups/<this computer's name>/`, only what changed since the last time.

`losos-backup` (`src/losos-backup`, `nixos/modules/backup.nix`) does it,
from a timer in each person's session, and derisk Settings' **Backups**
page sets it up.

## Setting it up

The Backups page, under System in Settings:

1. **Box.** It lists the boxes it can reach. Picking one (or typing its
   name, `mattbox.local`) fetches the box's certificate from
   `/setup/losos-ca.crt` and shows its SHA-256 fingerprint, in the same
   spelling as the box's own setup wizard, to compare. From then on every
   connection to the box has to present exactly that certificate; the box's
   certificate is self-signed for its `.local` name, so there is no
   authority to ask instead.
2. **LosOS cloud** and **LosOS Git.** Each takes the account's name and
   password once and trades it for a credential of this computer's own: an
   app password, which LosOS cloud lists under the account's security
   settings, and an access token named "LosOS Desktop backups (*computer*)"
   with the `write:repository` and `write:user` scopes, which LosOS Git
   lists under the account's applications. The password is not kept. The
   two apps keep separate accounts on a box, so signing in to one is
   enough to back up to it: without LosOS Git, repositories and configs go
   to LosOS cloud as files with the rest.
3. **Back up every day** turns the timer's backups on. **Back up now**
   runs one at once.

The page shows when the last backup went through, what it sent, and any
file or repository the box refused, with the box's reason.

The same steps on the command line:

```sh
losos-backup choose mattbox.local
printf '%s' "$password" | losos-backup sign-in cloud matus
printf '%s' "$password" | losos-backup sign-in git matus
losos-backup on
losos-backup run          # now
losos-backup status       # what the page shows, as JSON
```

## What a backup does

`losos-backup.timer` asks every hour; the run goes ahead only when the
backups are on and the last one that went through is a day old. So a
computer that was off, asleep or away from the box catches up within the
hour after it is back, and a box that cannot be reached is a skipped run,
tried again the next hour, not a failure. The runs are the person's own,
in their user manager: systemd-homed unlocks a home only while its owner is
signed in, and the backup runs with the lowest CPU and disk priority.

It walks the home without following symbolic links:

- A directory with a `.git` directory, or a bare repository, is a
  repository. Its branches and tags are pushed to a repository named after
  its directory (or its whole path, when two share a name), forced, and a
  branch deleted at home is deleted on the box, so the box's copy is a
  mirror. Uncommitted work goes too: the work tree as it is, untracked
  files included and ignored ones not, as one commit on top of `HEAD` on
  the branch `losos-backup/uncommitted`. It is made with an index of its
  own, so the repository's index, branches and stash are left as they
  were, and the repository's hooks do not run. A worktree's checkout (a
  `.git` file) is skipped, its commits being in the repository it belongs
  to.
- The configs, every file under `~/.config` and every file directly in
  the home whose name starts with a dot (`.bashrc`, `.gitconfig`), are
  committed together to `main` of the private repository
  `<computer>-config`, one commit each run that finds them changed, so
  the repository is their history. A run starts from `main` as the box
  has it, so the history carries on when this computer's own copy
  (`~/.local/state/losos-backup/config.git`) is lost, and an edit made on
  the box becomes a parent rather than a refused push. The commit holds
  exactly those files, read as they are: no ignore file leaves one out.
  A repository inside `~/.config`, such as an editor's config kept in
  Git, goes as a repository of its own instead. Dot directories in the
  home, `~/.ssh` or `~/.local`, are not configs and go to LosOS cloud;
  so do the configs until they are in LosOS Git, so a push that fails
  leaves them with the rest rather than nowhere.
- Every other file goes to LosOS cloud at its path under the home, with its
  modification time, when its size or time differs from what the last
  backup sent. A file larger than 128 MiB goes up in 64 MiB chunks
  (Nextcloud's chunked upload), since the box's web server takes 512 MiB at
  most in one request. A file or folder deleted from the home is deleted
  there too and goes to LosOS cloud's trash, which, with the old versions
  LosOS cloud keeps of every changed file, is where an older copy comes
  back from.
- Left out: `~/.cache`, the trash, Flatpak's, containers' and machines'
  downloads and disks, cargo's and npm's caches, any `node_modules`, any
  directory with a `CACHEDIR.TAG` (cargo's `target`, ccache and most other
  caches write one), systemd-homed's `~/.identity`, and the backups' own
  settings and state. `~/.config/losos-backup/exclude` adds paths, one per
  line, relative to the home.

A file the box refuses, such as a name Nextcloud forbids (`.htaccess`), is
listed in the report and does not stop the rest. The run stops when the box
stops answering, keeping what it had sent: the next one carries on from
there.

## How it reaches the box

On a PC the one machine that talks to LosOS boxes is the LosOS boxes
machine, and every other machine reaches a box through it: its `losos-boxes
route` passes each connection on ports 80 and 443 to the box its `Host`
header or TLS server name asks for, as it is, so the box's own certificate
answers. The host's `losos-boxes relay` writes the boxes each such machine
sees into `/run/losos-boxes/hosts/<machine>`, one `<machine's address>
<box>.local` line each. The backups read those files the way libvirt's DNS
does for the [machines](hypervisor.md), and connect to that address with
the box's name; git gets the same pin through `http.curloptResolve`.

That directory exists on every PC with machines and is empty while no
LosOS boxes machine runs. Then a run starts one the person has (`pm machine
start`), waits up to five minutes for it to see the box, and stops it again
afterwards; it never goes round the machine to the box. Starting a machine
on `qemu:///system` is an administrator's right, so for anyone else the
machine has to be running already. A system without the directory, a phone,
has no machines and reaches the box by its name, through resolved's mDNS.

## Where things are kept

| Path | What |
| --- | --- |
| `~/.config/losos-backup/config.json` | The box, whether backups are on, the accounts |
| `~/.config/losos-backup/secrets.json` | The app password and the token, readable by the person alone |
| `~/.config/losos-backup/box.pem` | The box's pinned certificate, which git reads as its only CA |
| `~/.local/state/losos-backup/files.json` | What LosOS cloud has, with each file's size and time |
| `~/.local/state/losos-backup/repos.json` | The name each repository got in LosOS Git |
| `~/.local/state/losos-backup/config.git` | This computer's copy of the configs' repository |
| `~/.local/state/losos-backup/last.json` | The last run's report |

The two secrets are in a file rather than the Secret Service, unlike
`losos`'s keys: the timer runs with nobody at the screen, and a locked
keyring would ask for its password in a window. The home they are in is
encrypted by systemd-homed whenever its owner is signed out. Signing out of
LosOS cloud on the Backups page revokes its app password on the box; a
LosOS Git token can only be deleted with the password, in the account's
settings there.

## Tested

`losos-backup`'s own tests cover the walk, the names, the paths, the
relay's hosts files and a snapshot of uncommitted work. `nixos/tests/backup.nix`
boots a box with Nextcloud and Forgejo laid out as a LosOS box lays them
out, behind a certificate made the way the box makes its own, and a desktop
that chooses it, signs in to both, backs up a home with a large file, a
cache, configs and a repository with uncommitted work, then backs up again
after changes and checks what arrived and what went, including the
configs' second commit on top of their first; then the same through a
hosts file as the relay writes it, and a skipped run once no machine names
the box.
