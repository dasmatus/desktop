@echo off
rem What a Windows machine takes from the LosOS Windows tools CD at the first
rem sign-in after Setup (autounattend.xml's FirstLogonCommands). Setup itself
rem only loaded the disk and network drivers; this installs the rest.

rem This CD's own drive, whatever letter Windows gave it.
set tools=%~d0

rem virtio-win's guest tools: every virtio driver (the display, the balloon,
rem the tablet, the random number generator) and the QEMU guest agent, which
rem is how the host's desktop sees the machine's address and shuts it down.
"%tools%\virtio-win-guest-tools.exe" /install /passive /norestart

rem WinApps' own Windows setup: Remote Desktop on, RemoteApp for every
rem program, and its firewall rule, so the host's desktop can open this
rem machine's apps as windows of their own. Remote Desktop needs Windows' Pro
rem edition or above; on Home this does nothing harmful and WinApps finds no
rem server. It asks to run as an administrator if this does not already.
call "%tools%\losos\winapps\install.bat"
