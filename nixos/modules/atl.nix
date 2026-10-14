# Android app support, through the Android Translation Layer.
#
# ATL runs Android apps as native Linux processes: it translates Android's Java
# and NDK APIs onto GTK4, Vulkan and the host's own libraries, so an app shows
# up as an ordinary window in the derisk session rather than inside an emulated
# Android. Off by default: it pulls in a large dependency closure (WebKitGTK,
# FFmpeg, the Vulkan and OpenXR loaders) that an image with no interest in
# Android apps should not carry, and its package is still being brought up
# (see nixos/pkgs/android-translation-layer.nix).
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.losos.android;

  # An APK's code is the APK author's, so it runs as Flatpak runs an app:
  # in its own namespaces under bubblewrap, seeing the Nix store, /etc and
  # the GPU read-only, its own home and nothing else of the user's. ATL
  # running it unconfined gave it the user's home, Wayland session, agent
  # sockets and everything else in the runtime directory.
  #
  #   losos-android-run APP.apk
  #
  # What it gets:
  # - a home of its own, ~/.local/share/losos/android/<name>, <name> being
  #   the APK's file name without .apk, so an update to the same file name
  #   keeps the app's data. The user's own home is not there at all.
  # - the APK, read-only, as /run/losos-android/<name>.apk.
  # - the Wayland socket and PipeWire's (sound, and with it the
  #   microphone), and none of the runtime directory's other sockets.
  # - the session bus only through xdg-dbus-proxy, which lets it talk to
  #   the desktop portals (file chooser, open URI, notifications) and to
  #   nothing else.
  # - the network, which nearly every Android app expects.
  # - an environment of the variables below only: no SSH_AUTH_SOCK, no
  #   GNUPGHOME, nothing a session set that it was not given.
  #
  # bubblewrap needs unprivileged user namespaces, as Flatpak does, and the
  # PC kernel has them. The runner is a parameter so the sandbox can be
  # tested with a program other than ATL (losos-android-sandbox check).
  sandbox =
    runner:
    pkgs.writeShellApplication {
      name = "losos-android-run";
      runtimeInputs = [
        pkgs.bubblewrap
        pkgs.coreutils
        pkgs.xdg-dbus-proxy
      ];
      text = ''
        apk=$(realpath -e -- "''${1:?usage: losos-android-run APP.apk}")
        name=$(basename -- "$apk" .apk)
        runtime=''${XDG_RUNTIME_DIR:?no XDG_RUNTIME_DIR}
        data="''${XDG_DATA_HOME:-$HOME/.local/share}/losos/android/$name"
        mkdir -p "$data"

        args=(
          --unshare-all --share-net --die-with-parent --new-session
          --cap-drop ALL
          --ro-bind /nix/store /nix/store
          --ro-bind /etc /etc
          --ro-bind-try /run/current-system /run/current-system
          --ro-bind-try /run/opengl-driver /run/opengl-driver
          --proc /proc
          --dev /dev
          --dev-bind-try /dev/dri /dev/dri
          --ro-bind-try /sys/bus /sys/bus
          --ro-bind-try /sys/class /sys/class
          --ro-bind-try /sys/dev /sys/dev
          --ro-bind-try /sys/devices /sys/devices
          --tmpfs /tmp
          --dir "$runtime" --chmod 0700 "$runtime"
          --bind "$data" "$HOME"
          --ro-bind "$apk" "/run/losos-android/$name.apk"
          --clearenv
          --setenv HOME "$HOME"
          --setenv XDG_RUNTIME_DIR "$runtime"
          --setenv PATH "$PATH"
        )
        for var in LANG LANGUAGE LC_ALL XDG_DATA_DIRS XDG_CONFIG_DIRS XDG_SESSION_TYPE \
          GDK_BACKEND GTK_THEME XCURSOR_THEME XCURSOR_SIZE XCURSOR_PATH \
          LOCALE_ARCHIVE TZDIR GIO_EXTRA_MODULES GSETTINGS_SCHEMA_DIR; do
          if [ -n "''${!var:-}" ]; then
            args+=(--setenv "$var" "''${!var}")
          fi
        done
        if [ -n "''${WAYLAND_DISPLAY:-}" ] && [ -S "$runtime/$WAYLAND_DISPLAY" ]; then
          args+=(--ro-bind "$runtime/$WAYLAND_DISPLAY" "$runtime/$WAYLAND_DISPLAY")
          args+=(--setenv WAYLAND_DISPLAY "$WAYLAND_DISPLAY")
        fi
        if [ -S "$runtime/pipewire-0" ]; then
          args+=(--ro-bind "$runtime/pipewire-0" "$runtime/pipewire-0")
        fi

        proxy=
        dir=$(mktemp -d "$runtime/losos-android.XXXXXX")
        cleanup() {
          if [ -n "$proxy" ]; then kill "$proxy" 2> /dev/null || true; fi
          rm -rf "$dir"
        }
        trap cleanup EXIT
        if [ -n "''${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
          xdg-dbus-proxy "$DBUS_SESSION_BUS_ADDRESS" "$dir/bus" --filter \
            --talk='org.freedesktop.portal.*' \
            --broadcast='org.freedesktop.portal.*=@/org/freedesktop/portal/*' &
          proxy=$!
          for _ in $(seq 50); do
            [ -S "$dir/bus" ] && break
            sleep 0.1
          done
          if [ -S "$dir/bus" ]; then
            args+=(--ro-bind "$dir/bus" "$runtime/bus")
            args+=(--setenv DBUS_SESSION_BUS_ADDRESS "unix:path=$runtime/bus")
          fi
        fi

        bwrap "''${args[@]}" -- ${runner} "/run/losos-android/$name.apk"
      '';
    };

  run = sandbox (lib.getExe pkgs.android-translation-layer);

  # Open an .apk from the file manager by handing it to ATL, inside the
  # sandbox above. ATL picks the launchable activity itself, so the handler
  # passes only the file. %f, not %F: ATL runs one app per process.
  atlDesktopItem = pkgs.makeDesktopItem {
    name = "android-translation-layer";
    desktopName = "Android app";
    exec = "${lib.getExe run} %f";
    mimeTypes = [ "application/vnd.android.package-archive" ];
    # An installed app is launched by ATL, not shown in the app grid itself;
    # this entry exists to be the default handler for APK files.
    noDisplay = true;
  };
in
{
  options.losos.android = {
    enable = lib.mkEnableOption ''
      running Android apps on the desktop through the Android Translation
      Layer. This installs ATL; whether opening an .apk runs it is a separate
      choice, openApks
    '';

    openApks = lib.mkEnableOption ''
      making ATL the default handler for .apk files, so "Open" on one in the
      file manager runs it, through losos-android-run's bubblewrap sandbox:
      its own home, the Wayland and PipeWire sockets, the desktop portals
      and the network, and nothing else of the user's. Still a separate
      choice, because the sandbox is not Android's: the app keeps the
      network and the microphone, and runs as soon as the file is opened
    '';
  };

  config = lib.mkMerge [
    {
      # Whether or not Android apps are on, so the sandbox can be tried
      # with a stand-in for ATL, which does not build yet.
      system.build.androidSandbox = sandbox;
    }
    (lib.mkIf cfg.enable {
      # The handler entry is installed only when openApks is on: without it, an
      # .apk has no default application and nothing runs on "Open".
      environment.systemPackages = [
        pkgs.android-translation-layer
        run
      ]
      ++ lib.optional cfg.openApks atlDesktopItem;

      # Make ATL the default application for APKs, so the file manager's "Open"
      # routes to it. The MIME type is the one Android itself uses for packages.
      # Gated on openApks for the reason its option text gives.
      xdg.mime = lib.mkIf cfg.openApks {
        enable = true;
        defaultApplications."application/vnd.android.package-archive" = "android-translation-layer.desktop";
      };

      # ATL renders with GTK4 + Vulkan and talks to the GPU directly; the desktop
      # already enables hardware.graphics, but keep it explicit so enabling
      # Android support alone is sufficient.
      hardware.graphics.enable = true;
    })
  ];
}
