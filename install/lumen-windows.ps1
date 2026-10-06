<#
  Lumen for Windows.

    Lumen-Installer-Windows.exe                         one-click installer (runs this with -Gui)
    powershell -ExecutionPolicy Bypass -File lumen-windows.ps1 [-Yes] [-Uninstall]

  Install copies Lumen, Microsoft-signed shim and MokManager to \EFI\lumen\
  on the EFI system partition and adds a firmware boot entry "Lumen". It is
  made the *next* boot only; the default stays Windows Boot Manager until
  Lumen has started successfully on this PC once (it then sets the
  LumenHealthy firmware variable and makes itself the default). If that
  first start fails for any reason, the PC keeps booting Windows as before.

  Windows updates can reset the boot order and firmware updates can wipe
  boot entries, so a SYSTEM scheduled task runs this script with -Heal at
  startup, when shutdown begins and after Windows Update installs anything.
  It recreates the entry if missing and, once Lumen has proven itself, puts
  it back first. Boot entries are edited through the UEFI firmware-variable
  API, not by parsing bcdedit output (which is localised).

  Every install step is undone if a later one fails. A log is written to
  %ProgramData%\Lumen\install.log.
#>
param(
    [switch]$Heal,
    [switch]$Diagnose,
    [switch]$Uninstall,
    [switch]$Gui,
    [switch]$Yes,
    [string]$Code,
    [string]$Bundle = $PSScriptRoot
)
$ErrorActionPreference = 'Stop'

$Version = '0.3.0'
$Data = Join-Path $env:ProgramData 'Lumen'
$LogFile = Join-Path $Data 'install.log'
$TaskName = 'Lumen boot order'
$UninstallKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Lumen'
$Label = 'Lumen'
$EspType = '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}'
$LumenGuid = '{4c756d65-6e00-4b6f-9f2a-6c756d656e21}'

function Get-OsArch {
    try {
        if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -eq 'Arm64') { return 'aarch64' }
    } catch {}
    if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64') { return 'aarch64' }
    'x86_64'
}
$OsArch = Get-OsArch
$Arch = if ($OsArch -eq 'aarch64') { 'aa64' } else { 'x64' }
$Loader = "\EFI\lumen\shim$Arch.efi"
$BundleRoot = $Bundle
# The one-click installer ships both architectures side by side.
if (Test-Path (Join-Path $Bundle $OsArch)) { $Bundle = Join-Path $Bundle $OsArch }
$IconFile = @((Join-Path $BundleRoot 'lumen.ico'), (Join-Path $Data 'lumen.ico')) | Where-Object { Test-Path $_ } | Select-Object -First 1

New-Item -ItemType Directory -Force $Data | Out-Null
function Write-Log([string]$msg) {
    Add-Content -Path $LogFile -Value "$(Get-Date -Format s)  $msg" -ErrorAction SilentlyContinue
    # Write-Host, not Write-Output: pipeline output would become part of the
    # calling function's return value (e.g. a boot entry number).
    if (-not $Gui) { Write-Host $msg }
}

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
    [DllImport("user32.dll")]
    public static extern bool SetProcessDPIAware();

    /// Returns 0 on success, or 1300 if the account doesn't hold the privilege.
    public static int EnablePrivilege() {
        IntPtr token;
        if (!OpenProcessToken(GetCurrentProcess(), 0x28, out token)) throw new Win32Exception();
        TokenPrivileges tp = new TokenPrivileges { Count = 1, Attributes = 2 };
        if (!LookupPrivilegeValueW(null, "SeSystemEnvironmentPrivilege", out tp.Luid)) throw new Win32Exception();
        if (!AdjustTokenPrivileges(token, false, ref tp, 0, IntPtr.Zero, IntPtr.Zero)) throw new Win32Exception();
        return Marshal.GetLastWin32Error();
    }

    /// Last error from Get (203 = variable not found).
    public static int LastGetError;

    // Other components (e.g. Confirm-SecureBootUEFI) switch this privilege
    // off again after using it, so it's re-enabled before every call.
    static void Ensure() { try { EnablePrivilege(); } catch { } }

    public static byte[] Get(string name, string guid) {
        Ensure();
        byte[] buf = new byte[65536];
        uint attr;
        uint n = GetFirmwareEnvironmentVariableExW(name, guid, buf, (uint)buf.Length, out attr);
        if (n == 0) { LastGetError = Marshal.GetLastWin32Error(); return null; }
        LastGetError = 0;
        Array.Resize(ref buf, (int)n);
        return buf;
    }

    public static void Set(string name, string guid, byte[] data, uint attr) {
        Ensure();
        if (!SetFirmwareEnvironmentVariableExW(name, guid, data, (uint)(data == null ? 0 : data.Length), attr)) {
            int code = Marshal.GetLastWin32Error();
            throw new Win32Exception(code, "The firmware refused to save " + name + ": " + new Win32Exception(code).Message + " (error " + code + ")");
        }
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

# ---- firmware boot entries ------------------------------------------------

# Some machines (locked-down accounts, some VMs) can't read or write firmware
# variables directly even as administrator, while Windows' own bcdedit still
# can manage boot entries. Detect that and do everything through bcdedit.
$script:UseBcdedit = $false
function Initialize-FirmwareAccess {
    $priv = [LumenFw]::EnablePrivilege()
    # Probe both the boot order and an actual boot entry: some systems allow
    # one but not the other.
    $order = [LumenFw]::Get('BootOrder', [LumenFw]::GlobalGuid)
    $err = [LumenFw]::LastGetError
    if ($order) {
        $null = [LumenFw]::Get(('Boot{0:X4}' -f [BitConverter]::ToUInt16($order, 0)), [LumenFw]::GlobalGuid)
        $err = [LumenFw]::LastGetError
    }
    if (-not $order -or $err -ne 0) {
        $script:UseBcdedit = $true
        Write-Log "Direct firmware variable access unavailable (privilege $priv, read error $err); using bcdedit."
    } else {
        Write-Log 'Using direct firmware variable access.'
    }
}

# The firmware entry's bcdedit identifier, found by its description. Only
# the GUID and our own description text are matched, so it's locale-proof.
function Find-LumenBcdId {
    $text = bcdedit /enum firmware 2>$null | Out-String
    foreach ($block in ($text -split '(\r?\n){2,}')) {
        if ($block -match '(?m)\s{2,}Lumen\s*$') {
            $m = [regex]::Match($block, '\{[0-9a-fA-F-]{36}\}')
            if ($m.Success) { return $m.Value }
        }
    }
    $null
}

function Get-BootOrder {
    $b = [LumenFw]::Get('BootOrder', [LumenFw]::GlobalGuid)
    if (-not $b) { return @() }
    for ($i = 0; $i + 1 -lt $b.Length; $i += 2) { [BitConverter]::ToUInt16($b, $i) }
}

function Set-BootOrder([uint16[]]$order) {
    $bytes = [byte[]]($order | ForEach-Object { [BitConverter]::GetBytes([uint16]$_) } | ForEach-Object { $_ })
    [LumenFw]::Set('BootOrder', [LumenFw]::GlobalGuid, $bytes, [LumenFw]::NvBsRt)
}

function Set-BootNext([uint16]$num) {
    [LumenFw]::Set('BootNext', [LumenFw]::GlobalGuid, [BitConverter]::GetBytes($num), [LumenFw]::NvBsRt)
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

function Test-Healthy { $null -ne [LumenFw]::Get('LumenHealthy', $LumenGuid) }

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

function Test-BootSlotFree([uint16]$n) {
    # Only a definite "not found" (203) means free. Any other failure means
    # we can't tell, and an existing entry must never be overwritten.
    $null -eq [LumenFw]::Get(('Boot{0:X4}' -f $n), [LumenFw]::GlobalGuid) -and [LumenFw]::LastGetError -eq 203
}

function New-LumenEntry {
    $part = Get-EspPartition
    if (-not $part) { throw "Couldn't find the EFI system partition." }
    try {
        $used = @(Get-BootOrder)
        $num = 0..0xFF | Where-Object { $used -notcontains $_ -and (Test-BootSlotFree $_) } | Select-Object -First 1
        if ($null -eq $num) { throw "no free boot entry slot (firmware read error $([LumenFw]::LastGetError))" }
        $num = [uint16]$num
        if (-not (Test-BootSlotFree $num)) { throw ('Boot{0:X4} is no longer free' -f $num) }
        [LumenFw]::Set(('Boot{0:X4}' -f $num), [LumenFw]::GlobalGuid, (New-LoadOption $part $Loader $Label), [LumenFw]::NvBsRt)
        if ((Get-BootDescription $num) -ne $Label) { throw "the firmware didn't keep the new boot entry" }
        Write-Log "Created boot entry Boot$('{0:X4}' -f $num) directly."
        return $num
    } catch {
        Write-Log "Direct boot entry write failed ($($_.Exception.Message)); trying bcdedit."
    }
    # Fallback: let Windows' boot configuration service create it. Only the
    # GUID in bcdedit's output is used, which isn't localised.
    $out = bcdedit /copy '{bootmgr}' /d $Label 2>&1 | Out-String
    $id = [regex]::Match($out, '\{[0-9a-fA-F-]{36}\}').Value
    if (-not $id) { throw "Windows couldn't create a boot entry either: $out" }
    $set = bcdedit /set $id path $Loader 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) { bcdedit /delete $id | Out-Null; throw "Windows couldn't point the boot entry at Lumen: $set" }
    # The firmware entry only exists once it's in the firmware boot order.
    bcdedit /set '{fwbootmgr}' displayorder $id /addlast | Out-Null
    $found = Find-LumenEntry
    if ($null -eq $found) {
        bcdedit /set '{fwbootmgr}' displayorder $id /remove | Out-Null
        bcdedit /delete $id | Out-Null
        throw "This PC's firmware doesn't allow new boot entries to be added (this happens on some virtual machines and centrally managed PCs). Nothing was changed."
    }
    Write-Log "Created boot entry Boot$('{0:X4}' -f $found) via bcdedit."
    $found
}

function Remove-LumenEntry([uint16]$num) {
    Set-BootOrder @(Get-BootOrder | Where-Object { $_ -ne $num })
    [LumenFw]::Set(('Boot{0:X4}' -f $num), [LumenFw]::GlobalGuid, $null, [LumenFw]::NvBsRt)
}

# Can Lumen start right now? Under Secure Boot, shim only starts it if its
# key was approved; otherwise it would stop at shim's error screen. A queued
# approval counts: its screen appears on the next boot, which must go ahead.
function Test-CanStart {
    if (-not (Test-SecureBoot)) { return $true }
    $cer = @((Join-Path $Data 'lumen.cer'), (Join-Path $Bundle 'lumen.cer')) | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $cer) { return $true }
    [byte[]]$cert = [IO.File]::ReadAllBytes($cer)
    (Test-KeyEnrolled $cert) -or ([LumenFw]::IndexOf([LumenFw]::Get('MokNew', [LumenFw]::ShimGuid), $cert) -ge 0)
}

# Promote Lumen to first only once it has proven it runs on this PC, and
# step it aside if it can't start (Secure Boot turned on without approval).
function Invoke-Heal {
    $num = Find-LumenEntry
    if ($null -eq $num) {
        if (-not (Test-LumenFiles)) { return }   # uninstalled from the ESP; nothing to heal
        $order = @(Get-BootOrder)
        $num = [uint16]@(New-LumenEntry)[-1]
        if ((Test-Healthy) -and (Test-CanStart)) { Set-BootOrder (@($num) + $order) } else { Set-BootOrder ($order + @($num)) }
        Write-Log "Recreated the Lumen boot entry (Boot$('{0:X4}' -f $num))."
    }
    $order = @(Get-BootOrder)
    if (-not (Test-CanStart)) {
        # Don't let every boot stop at shim's "Verification failed" screen.
        if ($order.Count -gt 0 -and $order[0] -eq $num) {
            Set-BootOrder (@($order | Where-Object { $_ -ne $num }) + @($num))
            Write-Log 'Secure Boot is on but Lumen is not approved yet: Windows starts directly until the installer is run again.'
        }
        $next = [LumenFw]::Get('BootNext', [LumenFw]::GlobalGuid)
        if ($next -and [BitConverter]::ToUInt16($next, 0) -eq $num) { [LumenFw]::Set('BootNext', [LumenFw]::GlobalGuid, $null, [LumenFw]::NvBsRt) }
        return
    }
    if (-not (Test-Healthy)) { return }
    if ($order.Count -eq 0 -or $order[0] -ne $num) {
        Set-BootOrder (@($num) + @($order | Where-Object { $_ -ne $num }))
        Write-Log 'Moved Lumen back to the front of the boot order.'
    }
}

# ---- EFI system partition -------------------------------------------------

function Use-Esp([scriptblock]$body) {
    $letter = (70..90 | ForEach-Object { [char]$_ } | Where-Object { -not (Test-Path "$($_):\") } | Select-Object -Last 1)
    if (-not $letter) { throw 'No free drive letter to open the EFI system partition.' }
    mountvol "$($letter):" /S | Out-Null
    if (-not (Test-Path "$($letter):\")) { throw "Couldn't open the EFI system partition." }
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

function Test-SecureBoot { try { Confirm-SecureBootUEFI } catch { $false } }

function Test-KeyEnrolled([byte[]]$cert) {
    [LumenFw]::IndexOf([LumenFw]::Get('MokListRT', [LumenFw]::ShimGuid), $cert) -ge 0
}

function New-ApprovalCode {
    # Digits only: the blue approval screen uses a US keyboard layout, and
    # digits are where they're expected on every layout.
    '{0:D4}' -f (Get-Random -Minimum 0 -Maximum 10000)
}

# ---- install / uninstall ---------------------------------------------------

function Test-Preflight {
    if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Lumen needs administrator rights to install.'
    }
    if ($env:firmware_type -ne 'UEFI') {
        throw "This PC starts Windows in legacy BIOS mode. Lumen needs UEFI mode, which you can switch to with Microsoft's MBR2GPT tool."
    }
    if (-not $Uninstall) {
        foreach ($f in 'lumen.efi', "shim$Arch.efi", "mm$Arch.efi", 'lumen.cer') {
            if (-not (Test-Path (Join-Path $Bundle $f))) { throw "The installer is incomplete ($f is missing). Please download it again." }
        }
    }
}

function Get-InstallState {
    $sb = Test-SecureBoot
    [byte[]]$cert = [IO.File]::ReadAllBytes((Join-Path $Bundle 'lumen.cer'))
    $bitlocker = $false
    try { $bitlocker = (Get-BitLockerVolume -MountPoint $env:SystemDrive -ErrorAction Stop).ProtectionStatus -eq 'On' } catch {}
    $enrolled = Test-KeyEnrolled $cert
    if ($script:UseBcdedit -and -not $enrolled) {
        if ($sb) {
            throw "Secure Boot is on, but this account can't change firmware settings, which the one-time Secure Boot approval needs. Run the installer from a regular administrator account."
        }
        Write-Log "Can't queue the Secure Boot approval from this account; Lumen will work while Secure Boot stays off."
    }
    @{
        Installed = ($(if ($script:UseBcdedit) { [bool](Find-LumenBcdId) } else { $null -ne (Find-LumenEntry) })) -or (Test-LumenFiles)
        SecureBoot = $sb
        # Approval is asked for even with Secure Boot off, so turning it on
        # later doesn't stop Lumen from starting.
        NeedsKey = -not $enrolled -and -not $script:UseBcdedit
        BitLocker = $bitlocker
    }
}

function Invoke-Install($state, [string]$code, [scriptblock]$progress) {
    $created = @{ Dir = $false; Entry = $null; BcdId = $null; Order = @(if (-not $script:UseBcdedit) { Get-BootOrder }) }
    try {
        & $progress 'Copying Lumen to the EFI system partition…'
        Use-Esp {
            param($esp)
            $dir = "$esp\EFI\lumen"
            $need = 0; Get-ChildItem $Bundle -File | ForEach-Object { $need += $_.Length }
            $free = (Get-PSDrive $esp.Substring(0, 1)).Free
            if (-not (Test-Path $dir) -and $free -lt $need + 512KB) {
                throw "The EFI system partition is full ($([int]($free / 1KB)) KB free, $([int]($need / 1KB)) KB needed)."
            }
            if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null; $created.Dir = $true }
            $files = @{ "shim$Arch.efi" = "shim$Arch.efi"; "mm$Arch.efi" = "mm$Arch.efi"; 'lumen.cer' = 'lumen.cer'; 'lumen.efi' = "grub$Arch.efi" }
            foreach ($src in $files.Keys) {
                $from = Join-Path $Bundle $src
                $to = Join-Path $dir $files[$src]
                # Write under a temporary name first so a crash never leaves a half-written loader.
                Copy-Item $from "$to.new" -Force
                Move-Item "$to.new" $to -Force
                if ((Get-FileHash $from).Hash -ne (Get-FileHash $to).Hash) { throw "Couldn't write $to correctly." }
            }
            # Installs before 0.3 shipped "timeout 5" from the template; Lumen
            # now waits by default, so replace that untouched template line.
            if (Test-Path "$dir\lumen.conf") {
                $old = Get-Content -Raw "$dir\lumen.conf"
                $pattern = '(?m)^# Seconds before the highlighted entry starts\. 0 = immediately, -1 = wait forever\.\r?\ntimeout 5[ \t]*\r?$'
                if ($old -match $pattern) {
                    $replacement = "# Lumen waits until you choose. To start the highlighted entry automatically,`r`n# use the Auto-start button in Lumen's menu (Off, 5, 10 or 30 seconds), or set`r`n# a number of seconds here (0 = immediately). The button overrides this line.`r`n# timeout 10"
                    Set-Content -Path "$dir\lumen.conf" -Value ([regex]::Replace($old, $pattern, $replacement)) -Encoding ascii -NoNewline
                    Write-Log 'Updated lumen.conf: Lumen now waits until you choose (old 5-second default removed).'
                }
            }
            if (-not (Test-Path "$dir\lumen.conf")) {
                $conf = if (Test-Path (Join-Path $Bundle 'lumen.conf')) { Get-Content -Raw (Join-Path $Bundle 'lumen.conf') } else { "default last`r`n" }
                if ($state.BitLocker) {
                    # BitLocker measures the boot chain: let the firmware start
                    # Windows itself so it never asks for the recovery key.
                    $conf += "`r`n# Added by installer: BitLocker detected`r`nbootnext Windows`r`n"
                }
                Set-Content -Path "$dir\lumen.conf" -Value $conf -Encoding ascii
            }
        }
        Write-Log "Copied Lumen $Version files."

        & $progress 'Adding Lumen to the boot menu…'
        if ($script:UseBcdedit) {
            $id = Find-LumenBcdId
            if (-not $id) {
                $out = bcdedit /copy '{bootmgr}' /d $Label 2>&1 | Out-String
                $id = [regex]::Match($out, '\{[0-9a-fA-F-]{36}\}').Value
                if (-not $id) { throw "Windows couldn't create a boot entry: $out" }
                $created.BcdId = $id
                bcdedit /set $id path $Loader | Out-Null
                if ($LASTEXITCODE -ne 0) { throw "Windows couldn't point the boot entry at Lumen." }
                Write-Log "Created boot entry $id via bcdedit."
            }
            # Can't read LumenHealthy here, so never promote: try it next boot
            # only; the SYSTEM heal task promotes it once Lumen has run.
            bcdedit /set '{fwbootmgr}' displayorder $id /addlast | Out-Null
            bcdedit /set '{fwbootmgr}' bootsequence $id | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "Windows couldn't schedule Lumen for the next boot." }
            Write-Log 'Lumen will be tried on the next boot; the default stays unchanged until it has started once.'
        } else {
        $num = Find-LumenEntry
        if ($null -eq $num) {
            $num = [uint16]@(New-LumenEntry)[-1]
            $created.Entry = $num
        }
        $rest = @($created.Order | Where-Object { $_ -ne $num })
        if (Test-Healthy) {
            Set-BootOrder (@($num) + $rest)
        } else {
            # Not proven on this PC yet: keep the current default, try Lumen next boot.
            Set-BootOrder ($rest + @($num))
            Set-BootNext $num
            Write-Log 'Lumen will be tried on the next boot; the default stays unchanged until it has started once.'
        }
        }

        & $progress 'Setting up automatic repair after updates…'
        if ($PSCommandPath -ne (Join-Path $Data 'lumen-windows.ps1')) { Copy-Item $PSCommandPath (Join-Path $Data 'lumen-windows.ps1') -Force }
        if ($IconFile -and $IconFile -ne (Join-Path $Data 'lumen.ico')) { Copy-Item $IconFile (Join-Path $Data 'lumen.ico') -Force }
        if ($Bundle -ne $Data) { Copy-Item (Join-Path $Bundle 'lumen.cer') (Join-Path $Data 'lumen.cer') -Force }
        Register-HealTask
        Register-Uninstaller

        if ($state.NeedsKey) {
            & $progress 'Preparing Secure Boot approval…'
            [byte[]]$cert = [IO.File]::ReadAllBytes((Join-Path $Bundle 'lumen.cer'))
            $req = Get-MokRequest $cert $code
            [LumenFw]::Set('MokNew', [LumenFw]::ShimGuid, $req.New, [LumenFw]::NvBsRt)
            [LumenFw]::Set('MokAuth', [LumenFw]::ShimGuid, $req.Auth, [LumenFw]::NvBsRt)
            Write-Log 'Queued Secure Boot key approval.'
        }
        Write-Log "Install of Lumen $Version finished."
    } catch {
        Write-Log "Install failed: $($_.Exception.Message). Rolling back."
        try {
            if ($null -ne $created.Entry) { Remove-LumenEntry ([uint16]@($created.Entry)[-1]) }
            if ($created.BcdId) { bcdedit /delete $created.BcdId | Out-Null }
            if ($created.Order.Count -gt 0) { Set-BootOrder $created.Order }
            if ($created.Dir) { Use-Esp { param($esp) Remove-Item -Recurse -Force "$esp\EFI\lumen" -ErrorAction SilentlyContinue } }
        } catch { Write-Log "Rollback problem: $($_.Exception.Message)" }
        throw
    }
}

function Register-HealTask {
    $action = New-ScheduledTaskAction -Execute "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" `
        -Argument "-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$Data\lumen-windows.ps1`" -Heal"
    $eventClass = Get-CimClass -Namespace Root/Microsoft/Windows/TaskScheduler -ClassName MSFT_TaskEventTrigger
    $newEvent = {
        param([string]$log, [string]$query)
        $t = New-CimInstance -CimClass $eventClass -ClientOnly
        $t.Enabled = $true
        $t.Subscription = "<QueryList><Query Id=`"0`" Path=`"$log`"><Select Path=`"$log`">$query</Select></Query></QueryList>"
        $t
    }
    $triggers = @(
        (New-ScheduledTaskTrigger -AtStartup),
        (& $newEvent 'System' "*[System[Provider[@Name='User32'] and EventID=1074]]"),   # shutdown/restart begins
        (& $newEvent 'Microsoft-Windows-WindowsUpdateClient/Operational' '*[System[EventID=19]]')   # update installed
    )
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 2)
    Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $triggers -Settings $settings -User 'SYSTEM' -RunLevel Highest -Force | Out-Null
}

function Register-Uninstaller {
    New-Item -Path $UninstallKey -Force | Out-Null
    $ps = "`"$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe`" -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$Data\lumen-windows.ps1`" -Uninstall"
    $values = @{
        DisplayName = 'Lumen boot manager'
        DisplayVersion = $Version
        Publisher = 'Lumen'
        UninstallString = "$ps -Gui"
        QuietUninstallString = "$ps -Yes"
        DisplayIcon = Join-Path $Data 'lumen.ico'
        InstallLocation = $Data
    }
    foreach ($k in $values.Keys) { Set-ItemProperty -Path $UninstallKey -Name $k -Value $values[$k] }
    Set-ItemProperty -Path $UninstallKey -Name NoModify -Value 1 -Type DWord
    Set-ItemProperty -Path $UninstallKey -Name NoRepair -Value 1 -Type DWord
}

function Invoke-Uninstall {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    if ($script:UseBcdedit) {
        $id = Find-LumenBcdId
        if ($id) { bcdedit /delete $id | Out-Null }
    }
    $num = if ($script:UseBcdedit) { $null } else { Find-LumenEntry }
    if ($null -ne $num) {
        $next = [LumenFw]::Get('BootNext', [LumenFw]::GlobalGuid)
        if ($next -and [BitConverter]::ToUInt16($next, 0) -eq $num) { [LumenFw]::Set('BootNext', [LumenFw]::GlobalGuid, $null, [LumenFw]::NvBsRt) }
        Remove-LumenEntry $num
    }
    Use-Esp { param($esp) Remove-Item -Recurse -Force "$esp\EFI\lumen" -ErrorAction SilentlyContinue }
    foreach ($v in 'LumenHealthy', 'LumenLastBoot') {
        try { [LumenFw]::Set($v, $LumenGuid, $null, [LumenFw]::NvBsRt) } catch {}
    }
    # Withdraw a Secure Boot approval request that was never confirmed.
    # Only a request that contains Lumen's own certificate is touched.
    $cer = @((Join-Path $Bundle 'lumen.cer'), (Join-Path $Data 'lumen.cer')) | Where-Object { Test-Path $_ } | Select-Object -First 1
    $pending = [LumenFw]::Get('MokNew', [LumenFw]::ShimGuid)
    if ($cer -and $pending -and [LumenFw]::IndexOf($pending, [IO.File]::ReadAllBytes($cer)) -ge 0) {
        foreach ($v in 'MokNew', 'MokAuth') { try { [LumenFw]::Set($v, [LumenFw]::ShimGuid, $null, [LumenFw]::NvBsRt) } catch {} }
    }
    Remove-Item -Path $UninstallKey -Recurse -Force -ErrorAction SilentlyContinue
    Write-Log 'Lumen removed.'
    # This script lives in $Data; remove the folder once we've exited.
    Start-Process -WindowStyle Hidden cmd.exe "/c timeout /t 3 >nul & rmdir /s /q `"$Data`""
}

# ---- diagnostics -----------------------------------------------------------------

# Everything needed to understand a failed install, without changing anything
# except one harmless test variable that is deleted straight away.
function Get-DiagnosticReport {
    $lines = New-Object Collections.Generic.List[string]
    function Add([string]$label, [scriptblock]$body) {
        try { $value = (& $body | Out-String).TrimEnd() } catch { $value = "(failed: $($_.Exception.Message))" }
        $lines.Add("== $label")
        $lines.Add($value)
        $lines.Add('')
    }
    $lines.Add("Lumen $Version diagnostics, $(Get-Date -Format s)")
    $lines.Add('')
    Add 'Windows' {
        $os = Get-CimInstance Win32_OperatingSystem
        "$($os.Caption) $($os.Version) build $($os.BuildNumber), $($os.OSArchitecture); OS arch seen: $OsArch"
    }
    Add 'PC' {
        $cs = Get-CimInstance Win32_ComputerSystem
        $bios = Get-CimInstance Win32_BIOS
        $board = Get-CimInstance Win32_BaseBoard
        "Maker: $($cs.Manufacturer)  Model: $($cs.Model)"
        "Board: $($board.Manufacturer) $($board.Product)"
        "Firmware: $($bios.Manufacturer) $($bios.SMBIOSBIOSVersion) ($($bios.ReleaseDate))"
        "Virtual machine: $(if ($cs.HypervisorPresent -and $cs.Model -match 'Virtual|VMware|KVM|QEMU') { 'likely' } else { 'no' })"
    }
    Add 'Firmware mode and security' {
        "Firmware type: $env:firmware_type"
        "Secure Boot: $(Test-SecureBoot)"
        try { "BitLocker on $($env:SystemDrive): $((Get-BitLockerVolume -MountPoint $env:SystemDrive -ErrorAction Stop).ProtectionStatus)" } catch { "BitLocker: unknown ($($_.Exception.Message))" }
        try {
            $dg = Get-CimInstance -Namespace root\Microsoft\Windows\DeviceGuard -ClassName Win32_DeviceGuard -ErrorAction Stop
            "Virtualization-based security: $(@('off','configured','running')[[int]$dg.VirtualizationBasedSecurityStatus])"
            "Security services running: $(($dg.SecurityServicesRunning | ForEach-Object { @{1='Credential Guard';2='Memory integrity (HVCI)';3='System Guard';4='SMM firmware measurement';5='Kernel-mode stack protection';7='Hypervisor-enforced paging translation'}[[int]$_] }) -join ', ')"
        } catch { "Device Guard: unknown" }
    }
    Add 'Firmware variable access' {
        $priv = [LumenFw]::EnablePrivilege()
        "Privilege SeSystemEnvironmentPrivilege: $(if ($priv -eq 0) { 'enabled' } else { "not held (error $priv)" })"
        $order = [LumenFw]::Get('BootOrder', [LumenFw]::GlobalGuid)
        "BootOrder: $(if ($order) { (@(Get-BootOrder) | ForEach-Object { '{0:X4}' -f $_ }) -join ',' } else { 'read failed, error ' + [LumenFw]::LastGetError })"
        foreach ($v in 'BootCurrent', 'BootNext', 'Timeout', 'SecureBoot', 'OsIndicationsSupported') {
            $d = [LumenFw]::Get($v, [LumenFw]::GlobalGuid)
            "${v}: $(if ($d) { ($d | ForEach-Object { '{0:x2}' -f $_ }) -join ' ' } else { 'error ' + [LumenFw]::LastGetError })"
        }
        foreach ($n in (@(Get-BootOrder) + (0..15)) | Select-Object -Unique) {
            $d = Get-BootDescription $n
            'Boot{0:X4}: {1}' -f $n, $(if ($d) { $d } else { "(no entry, error $([LumenFw]::LastGetError))" })
        }
        # Harmless write test with Lumen's own variable, deleted immediately.
        try {
            [LumenFw]::Set('LumenDiagTest', $LumenGuid, [byte[]](1, 2, 3), [LumenFw]::NvBsRt)
            $back = [LumenFw]::Get('LumenDiagTest', $LumenGuid)
            [LumenFw]::Set('LumenDiagTest', $LumenGuid, $null, [LumenFw]::NvBsRt)
            "Write test (own variable): $(if ($back) { 'OK' } else { 'written but not readable, error ' + [LumenFw]::LastGetError })"
        } catch { "Write test (own variable): FAILED: $($_.Exception.Message)" }
        "Lumen boot entry: $(if ($null -ne ($n = Find-LumenEntry)) { 'Boot{0:X4}' -f $n } else { 'none' })   LumenHealthy: $(Test-Healthy)"
    }
    Add 'bcdedit /enum firmware' { bcdedit /enum firmware 2>&1 }
    Add 'EFI system partition' {
        Get-EspPartition | Select-Object DiskNumber, PartitionNumber, @{n='SizeMB';e={[int]($_.Size / 1MB)}}, Guid | Format-List | Out-String
        Use-Esp { param($esp)
            "Free: $([int]((Get-PSDrive $esp.Substring(0, 1)).Free / 1KB)) KB"
            Get-ChildItem "$esp\EFI" -Directory | ForEach-Object { "\EFI\$($_.Name)" }
        }
    }
    Add 'Scheduled task' { (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue | Select-Object TaskName, State | Out-String) + "(none means not installed)" }
    Add 'install.log' { if (Test-Path $LogFile) { Get-Content $LogFile -Tail 200 } else { '(none)' } }
    $lines -join "`r`n"
}

# Saves the report where people will find it; returns the path.
function Save-DiagnosticReport {
    $desktop = [Environment]::GetFolderPath('Desktop')
    $dir = if ($desktop -and (Test-Path $desktop)) { $desktop } else { $Data }
    $path = Join-Path $dir 'Lumen diagnostics.txt'
    Set-Content -Path $path -Value (Get-DiagnosticReport) -Encoding UTF8
    $path
}

# ---- graphical front end ---------------------------------------------------

function New-Window {
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    [void][LumenFw]::SetProcessDPIAware()
    [Windows.Forms.Application]::EnableVisualStyles()
    $f = New-Object Windows.Forms.Form
    $f.Text = 'Lumen'
    $f.ClientSize = New-Object Drawing.Size(560, 380)
    $f.StartPosition = 'CenterScreen'
    $f.FormBorderStyle = 'FixedDialog'
    $f.MaximizeBox = $false
    $f.BackColor = [Drawing.Color]::FromArgb(18, 20, 34)
    $f.ForeColor = [Drawing.Color]::White
    $f.Font = New-Object Drawing.Font('Segoe UI', 10)
    if ($IconFile) { $f.Icon = New-Object Drawing.Icon($IconFile) }

    $title = New-Object Windows.Forms.Label
    $title.Font = New-Object Drawing.Font('Segoe UI Semibold', 20)
    $title.SetBounds(32, 24, 500, 44)
    $body = New-Object Windows.Forms.Label
    $body.SetBounds(34, 78, 494, 150)
    $body.ForeColor = [Drawing.Color]::FromArgb(205, 208, 225)
    $codeBox = New-Object Windows.Forms.Label
    $codeBox.Font = New-Object Drawing.Font('Segoe UI Semibold', 30)
    $codeBox.ForeColor = [Drawing.Color]::FromArgb(140, 160, 255)
    $codeBox.TextAlign = 'MiddleCenter'
    $codeBox.SetBounds(32, 232, 496, 60)
    $bar = New-Object Windows.Forms.ProgressBar
    $bar.Style = 'Marquee'
    $bar.SetBounds(34, 250, 494, 8)
    $primary = New-Object Windows.Forms.Button
    $primary.SetBounds(392, 316, 136, 38)
    $primary.FlatStyle = 'Flat'
    $primary.BackColor = [Drawing.Color]::FromArgb(92, 102, 255)
    $primary.FlatAppearance.BorderSize = 0
    $secondary = New-Object Windows.Forms.Button
    $secondary.SetBounds(244, 316, 136, 38)
    $secondary.FlatStyle = 'Flat'
    $secondary.FlatAppearance.BorderColor = [Drawing.Color]::FromArgb(70, 74, 100)
    $f.Controls.AddRange(@($title, $body, $codeBox, $bar, $primary, $secondary))
    $f.AcceptButton = $primary

    $w = @{ Form = $f; Title = $title; Body = $body; Code = $codeBox; Bar = $bar; Primary = $primary; Secondary = $secondary; Choice = 'close' }
    $primary.add_Click({ $w.Choice = 'primary'; $w.Form.Hide() }.GetNewClosure())
    $secondary.add_Click({ $w.Choice = 'secondary'; $w.Form.Hide() }.GetNewClosure())
    $w
}

# Shows a page and waits for a button; returns 'primary', 'secondary' or 'close'.
function Show-Page($w, [string]$title, [string]$body, [string]$primary, [string]$secondary, [string]$code) {
    $w.Title.Text = $title
    $w.Body.Text = $body
    $w.Code.Text = $code
    $w.Code.Visible = [bool]$code
    $w.Bar.Visible = $false
    $w.Primary.Text = $primary
    $w.Primary.Visible = [bool]$primary
    $w.Secondary.Text = $secondary
    $w.Secondary.Visible = [bool]$secondary
    $w.Choice = 'close'
    [void]$w.Form.ShowDialog()
    $w.Choice
}

# Shows a busy page; returns a scriptblock that updates its status line.
function Show-Busy($w, [string]$title) {
    $w.Title.Text = $title
    $w.Body.Text = ''
    $w.Code.Visible = $false
    $w.Primary.Visible = $false
    $w.Secondary.Visible = $false
    $w.Bar.Visible = $true
    $w.Form.Show()
    { param($msg) $w.Body.Text = $msg; [Windows.Forms.Application]::DoEvents() }.GetNewClosure()
}

function Start-Gui {
    $w = New-Window
    try {
        Test-Preflight
        Initialize-FirmwareAccess
        $state = if ($Uninstall) { $null } else { Get-InstallState }
        $remove = [bool]$Uninstall
        if (-not $remove) {
            $lines = @(
                'Lumen adds a graphical menu that appears when your PC starts, so you can choose between Windows and your other operating systems.',
                '',
                'Your current setup is kept: Windows Boot Manager stays on the PC, and if Lumen ever has a problem the PC simply starts Windows as usual.'
            )
            if ($state.NeedsKey) {
                $lines += ''
                $lines += $(if ($state.SecureBoot) { 'Secure Boot stays on. You will approve Lumen once on the next restart.' }
                            else { 'You will approve Lumen once on the next restart, so it keeps working if you turn on Secure Boot later.' })
            }
            if ($state.BitLocker) { $lines += ''; $lines += 'BitLocker detected: Windows will be started in a way that never asks for your recovery key.' }
            $verb = if ($state.Installed) { 'Update' } else { 'Install' }
            $choice = Show-Page $w "$verb Lumen" ($lines -join "`n") $verb $(if ($state.Installed) { 'Remove' } else { 'Cancel' })
            if ($choice -eq 'secondary' -and $state.Installed) { $remove = $true }
            elseif ($choice -ne 'primary') { return }
        }

        if ($remove) {
            if ((Show-Page $w 'Remove Lumen?' "Lumen will be removed from your PC's boot menu. Windows and your other systems aren't affected." 'Remove' 'Cancel') -ne 'primary') { return }
            $p = Show-Busy $w 'Removing Lumen'
            & $p 'Removing…'
            Invoke-Uninstall
            $w.Form.Hide()
            [void](Show-Page $w 'Lumen removed' 'Your PC will start the way it did before Lumen was installed.' 'Close' '')
            return
        }

        $code = if ($Code) { $Code } else { New-ApprovalCode }
        $p = Show-Busy $w $(if ($state.Installed) { 'Updating Lumen' } else { 'Installing Lumen' })
        Invoke-Install $state $code $p
        $w.Form.Hide()

        if ($state.NeedsKey) {
            $text = "Restart your PC. A blue screen titled `"Shim UEFI key management`" appears once:`n`n" +
                "1.  Press any key`n2.  Choose Enroll MOK, then Continue, then Yes`n3.  Type this code, press Enter, then choose Reboot"
            $choice = Show-Page $w 'One last step' $text 'Restart now' 'Later' $code
        } else {
            $choice = Show-Page $w 'Lumen is installed' "Restart your PC to see Lumen.`n`nFrom then on it appears every time your PC starts, even after Windows updates." 'Restart now' 'Later'
        }
        if ($choice -eq 'primary') { Restart-Computer -Force }
    } catch {
        $err = $_.Exception.Message
        Write-Log "Error: $err"
        $w.Form.Hide()
        $report = try { Save-DiagnosticReport } catch { $null }
        $where = if ($report) { "A diagnostics report was saved as `"$(Split-Path $report -Leaf)`" on the Desktop. Please send it to whoever gave you Lumen." } else { "Details are in $LogFile." }
        if ((Show-Page $w 'Something went wrong' "$err`n`nAny changes were undone, so your PC starts as before.`n`n$where" $(if ($report) { 'Open report' } else { 'Close' }) $(if ($report) { 'Close' } else { '' })) -eq 'primary' -and $report) {
            Start-Process notepad.exe -ArgumentList "`"$report`""
        }
    } finally {
        $w.Form.Dispose()
    }
}

# ---- entry point -------------------------------------------------------------

if ($Diagnose) {
    # Support aid: reports what the installer sees; changes nothing.
    if ($Gui) {
        $w = New-Window
        try {
            $p = Show-Busy $w 'Checking this PC'
            & $p 'Collecting boot and firmware details…'
            $report = Save-DiagnosticReport
            $w.Form.Hide()
            if ((Show-Page $w 'Diagnostics saved' "The report was saved as `"$(Split-Path $report -Leaf)`" on the Desktop. Nothing on this PC was changed.`n`nPlease send it to whoever gave you Lumen." 'Open report' 'Close') -eq 'primary') {
                Start-Process notepad.exe -ArgumentList "`"$report`""
            }
        } catch {
            $w.Form.Hide()
            [void](Show-Page $w 'Diagnostics failed' $_.Exception.Message 'Close' '')
        } finally { $w.Form.Dispose() }
    } else {
        Get-DiagnosticReport
    }
    return
}
if ($Heal) {
    try { [void][LumenFw]::EnablePrivilege(); Invoke-Heal } catch { Write-Log "Heal: $($_.Exception.Message)" }
    return
}
if ($Gui) { Start-Gui; return }

Test-Preflight
Initialize-FirmwareAccess
if ($Uninstall) { Invoke-Uninstall; Write-Output 'Lumen removed. Your PC will start the way it did before.'; return }
$state = Get-InstallState
if (-not $Yes) {
    $verb = if ($state.Installed) { 'Update' } else { 'Install' }
    if ((Read-Host "$verb Lumen? Your current boot setup is kept. [Y/n]") -match '^[nN]') { return }
}
$code = if ($Code) { $Code } else { New-ApprovalCode }
try {
    Invoke-Install $state $code { param($m) Write-Host $m }
} catch {
    $report = try { Save-DiagnosticReport } catch { $null }
    if ($report) { Write-Output "Install failed; diagnostics saved to $report" }
    throw
}
if ($state.NeedsKey) {
    Write-Output ''
    Write-Output "Restart. On the blue `"Shim UEFI key management`" screen: press a key -> Enroll MOK -> Continue -> Yes -> type $code -> Reboot"
} else {
    Write-Output 'Done. Restart your PC to see Lumen.'
}
