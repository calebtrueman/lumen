# Lumen

![Lumen's boot menu with Windows, Fedora, Ubuntu and Arch Linux, counting down to start Windows](docs/screenshots/menu.png)

A graphical OS picker for UEFI PCs. It runs before any operating system,
finds every OS on every drive and USB stick, and starts the one you pick.
Each OS gets a card with its real logo in its brand colours. Animations are
smooth, and you can use the keyboard or a mouse/touchpad.

Lumen doesn't replace GRUB or Windows Boot Manager; it starts them. Picking
Windows starts `bootmgfw.efi` directly, and picking Ubuntu starts Ubuntu's own
shim/GRUB. Lumen works with Secure Boot on, survives Windows updates and
firmware updates, and can't leave a PC unbootable.

## Finding every OS

Lumen runs in the firmware, so it doesn't depend on `os-prober` running
inside Linux the way GRUB does. It uses several independent sources:

1. **It connects every disk controller first.** Fast Boot firmware often only
   initialises the boot drive, so OSes on second SSDs, NVMe or USB disks would
   otherwise stay invisible.
2. **It scans every EFI partition** for known loaders:
   - Windows Boot Manager
   - macOS
   - each vendor folder under `\EFI\*` (shim, GRUB, systemd-boot, rEFInd,
     FreeBSD `loader.efi`, Clear Linux, Qubes `xen.efi`, elilo)
   - unified kernel images in `\EFI\Linux`
   - the UEFI Shell
3. **It reads the firmware's boot menu (`Boot####`).** Every OS installer
   registers itself there, so a loader at an unusual path still shows up.
   Entries for partitions that no longer exist are hidden.
4. **It detects live and installer USB sticks**, including ones plugged in
   while the menu is open. It works out what's on a stick from the strongest
   evidence available:
   1. Windows setup files, shown as "Windows Setup"
   2. `.disk/info`
   3. systemd-boot and GRUB menu titles
   4. the volume label
   5. the distro's signing certificate embedded in its shim

   So an Ubuntu, Fedora, Debian, Arch, openSUSE or Mint stick shows up by
   name, even with a blank label.

![A Debian live USB plugged in while the menu is open appears as a new card](docs/screenshots/usb.png)

*A Debian live USB plugged in while the menu was open. Its label was blank,
so Lumen recognised it from the Debian certificate inside its shim.*

Partitions are matched by signature *and* file, so cloned disks and sticks
flashed from the same ISO are handled correctly.

## Icons

There are about 85 OS tiles. They use real logos from
[Font Logos](https://github.com/lukas-w/font-logos) (public domain) and
[Font Awesome Free](https://fontawesome.com) brands (CC BY 4.0). They cover:

- Windows and macOS
- the Ubuntu family, Mint, Pop!_OS, elementary and Zorin
- Debian, Kali, MX and Tails
- Fedora, Nobara, RHEL, CentOS, Rocky and Alma
- openSUSE and SUSE
- Arch, Manjaro, EndeavourOS, CachyOS and Garuda
- NixOS, Gentoo, Void, Alpine, Slackware, Solus and Qubes
- SteamOS and Android
- ChromeOS, FreeBSD and OpenBSD
- and many more

Projects without a logo glyph, such as Haiku and Proxmox, get a brand-coloured
letter tile.

![All OS icon tiles](docs/screenshots/icons.png)

To preview every tile: `cargo run --release --manifest-path
tools/gallery/Cargo.toml -- gallery.ppm`.

## Keys and mouse

| Input | Action |
| --- | --- |
| ← → / mouse hover / scroll wheel | choose |
| Enter, Space, click | start |
| 1–9 | start that entry immediately |
| Tab, ↓ / ↑ | OS row ↔ Firmware Settings / Restart / Shut Down |
| Esc / any key / moving the mouse | stop the countdown |
| F5 | rescan (rarely needed: new USB sticks are detected automatically) |

| Mouse | Power row |
| --- | --- |
| ![Hovering a card with the mouse selects it](docs/screenshots/mouse.png) | ![Firmware Settings, Restart and Shut Down](docs/screenshots/power.png) |

Mice and touchpads (relative pointers) and touchscreens/tablets (absolute
pointers) all work, as long as the firmware has a driver for them, which
nearly all PC firmware does. Lumen highlights whatever you booted last time.

## Secure Boot

Lumen boots the way Linux distributions do under Secure Boot:

```
firmware ──▶ shim (signed by Microsoft) ──▶ Lumen (signed with Lumen's key)
                                               ├──▶ Windows Boot Manager
                                               └──▶ distro shim ──▶ GRUB …
```

The installer adds Lumen's public key once as a Machine Owner Key (MOK). On
the next boot a blue *Shim UEFI key management* screen appears:

1. Press a key.
2. Choose **Enroll MOK**, then **Continue**, then **Yes**.
3. Type the one-time password you chose during install.
4. Reboot.

After that, Lumen starts automatically with Secure Boot fully on.

| One-time key approval (shim's MokManager) | A loader Secure Boot refuses |
| --- | --- |
| ![The blue Perform MOK management screen with Enroll MOK](docs/screenshots/mok-enroll.png) | ![Error notice: Couldn't start Windows, it was blocked by Secure Boot](docs/screenshots/secure-boot-blocked.png) |

- **Linux** uses `mokutil`, with a built-in fallback if it isn't installed.
- **Windows** has no `mokutil`, so the installer writes the same enrollment
  request through the Windows firmware API.

Loaders that Secure Boot rejects are reported in the UI, and Lumen falls back
to the firmware's own boot entry for that OS.

**BitLocker.** Starting Windows through any boot manager changes the TPM
measurements, which can trigger a recovery-key prompt. Both installers detect
BitLocker and add `bootnext Windows` to the config. Windows is then started
through the firmware's own entry (one quick reboot), measured exactly like a
normal boot.

## Update-proof

Windows feature updates, distro GRUB updates (`grub-install`) and BIOS
updates can reorder or wipe firmware boot entries. Lumen repairs this in
three places:

- **Lumen itself.** Each time it starts from its own entry, it moves itself
  back to first if something jumped ahead.
- **Windows.** A SYSTEM scheduled task, *Lumen boot order*, runs at startup,
  when shutdown begins, and after Windows Update installs anything. It
  recreates Lumen's entry if it was deleted and puts it back first. It edits
  the firmware variables directly, so it doesn't depend on `bcdedit`'s
  localised output.
- **Linux.** `lumen-heal.service` does the same at every boot and shutdown.

None of these ever remove other entries.

## It can't strand you

- **Lumen is its own entry in the boot order**, alongside Windows Boot
  Manager and GRUB, not a replacement for them. If it's missing, refused or
  broken, the firmware boots the next entry, as it does today.
- **If Lumen crashes, it exits to the firmware**, which boots the next entry.
  It never hangs or powers off.
- **The firmware watchdog is turned off while the menu is open.**

## Install

Download and run the installer for the OS you're using right now. You only
need to do this once, from any one of your systems.

| | |
| --- | --- |
| **Windows** | Double-click **`Lumen-Installer-Windows.exe`** and approve the administrator prompt. Works on x64 and ARM64 PCs. |
| **Linux** | Double-click **`Lumen-Installer-Linux.run`**, or run `sh Lumen-Installer-Linux.run` in a terminal. Works on x86_64 and aarch64. |

The installer shows what will happen, does it, and tells you the one thing
you might need to do yourself. If Secure Boot is on, that's typing a 4-digit
code on a blue approval screen at the next restart. The code is digits only,
because that screen uses a US keyboard layout.

**Why it can't break booting:**

- **Lumen is added next to your existing boot loaders and doesn't replace
  them.** Windows Boot Manager and GRUB are never modified.
- **Lumen is first tried as the *next boot only*.** Your current default stays
  the default until Lumen has actually started on that PC and drawn its menu.
  Only then does it make itself the default. If the first start fails for
  any reason (a skipped Secure Boot approval, odd firmware, a graphics
  problem), the PC simply keeps starting the way it always has.
- **The installer checks everything first and undoes everything if any step
  fails.** That includes UEFI mode, administrator rights, the EFI partition,
  free space and the processor type. On Linux it also installs `efibootmgr`
  if needed and mounts the EFI partition if your distro doesn't.
- **Files are written under a temporary name, then verified.**
- **Logs:** `%ProgramData%\Lumen\install.log` on Windows; terminal output
  on Linux.

**Uninstall:**

- **Windows:** Settings → Apps → *Lumen boot manager* → Uninstall.
- **Linux:** `sh Lumen-Installer-Linux.run --uninstall`.

Either way the PC starts exactly as it did before.

**Silent deployment:** `Lumen-Installer-Windows.exe /quiet` (and
`/quiet /uninstall`). On Linux, use `sudo ./install-linux.sh --yes` from a
bundle.

### Building the installers

```sh
brew install mingw-w64 osslsigncode     # macOS; apt install mingw-w64 osslsigncode on Linux
tools/make-installers.sh                # -> dist/Lumen-Installer-{Windows.exe,Linux.run}, SHA256SUMS
```

This builds both architectures, signs Lumen with `keys/lumen.key`, and embeds
Debian's Microsoft-signed shim, fetched with pinned hashes.

> **Signing key:** PCs that approved Lumen under Secure Boot trust exactly one
> key: the private half of [`release/lumen.cer`](release/README.md), kept in
> `keys/lumen.key` (git-ignored, never committed). Every release must be signed
> with it, so `tools/dist.sh` refuses to build if it's missing or doesn't match
> the committed certificate, and `tools/genkey.sh` won't replace it. **Keep a
> backup** (e.g. in a password manager): if it's lost, every PC has to approve
> a new key. Test builds (CI) use `LUMEN_TEST_KEY=1`, which signs with a
> separate throwaway key; never install those on a real PC.

> **Windows SmartScreen:** until the `.exe` is Authenticode-signed with a code
> signing certificate, Windows shows *"Windows protected your PC"*. Click
> *More info → Run anyway*.

## Configuration

`\EFI\lumen\lumen.conf`:

```
timeout 5            # 0 = boot default immediately, -1 = wait forever
default last         # or part of a title, e.g. "Windows"
hide Recovery        # hide matching entries
bootnext Windows     # start via firmware entry + reboot (BitLocker-safe)
resolution keep      # keep | max | 1920x1080
clock off
entry Arch (fallback) | \EFI\Linux\arch-linux.efi | initrd=\initramfs-linux-fallback.img
```

## Testing in a VM

`tools/vm.py` simulates the following:

| Scenario | How it's tested |
| --- | --- |
| Multiple drives | a SATA Linux disk with Lumen, and an NVMe Windows disk |
| Firmware-only entries | an installer-registered entry at an odd path, plus a stale entry |
| Hot-plugged USB sticks | `--plug usb_debian`, `--plug usb_arch` |
| Scripted mouse | `--mouse-test X Y` |
| Boot-order self-heal | `--heal-test` |
| Secure Boot with Microsoft keys only | `--secureboot`, with the MOK enrolled (`--mok=enrolled`) or queued the way the Windows installer does it (`--mok=queued`) |

Each "OS" is a small test loader that prints how it was started.

```sh
brew install qemu osslsigncode && pip install virt-firmware
cargo build --release --features debugcon --examples --bins && tools/dist.sh
python3 tools/vm.py                                    # interactive window
python3 tools/vm.py --headless --wait 30 --shot menu   # -> target/vm/menu.png
python3 tools/test/check_windows_installer.py pwsh     # Windows installer byte-level checks
tools/linux-vm-test.py debian-13-genericcloud-arm64.qcow2   # real Debian: install, first boot, heal, uninstall
```

Discovery decisions are logged to `target/vm/debug.log` in `debugcon`
builds.

## Known limits

- BIOS/legacy (CSM) installs can't be started from UEFI and are hidden.
- Distros that install into another distro's folder, such as Mint, Pop!_OS,
  Zorin and elementary in `\EFI\ubuntu`, are shown as that folder's name.
  Telling them apart would need reading the Linux root filesystem (ext4/btrfs).
- Under Secure Boot, Lumen can only directly start loaders trusted by the
  firmware's `db`, such as Windows and distro shims. Anything signed only by
  a MOK falls back to its firmware entry.
- Rendering is done in software on the CPU. It's only been tested in QEMU
  (emulated x86 on Apple Silicon), so smoothness on real hardware, especially
  4K panels, still needs checking.

## Layout

| Path | Contents |
| --- | --- |
| `src/discover.rs` | controller connect, partition scan, USB identification, NVRAM merge |
| `src/os.rs` | OS table: names, brand tiles, logos, recognition clues |
| `src/launch.rs` | chainload, BootNext, boot-order self-heal, firmware setup |
| `src/mouse.rs` | relative/absolute pointer input |
| `src/ui.rs` | layout, animation, hit-testing, input |
| `src/gfx.rs`, `src/icons.rs`, `src/text.rs` | SDF renderer, icon tiles, TrueType text |
| `install/` | Linux/Windows installer logic, heal service, MOK request |
| `installer/windows/` | one-click `.exe` launcher (embeds the bundles) |
| `.github/workflows/ci.yml` | builds everything; real install/heal/uninstall on a Windows runner |
| `tools/` | dist/sign/fetch scripts, VM harness, icon gallery, tests |
