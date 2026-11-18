# Installed as a startup task in the private disposable QEMU Windows base image.
#requires -Version 7.0
$ErrorActionPreference = 'Stop'
$serial = [IO.Ports.SerialPort]::new('COM1', 115200)
$serial.Open()
$nonce = 'unavailable'
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(90)
    do {
        $cd = Get-Volume -FileSystemLabel 'FLARETEST' -ErrorAction SilentlyContinue
        if ($cd.DriveLetter) { break }
        Start-Sleep -Seconds 1
    } while ([DateTime]::UtcNow -lt $deadline)
    if (-not $cd.DriveLetter) { throw 'Flare CI test disc missing' }
    $disc = "$($cd.DriveLetter):\"
    $targets = Get-Content -Raw (Join-Path $disc targets.json) | ConvertFrom-Json
    $nonce = $targets.nonce
    $adapter = Get-NetAdapter | Where-Object MacAddress -eq '52-54-00-FA-00-03'
    if (@($adapter).Count -ne 1) { throw 'Missing the QEMU test adapter' }
    $null = Get-Service npcap
    $env:Path = "$env:SystemRoot\System32\Npcap;$env:Path"
    $password = Join-Path $disc password.txt
    & (Join-Path $disc windows-npcap-smoke.ps1) -Executable (Join-Path $disc lflare.exe) `
        -Interface ("\Device\NPF_{" + $adapter.InterfaceGuid.ToString().Trim('{}') + "}") `
        -FingerprintA $targets.a -FingerprintB $targets.b -PasswordFileA $password -PasswordFileB $password |
        ForEach-Object { $serial.WriteLine($_) }
    $serial.WriteLine("DONE $nonce")
} catch {
    $serial.WriteLine("FAIL $nonce $($_.Exception.Message)")
    exit 1
} finally {
    $serial.Close()
}
