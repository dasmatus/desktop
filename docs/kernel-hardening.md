# Kernel hardening

Three of the kernel's own defences are on in every image:
`nixos/modules/kernel-hardening.nix` turns them on, and the
[security report](security-report.md) checks each of them.

| Defence | Where it is set | What the report reads |
|---|---|---|
| Lockdown, integrity mode | the kernel's build | `/sys/kernel/security/lockdown` says `[integrity]` |
| Module signatures | the kernel's build | `/sys/module/module/parameters/sig_enforce` is `Y` |
| IOMMU, strict | the kernel's command line | `/sys/class/iommu` lists the machine's IOMMUs |
| Unprivileged BPF off | `kernel.unprivileged_bpf_disabled = 1` | the sysctl |
| BPF JIT hardening | `net.core.bpf_jit_harden = 2` | the sysctl |

The sysctls apply on a phone too, where the vendor's kernel reads them. Lockdown
and the IOMMU's command line are for the kernel the PC image boots; a Halium
build carries no kernel of its own ([Halium](halium.md)).

## Lockdown

Lockdown keeps even root from changing the running kernel. In integrity mode
the kernel refuses `/dev/mem` and `/dev/port`, an unsigned `kexec`, writes to
model-specific registers and raw PCI configuration space, custom ACPI tables,
and any kernel module that does not carry a valid signature. That last one is
what gives it teeth: a module is kernel code, so a root that could load any
module could switch lockdown off again.

nixpkgs builds its kernel with neither the lockdown module nor module
signatures, because signing means generating a key during the build, and a
kernel signed with a key made fresh each time is never bit-for-bit the same
twice. This image builds its own kernel with both. The build signs every
module it makes with that key and then throws the key away: nixpkgs keeps the
build tree out of the kernel's `dev` output, so nothing built afterwards can
sign a module. The cost is that the kernel, and every package built against
it, comes from this project's CI and its cache rather than cache.nixos.org.

NVIDIA's open module is built inside the same kernel build for that reason
(`nvidia.nix`). nixpkgs builds it as a separate package against the kernel's
`dev` output, which has no key, so its modules would be unsigned and the
kernel would refuse them. The kernel build compiles NVIDIA's source against
its own build tree and installs it with its own `modules_install`, which signs
and compresses it like every other module, then checks that each one carries
a signature.

Integrity mode is forced in the build (`LOCK_DOWN_KERNEL_FORCE_INTEGRITY`)
rather than asked for with `lockdown=` on the command line. Without Secure
Boot, systemd-stub accepts a command line from the boot loader, and a forced
mode cannot be lowered from there. Confidentiality mode would also stop root
reading kernel memory through perf, kprobes, tracefs and BPF, which takes
profilers and tracing away from anyone developing on the machine, for a gain
that only matters once root is the attacker and the boot chain is signed.

### What it costs

- **Hibernation.** A locked-down kernel refuses to hibernate in any mode,
  because a hibernation image is kernel memory written to disk and read back,
  and nothing in mainline Linux authenticates it. Laptops suspend when the lid
  closes and never hibernate ([Hibernation](hibernation.md)).
- **Out-of-tree modules.** A module that is not built inside the kernel's
  build cannot load. NVIDIA's is the only one the image carries; anything else
  (a DKMS-style driver, a module in a system extension) is refused.
- **Low-level tools.** `/dev/mem` readers, MSR writers, `ioperm` users and
  flashing tools that poke hardware directly fail with `EPERM`.

## IOMMU

The IOMMU translates every device's DMA, so a device can reach only the memory
its driver mapped for it. Without one, any device that can master the bus, a
Thunderbolt dock or a malicious PCIe card among them, can read and write all of
memory.

- `intel_iommu=on`, on x86. AMD's IOMMU is on whenever the firmware describes
  one; Intel's is off unless asked for, since nixpkgs leaves
  `INTEL_IOMMU_DEFAULT_ON` unset.
- `iommu.passthrough=0`. Translate DMA for every device rather than let a
  driver ask for an identity mapping. That is already the build's default, and
  saying it keeps a later default from changing it.
- `iommu.strict=1`. Invalidate the IOMMU's cache when a driver unmaps a buffer,
  not in a later batch. In lazy mode a device can still reach a buffer for a
  short while after the kernel freed and reused it. The cost is throughput on
  links much faster than a desktop's.

> [!NOTE]
> A virtual machine has an IOMMU only if its hypervisor emulates one (QEMU's
> `-device intel-iommu`), so the report's IOMMU row fails in most VMs whatever the
> image sets.

## BPF

BPF loads programs into the kernel and compiles them to machine code, and it
has been a steady source of privilege escalation.

- `kernel.unprivileged_bpf_disabled = 1`. nixpkgs' kernel already starts with
  unprivileged BPF off (`BPF_UNPRIV_DEFAULT_OFF`, the value 2), which root may
  turn back on. 1 cannot be undone until the next boot. Every BPF user in the
  image (systemd's cgroup filters, libvirt) runs as root.
- `net.core.bpf_jit_harden = 2`. Blind the constants in every JIT-compiled
  program, root's included, so a program cannot plant chosen machine code in
  executable kernel memory (JIT spraying). 1 would spare root's programs,
  which are the ones systemd loads, and those cost too little to compile to be
  worth sparing.

## Not done

- **Secure Boot.** The UKIs are unsigned, so root can still write a kernel of
  its choosing to the ESP and reboot into it. Lockdown protects the kernel
  that is running, not the next one ([Not done](not-done.md)).
- **The pre-boot DMA window.** `efi=disable_early_pci_dma` would clear bus
  mastering on PCI bridges before the kernel's IOMMU takes over. It is left
  off because the kernel's own help for it warns that it breaks some
  machines, and the image boots on hardware nobody tested.
