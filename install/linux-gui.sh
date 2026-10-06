#!/bin/sh
# Lumen one-click installer for Linux.
#
#   Double-click it (allow it to run as a program), or:  sh Lumen-Installer-Linux.run
#   Remove Lumen again:                                   sh Lumen-Installer-Linux.run --uninstall
#
# It shows native dialogs (zenity/kdialog) when a desktop is available and
# falls back to the terminal otherwise. The actual work is done by
# install-linux.sh as root; see that script for what it changes.
set -eu

PAYLOAD_LINE=__PAYLOAD_LINE__
SELF=$(cd "$(dirname "$0")" && pwd)/$(basename "$0")
MODE=install
[ "${1:-}" = "--uninstall" ] && MODE=uninstall

# ---- user interface ----------------------------------------------------------------

UI=tty
if [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    if command -v zenity >/dev/null; then UI=zenity
    elif command -v kdialog >/dev/null; then UI=kdialog
    fi
fi
if [ $UI = tty ] && ! [ -t 0 ] && [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    # Launched from a file manager without a dialog tool: reopen in a terminal.
    for t in x-terminal-emulator gnome-terminal konsole xfce4-terminal kgx tilix alacritty kitty xterm; do
        if command -v $t >/dev/null; then
            case $t in
                gnome-terminal|kgx|tilix) exec $t -- sh "$SELF" "$@" ;;
                *) exec $t -e sh "$SELF" "$@" ;;
            esac
        fi
    done
    exit 1
fi

ask() { # title text yes-label no-label
    case $UI in
        zenity) zenity --question --title="$1" --text="$2" --ok-label="$3" --cancel-label="$4" --width=480 2>/dev/null ;;
        kdialog) kdialog --title "$1" --yes-label "$3" --no-label "$4" --yesno "$2" 2>/dev/null ;;
        *) printf '\n%s\n\n%s\n\n%s? [Y/n] ' "$1" "$2" "$3"; read -r a; case $a in [nN]*) return 1 ;; esac ;;
    esac
}
info() { # title text
    case $UI in
        zenity) zenity --info --title="$1" --text="$2" --width=480 2>/dev/null || true ;;
        kdialog) kdialog --title "$1" --msgbox "$2" 2>/dev/null || true ;;
        *) printf '\n%s\n\n%s\n\n' "$1" "$2"; [ -t 0 ] && { printf 'Press Enter to close.'; read -r _; } || true ;;
    esac
}
fail() {
    case $UI in
        zenity) zenity --error --title="Lumen" --text="$1" --width=480 2>/dev/null || true ;;
        kdialog) kdialog --title "Lumen" --error "$1" 2>/dev/null || true ;;
        *) printf '\nLumen: %s\n' "$1" >&2; [ -t 0 ] && { printf 'Press Enter to close.'; read -r _; } || true ;;
    esac
    exit 1
}
busy() { # title: shows a pulsing progress window fed from stdin
    case $UI in
        zenity) zenity --progress --pulsate --auto-close --no-cancel --title="$1" --text="Working…" --width=420 2>/dev/null || true ;;
        *) cat ;;
    esac
}

# ---- preflight ----------------------------------------------------------------------

[ -d /sys/firmware/efi ] || fail "This PC started Linux in legacy BIOS mode. Lumen needs UEFI mode, which you can switch to in your firmware settings."
case $(uname -m) in
    x86_64) ARCH=x86_64 ;;
    aarch64) ARCH=aarch64 ;;
    *) fail "Lumen doesn't support this processor ($(uname -m))." ;;
esac

TMP=$(mktemp -d /tmp/lumen-install.XXXXXX)
chmod 755 "$TMP"
trap 'rm -rf "$TMP"' EXIT
tail -n +$PAYLOAD_LINE "$SELF" | tar xzf - -C "$TMP" 2>/dev/null || fail "The installer file is damaged. Please download it again."
B=$TMP/$ARCH
[ -x "$B/install-linux.sh" ] || fail "The installer file is damaged. Please download it again."

as_root() {
    if [ "$(id -u)" -eq 0 ]; then "$@"
    elif [ $UI != tty ] && command -v pkexec >/dev/null; then pkexec "$@"
    elif command -v sudo >/dev/null; then sudo "$@"
    else fail "Lumen needs administrator rights, but neither pkexec nor sudo is available."
    fi
}

installed() { command -v efibootmgr >/dev/null && efibootmgr 2>/dev/null | grep -q ' Lumen'; }
sb_on() { od -An -t u1 /sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c 2>/dev/null | awk 'NF { v = $NF } END { exit !(v == 1) }'; }

# ---- uninstall ------------------------------------------------------------------------

if [ $MODE = uninstall ]; then
    ask "Remove Lumen?" "Lumen will be removed from your PC's boot menu. Your operating systems aren't affected." "Remove" "Cancel" || exit 0
    as_root sh "$B/install-linux.sh" --uninstall --yes --result "$TMP/result" 2>&1 | busy "Removing Lumen" || true
    grep -q '^ok=1' "$TMP/result" 2>/dev/null || fail "Lumen couldn't be removed: $(sed -n 's/^error=//p' "$TMP/result" 2>/dev/null)"
    info "Lumen removed" "Your PC will start the way it did before Lumen was installed."
    exit 0
fi

# ---- install ----------------------------------------------------------------------------

VERB=Install
installed && VERB=Update
TEXT="Lumen adds a graphical menu that appears when your PC starts, so you can choose between your operating systems.

Your current setup is kept: GRUB, Windows and everything else stay as they are, and if Lumen ever has a problem the PC simply starts the way it does today."
if sb_on; then TEXT="$TEXT

Secure Boot stays on. You'll approve Lumen once on the next restart."
else TEXT="$TEXT

You'll approve Lumen once on the next restart, so it keeps working if you turn on Secure Boot later."; fi
ask "$VERB Lumen" "$TEXT" "$VERB" "Cancel" || exit 0

CODE=$(od -An -N2 -tu2 /dev/urandom | awk '{ printf "%04d", $1 % 10000 }')
as_root sh "$B/install-linux.sh" --yes --code "$CODE" --result "$TMP/result" 2>&1 | busy "Installing Lumen" || true
if ! grep -q '^ok=1' "$TMP/result" 2>/dev/null; then
    reason=$(sed -n 's/^error=//p' "$TMP/result" 2>/dev/null)
    fail "Lumen wasn't installed${reason:+: $reason}.

Any changes were undone, so your PC starts as before."
fi

distros=$(sed -n 's/^distros=//p' "$TMP/result")
if grep -q '^mok=queued' "$TMP/result"; then
    info "One last step" "Restart your PC. A blue screen titled \"Shim UEFI key management\" appears once${distros:+ (it approves Lumen and the signing keys of $distros, so Lumen can start them directly)}:

  1.  Press any key
  2.  Choose Enroll MOK, then Continue, then Yes
  3.  Type the code  $CODE  and press Enter, then choose Reboot

Lumen appears after that, and on every start from then on."
else
    info "Lumen is installed" "Restart your PC to see Lumen.

From then on it appears every time your PC starts, even after system updates."
fi
exit 0
