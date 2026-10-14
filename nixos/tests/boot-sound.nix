# The start-up sound (boot-sound.nix): the film's closing bars at boot, and
# the same bars reversed at shutdown.
#
# QEMU gives the VM an Intel HD Audio card whose output it writes to a WAV
# file, so the test hears what the machine plays: the boot half has to be
# in the file once the unit has run, and a shutdown has to add the reversed
# half after it, which the test tells apart by where each one is loudest.
#
# Only the module under test, not the whole image, so it boots the test
# driver's kernel quickly and runs without KVM too.
{ lib, ... }:

{
  name = "losos-desktop-boot-sound";

  nodes.machine = {
    imports = [ ../modules/boot-sound.nix ];
    virtualisation.qemu.options = [
      # Relative to the machine's state directory, where QEMU runs.
      "-audiodev wav,id=snd0,path=sound.wav"
      "-device intel-hda"
      "-device hda-output,audiodev=snd0"
    ];
  };

  testScript = ''
    import array
    import math
    import struct

    def heard():
        # QEMU writes the header first, with the rate and channels it chose,
        # and the samples as they come: 16-bit after a 44-byte header.
        data = (machine.state_dir / "sound.wav").read_bytes()
        channels, rate = struct.unpack("<HI", data[22:28])
        samples = array.array("h", data[44 : len(data) // 4 * 4])
        # Loudness every 50 ms.
        step = rate * channels // 20
        return [
            math.sqrt(sum(s * s for s in samples[i : i + step]) / step)
            for i in range(0, len(samples) - step, step)
        ]

    def sound(levels):
        loud = [i for i, l in enumerate(levels) if l > 300]
        return (loud[0], loud[-1], levels.index(max(levels))) if loud else None

    with subtest("the sound card came up and the boot half played"):
        # Not wait_for_unit: without KVM the kernel finds the card long
        # after the rest of the boot, and the unit is inactive until then.
        machine.wait_until_succeeds(
            "test $(systemctl show -P SubState losos-boot-sound.service) = exited"
        )
        assert machine.succeed("systemctl show -P ExecMainStatus losos-boot-sound.service").strip() == "0"
        # A first boot has no saved mixer, which alsa-store must not count
        # as a failure (boot-complete.target would).
        machine.succeed("systemctl is-active alsa-store.service")
        machine.fail("systemctl is-failed --quiet alsa-store.service")
        start = sound(heard())
        assert start, "nothing reached the sound card at boot"
        first, last, peak = start
        # Three seconds that are loudest early: the film's bars die away as
        # it ends.
        assert 40 <= last - first <= 70, f"the boot sound lasted {(last - first) * 50} ms"
        assert peak - first < (last - first) // 2, "the boot sound is loudest late, so it played backwards"

    with subtest("shutting down plays it reversed"):
        machine.shutdown()
        levels = heard()
        stop = sound(levels[last + 1 :])
        assert stop, "nothing reached the sound card at shutdown"
        first2, last2, peak2 = stop
        assert 40 <= last2 - first2 <= 70, f"the shutdown sound lasted {(last2 - first2) * 50} ms"
        assert last2 - peak2 < (last2 - first2) // 2, "the shutdown sound is loudest early, so it was not reversed"
  '';
}
