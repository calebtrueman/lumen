<#
  Lumen for Windows. Run from an elevated PowerShell in the bundle folder:

    powershell -ExecutionPolicy Bypass -File lumen-windows.ps1            # install
    powershell -ExecutionPolicy Bypass -File lumen-windows.ps1 -Uninstall

  Install copies Lumen, Microsoft-signed shim and MokManager to \EFI\lumen\
  on the EFI system partition and adds a firmware boot entry "Lumen" first
  in the boot order. Windows Boot Manager is not modified: if Lumen ever
  can't start, the firmware simply boots Windows.

  Windows updates can reset the boot order (and firmware updates can wipe
  boot entries), so a SYSTEM scheduled task runs this script with -Heal at
  startup, when shutdown begins and after Windows Update installs anything.
  It recreates the entry if missing and moves it back to first.

  Boot entries are edited through the UEFI firmware-variable API rather than
  by parsing bcdedit output, which is localised.
#>
param([switch]$Heal, [switch]$Uninstall)
$ErrorActionPreference = 'Stop'

$Data = Join-Path $env:ProgramData 'Lumen'
$TaskName = 'Lumen boot order'
$Label = 'Lumen'
$Arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aa64' } else { 'x64' }
$Loader = "\EFI\lumen\shim$Arch.efi"
$EspType = '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}'

Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class LumenFw {
    public const string GlobalGuid = "{8be4df61-93ca-11d2-aa0d-00e098032b8c}";
    public const string ShimGuid = "{605dab50-e046-4300-abb6-3dd810dd8b23}";
    public const uint NvBsRt = 7;

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern uint GetFirmwareEnvironmentVariableExW(string name, string guid, byte[] buf, uint size, out uint attr);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool SetFirmwareEnvironmentVariableExW(string name, string guid, byte[] buf, uint size, uint attr);
    [DllImport("kernel32.dll")]
    static extern IntPtr GetCurrentProcess();
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool LookupPrivilegeValueW(string system, string name, out long luid);
    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    struct TokenPrivileges { public uint Count; public long Luid; public uint Attributes; }
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool AdjustTokenPrivileges(IntPtr token, bool disableAll, ref TokenPrivileges state, uint len, IntPtr prev, IntPtr retLen);

    public static void EnablePrivilege() {
        IntPtr token;
        if (!OpenProcessToken(GetCurrentProcess(), 0x28, out token)) throw new Win32Exception();
        TokenPrivileges tp = new TokenPrivileges { Count = 1, Attributes = 2 };
        if (!LookupPrivilegeValueW(null, "SeSystemEnvironmentPrivilege", out tp.Luid)) throw new Win32Exception();
        if (!AdjustTokenPrivileges(token, false, ref tp, 0, IntPtr.Zero, IntPtr.Zero)) throw new Win32Exception();
    }

    public static byte[] Get(string name, string guid) {
        byte[] buf = new byte[65536];
        uint attr;
        uint n = GetFirmwareEnvironmentVariableExW(name, guid, buf, (uint)buf.Length, out attr);
        if (n == 0) return null;
        Array.Resize(ref buf, (int)n);
        return buf;
    }

    public static void Set(string name, string guid, byte[] data, uint attr) {
        if (!SetFirmwareEnvironmentVariableExW(name, guid, data, (uint)(data == null ? 0 : data.Length), attr))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Couldn't write firmware variable " + name);
    }

    public static int IndexOf(byte[] hay, byte[] needle) {
        if (hay == null) return -1;
        for (int i = 0; i + needle.Length <= hay.Length; i++) {
            int j = 0;
            while (j < needle.Length && hay[i + j] == needle[j]) j++;
            if (j == needle.Length) return i;
        }
        return -1;
    }
}
'@

[LumenFw]::EnablePrivilege()

# ---- firmware boot entries ------------------------------------------------

function Get-BootOrder {
    $b = [LumenFw]::Get('BootOrder', [LumenFw]::GlobalGuid)
    if (-not $b) { return @() }
    for ($i = 0; $i + 1 -lt $b.Length; $i += 2) { [BitConverter]::ToUInt16($b, $i) }
}

function Set-BootOrder([uint16[]]$order) {
    $bytes = [byte[]]($order | ForEach-Object { [BitConverter]::GetBytes([uint16]$_) } | ForEach-Object { $_ })
    [LumenFw]::Set('BootOrder', [LumenFw]::GlobalGuid, $bytes, [LumenFw]::NvBsRt)
}

function Get-BootDescription([uint16]$num) {
    $d = [LumenFw]::Get(('Boot{0:X4}' -f $num), [LumenFw]::GlobalGuid)
    if (-not $d -or $d.Length -lt 8) { return $null }
    $end = 6
    while ($end + 1 -lt $d.Length -and ($d[$end] -ne 0 -or $d[$end + 1] -ne 0)) { $end += 2 }
    [Text.Encoding]::Unicode.GetString($d, 6, $end - 6)
}

function Find-LumenEntry {
    # Entries in BootOrder first, then orphans the firmware dropped from it.
    $candidates = @(Get-BootOrder) + (0..0xFF)
    foreach ($n in $candidates) {
        if ((Get-BootDescription $n) -eq $Label) { return [uint16]$n }
    }
    $null
}

function Get-EspPartition {
    $systemDisk = (Get-Partition -DriveLetter $env:SystemDrive.Substring(0, 1)).DiskNumber
    Get-Partition | Where-Object { $_.GptType -eq $EspType } |
        Sort-Object { $_.DiskNumber -ne $systemDisk } | Select-Object -First 1
}

function New-LoadOption($part, [string]$file, [string]$desc) {
    # EFI_LOAD_OPTION with HD(part, GPT, guid, start, size)/File(path)/End.
    $sector = (Get-Disk -Number $part.DiskNumber).LogicalSectorSize
    $hd = New-Object byte[] 42
    $hd[0] = 4; $hd[1] = 1; [BitConverter]::GetBytes([uint16]42).CopyTo($hd, 2)
    [BitConverter]::GetBytes([uint32]$part.PartitionNumber).CopyTo($hd, 4)
    [BitConverter]::GetBytes([uint64]($part.Offset / $sector)).CopyTo($hd, 8)
    [BitConverter]::GetBytes([uint64]($part.Size / $sector)).CopyTo($hd, 16)
    ([Guid]$part.Guid).ToByteArray().CopyTo($hd, 24)   # .NET byte order == EFI GUID byte order
    $hd[40] = 2; $hd[41] = 2                            # GPT, GUID signature
    $name = [Text.Encoding]::Unicode.GetBytes($file + [char]0)
    $fp = New-Object byte[] (4 + $name.Length)
    $fp[0] = 4; $fp[1] = 4; [BitConverter]::GetBytes([uint16]$fp.Length).CopyTo($fp, 2); $name.CopyTo($fp, 4)
    [byte[]]$path = $hd + $fp + [byte[]](0x7F, 0xFF, 4, 0)
    [byte[]]$d = [Text.Encoding]::Unicode.GetBytes($desc + [char]0)
    [byte[]]([BitConverter]::GetBytes([uint32]1) + [BitConverter]::GetBytes([uint16]$path.Length) + $d + $path)
}

function New-LumenEntry {
    $part = Get-EspPartition
    $num = [uint16](0..0x0FFF | Where-Object { -not [LumenFw]::Get(('Boot{0:X4}' -f $_), [LumenFw]::GlobalGuid) } | Select-Object -First 1)
    [LumenFw]::Set(('Boot{0:X4}' -f $num), [LumenFw]::GlobalGuid, (New-LoadOption $part $Loader $Label), [LumenFw]::NvBsRt)
    $num
}

function Invoke-Heal {
    $num = Find-LumenEntry
    if ($null -eq $num) {
        if (-not (Test-LumenFiles)) { return }   # uninstalled from the ESP; nothing to heal
        $num = New-LumenEntry
        Write-Output "Recreated the Lumen boot entry (Boot$('{0:X4}' -f $num))."
    }
    $order = @(Get-BootOrder)
    if ($order.Count -eq 0 -or $order[0] -ne $num) {
        Set-BootOrder (@($num) + @($order | Where-Object { $_ -ne $num }))
        Write-Output "Moved Lumen back to the front of the boot order."
    }
}

# ---- EFI system partition -------------------------------------------------

function Use-Esp([scriptblock]$body) {
    $letter = (70..90 | ForEach-Object { [char]$_ } | Where-Object { -not (Test-Path "$($_):\") } | Select-Object -Last 1)
    mountvol "$($letter):" /S | Out-Null
    try { & $body "$($letter):" } finally { mountvol "$($letter):" /D | Out-Null }
}

function Test-LumenFiles { Use-Esp { param($esp) Test-Path "$esp$Loader" } }

# ---- Secure Boot key enrollment --------------------------------------------

function Get-MokRequest([byte[]]$cert, [string]$password) {
    # Same request mokutil --import writes: MokNew = EFI_SIGNATURE_LIST with
    # the certificate; MokAuth = SHA-256(MokNew || password as UTF-16LE).
    $sigSize = 16 + $cert.Length
    [byte[]]$new = ([Guid]'a5c059a1-94e4-4aa7-87b5-ab155c2bf072').ToByteArray() +
        [BitConverter]::GetBytes([uint32](28 + $sigSize)) + [BitConverter]::GetBytes([uint32]0) +
        [BitConverter]::GetBytes([uint32]$sigSize) + ([Guid]'605dab50-e046-4300-abb6-3dd810dd8b23').ToByteArray() + $cert
    [byte[]]$auth = [Security.Cryptography.SHA256]::Create().ComputeHash([byte[]]($new + [Text.Encoding]::Unicode.GetBytes($password)))
    @{ New = $new; Auth = $auth }
}

function Request-MokEnrollment([byte[]]$cert) {
    while ($true) {
        $p1 = Read-Host 'Choose a one-time password (you will type it once at the next boot)' -AsSecureString
        $p2 = Read-Host 'Type it again' -AsSecureString
        $a = [Runtime.InteropServices.Marshal]::PtrToStringBSTR([Runtime.InteropServices.Marshal]::SecureStringToBSTR($p1))
        $b = [Runtime.InteropServices.Marshal]::PtrToStringBSTR([Runtime.InteropServices.Marshal]::SecureStringToBSTR($p2))
        if ($a -and $a -eq $b -and $a.Length -le 16) { break }
        Write-Warning 'Passwords must match and be 1-16 characters.'
    }
    $req = Get-MokRequest $cert $a
    [LumenFw]::Set('MokNew', [LumenFw]::ShimGuid, $req.New, [LumenFw]::NvBsRt)
    [LumenFw]::Set('MokAuth', [LumenFw]::ShimGuid, $req.Auth, [LumenFw]::NvBsRt)
}

# ---- modes -------------------------------------------------------------------

if ($Heal) { Invoke-Heal; return }

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Please run this from an elevated (Administrator) PowerShell.'
}
if ($env:firmware_type -ne 'UEFI') { throw "This PC isn't booted in UEFI mode; Lumen needs UEFI." }

if ($Uninstall) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    $num = Find-LumenEntry
    if ($null -ne $num) {
        Set-BootOrder @(Get-BootOrder | Where-Object { $_ -ne $num })
        [LumenFw]::Set(('Boot{0:X4}' -f $num), [LumenFw]::GlobalGuid, $null, [LumenFw]::NvBsRt)
    }
    Use-Esp { param($esp) Remove-Item -Recurse -Force "$esp\EFI\lumen" -ErrorAction SilentlyContinue }
    Remove-Item -Recurse -Force $Data -ErrorAction SilentlyContinue
    Write-Output 'Lumen removed. Your PC will boot as it did before.'
    return
}

$Bundle = $PSScriptRoot
foreach ($f in 'lumen.efi', "shim$Arch.efi", "mm$Arch.efi", 'lumen.cer') {
    if (-not (Test-Path (Join-Path $Bundle $f))) { throw "Missing $f next to this script (use the bundle from tools/dist.sh)." }
}

$bitlocker = $false
try { $bitlocker = (Get-BitLockerVolume -MountPoint $env:SystemDrive -ErrorAction Stop).ProtectionStatus -eq 'On' } catch {}

Use-Esp {
    param($esp)
    $dir = "$esp\EFI\lumen"
    New-Item -ItemType Directory -Force $dir | Out-Null
    Copy-Item (Join-Path $Bundle "shim$Arch.efi"), (Join-Path $Bundle "mm$Arch.efi"), (Join-Path $Bundle 'lumen.cer') $dir -Force
    Copy-Item (Join-Path $Bundle 'lumen.efi') "$dir\grub$Arch.efi" -Force   # shim starts grub<arch>.efi from its folder
    if (-not (Test-Path "$dir\lumen.conf")) {
        $conf = if (Test-Path (Join-Path $Bundle 'lumen.conf')) { Get-Content -Raw (Join-Path $Bundle 'lumen.conf') } else { "timeout 5`r`ndefault last`r`n" }
        if ($bitlocker) {
            # BitLocker measures the boot chain: let the firmware start
            # Windows itself so it never asks for the recovery key.
            $conf += "`r`n# Added by installer: BitLocker detected`r`nbootnext Windows`r`n"
        }
        Set-Content -Path "$dir\lumen.conf" -Value $conf -Encoding ascii
    }
}
Write-Output 'Copied Lumen to the EFI system partition.'
if ($bitlocker) { Write-Output 'BitLocker detected: Windows will be started through its own firmware entry.' }

Invoke-Heal

# Self-healing task.
New-Item -ItemType Directory -Force $Data | Out-Null
Copy-Item $PSCommandPath (Join-Path $Data 'lumen-windows.ps1') -Force
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$Data\lumen-windows.ps1`" -Heal"
$eventClass = Get-CimClass -Namespace Root/Microsoft/Windows/TaskScheduler -ClassName MSFT_TaskEventTrigger
function New-EventTrigger([string]$log, [string]$query) {
    $t = New-CimInstance -CimClass $eventClass -ClientOnly
    $t.Enabled = $true
    $t.Subscription = "<QueryList><Query Id=`"0`" Path=`"$log`"><Select Path=`"$log`">$query</Select></Query></QueryList>"
    $t
}
$triggers = @(
    (New-ScheduledTaskTrigger -AtStartup),
    (New-EventTrigger 'System' "*[System[Provider[@Name='User32'] and EventID=1074]]"),   # shutdown/restart begins
    (New-EventTrigger 'Microsoft-Windows-WindowsUpdateClient/Operational' '*[System[EventID=19]]')   # update installed
)
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 2)
Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $triggers -Settings $settings -User 'SYSTEM' -RunLevel Highest -Force | Out-Null
Write-Output "Installed scheduled task '$TaskName' to keep Lumen first after updates."

$sb = $false
try { $sb = Confirm-SecureBootUEFI } catch {}
if ($sb) {
    [byte[]]$cert = [IO.File]::ReadAllBytes((Join-Path $Bundle 'lumen.cer'))
    $enrolled = [LumenFw]::IndexOf([LumenFw]::Get('MokListRT', [LumenFw]::ShimGuid), $cert) -ge 0
    if ($enrolled) {
        Write-Output "Secure Boot: Lumen's key is already enrolled."
    } else {
        Write-Output ''
        Write-Output "Secure Boot is on, so Lumen's key needs a one-time approval."
        Request-MokEnrollment $cert
        Write-Output @'

On the next boot a blue "Shim UEFI key management" screen appears:
  press a key -> Enroll MOK -> Continue -> Yes -> type the password -> Reboot
'@
    }
}
Write-Output 'Done. Lumen will appear on the next boot.'
