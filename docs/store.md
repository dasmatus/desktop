# The Store

Apps the image does not carry come from [Flathub](https://flathub.org), and
the Store is where a person finds and installs them. It is one of derisk's
own apps (`org.derisk.store`, `crates/derisk-store` in derisk), drawn by the
desktop like Settings and Files, and the image ships no other: it replaced
Bazaar, whose GTK 4 window took neither scrolling nor clicks under derisk.

- **Categories from what the apps say.** Every app on Flathub ships
  AppStream data, and the Store sorts by all of it. An app goes in each main
  category it names (`Game`, `Office`), then in each one its additional
  categories belong under in the freedesktop menu specification
  (`ArcadeGame` under Games, `WebBrowser` under Internet), so an app that
  names only `BoardGame` is still a game. An app that names no category at
  all goes where its keywords, name and summary point. Utilities, the
  catch-all, comes last whenever an app names something narrower. A
  category's page is split into sections by the additional categories its
  apps name (Arcade, Strategy, Emulators, ...).
- **Release notes on the front page.** Apps list their releases, with notes,
  in their AppStream data. The front page leads with the updates waiting,
  then what changed in the apps you have installed, then the categories,
  then the newest release notes across Flathub, each linking to its app.
- **Search** looks at names first, then keywords, then summaries and IDs;
  every word typed has to be found.
- **Your language.** Names, summaries, descriptions and release notes are
  shown in the session's language where Flathub has them translated.
- **Install, open, update and remove.** An app's page installs it, and once
  it is installed opens, updates or removes it. The Refresh button fetches
  Flathub's catalogue again and looks for updates.

## Where it installs

When libvirt has a machine called `flatpak`, the machine a PC's Flatpak
apps run in ([hypervisor.md](hypervisor.md)), the Store installs into it: it runs
`flatpak` there through the machine's guest agent, as root, so apps go into
the machine's system installation, and reads Flathub's catalogue and icons
from the machine's disk. It starts the machine when it is off. An app
installed there opens on the desktop like any other machine's app.

A phone has no machines, and keeps Flatpak on itself; so does a computer
without that machine. There the Store runs the system's
`flatpak`. flatpak's polkit rule lets an administrator install for everyone
without a password; for anyone else the Store installs for that person alone
(`--user`), from the app's flatpakref, which needs no remote set up.

## How it reads Flathub

flatpak keeps each remote's catalogue at
`/var/lib/flatpak/appstream/flathub/<arch>/active/appstream.xml.gz`, with the
icons it names in `icons/64x64/` beside it, and refreshes it with
`flatpak update --appstream`, which any user may run. Flathub's catalogue is
some 50 MB of XML once inflated, every string in dozens of languages, so the
Store reads it as a stream and keeps only what it shows: the whole of
Flathub, over three thousand apps, reads in about half a second. A category
or search page lays out only the rows on screen. A system that has never
fetched the catalogue (a first boot without the network) fetches it the
first time the Store opens.
