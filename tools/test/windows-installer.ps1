# Unit-tests the byte-level parts of install/lumen-windows.ps1 (load options,
# MOK requests) without touching firmware. Run with pwsh on any OS.
param([string]$Out)
$ast = [Management.Automation.Language.Parser]::ParseFile("$PSScriptRoot/../../install/lumen-windows.ps1", [ref]$null, [ref]$null)
foreach ($f in $ast.FindAll({ $args[0] -is [Management.Automation.Language.FunctionDefinitionAst] }, $false)) {
    if ($f.Name -in 'New-LoadOption', 'Get-MokRequest', 'Get-VendorCerts') { Invoke-Expression $f.Extent.Text }
}
function Get-Disk { param($Number) [pscustomobject]@{ LogicalSectorSize = 512 } }
$part = [pscustomobject]@{ DiskNumber = 0; PartitionNumber = 1; Offset = 1048576; Size = 104857600; Guid = '{0f2c4e5a-1b3d-4c6e-8f90-a1b2c3d4e5f6}' }
$opt = New-LoadOption $part '\EFI\lumen\shimx64.efi' 'Lumen'
$lumen = [IO.File]::ReadAllBytes("$PSScriptRoot/../../release/lumen.cer")
$req = Get-MokRequest @(,$lumen) 'hunter2'
# Two keys in one request, the second from a shim's .vendor_cert section
# (Debian's shim, as fetched for the build).
$shim = "$PSScriptRoot/../../vendor/shim/x86_64/shimx64.efi"
$vc = @(Get-VendorCerts ([IO.File]::ReadAllBytes($shim)))
$req2 = Get-MokRequest @($lumen, $vc[0]) 'hunter2'
@{ option = [Convert]::ToHexString($opt); mok_new = [Convert]::ToHexString($req.New); mok_auth = [Convert]::ToHexString($req.Auth)
   vendor_cert = [Convert]::ToHexString($vc[0]); mok_new2 = [Convert]::ToHexString($req2.New) } | ConvertTo-Json | Set-Content $Out
