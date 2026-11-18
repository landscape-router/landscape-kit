#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Interface,
    [Parameter(Mandatory)][ValidatePattern('^[a-f0-9]{64}$')][string]$FingerprintA,
    [Parameter(Mandatory)][ValidatePattern('^[a-f0-9]{64}$')][string]$FingerprintB,
    [Parameter(Mandatory)][string]$PasswordFileA,
    [Parameter(Mandatory)][string]$PasswordFileB,
    [string]$Executable = '.\lflare.exe',
    [ValidateRange(1,65535)][int]$RemotePort = 6443
)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'This smoke test requires Windows and Npcap.' }
if ($FingerprintA -eq $FingerprintB) { throw 'Supply two distinct router fingerprints.' }
$binary = (Resolve-Path -LiteralPath $Executable).Path
$devices = & $binary discover --dev $Interface --json | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "Discovery failed with exit code $LASTEXITCODE" }
foreach ($id in @($FingerprintA, $FingerprintB)) {
    if (@($devices | Where-Object fingerprint -eq $id).Count -ne 1) {
        throw "Missing or ambiguous target $id"
    }
}

function Test-FlareMapping([string]$Fingerprint, [string]$PasswordFile, [string]$Expected) {
    $passwordPath = (Resolve-Path -LiteralPath $PasswordFile).Path
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try { $port = $listener.LocalEndpoint.Port } finally { $listener.Stop() }
    $start = [Diagnostics.ProcessStartInfo]::new($binary)
    $start.UseShellExecute = $false
    $start.RedirectStandardError = $true
    $start.RedirectStandardOutput = $true
    foreach ($arg in @('cli', '--dev', $Interface, '--target', $Fingerprint, '--auth', 'root',
                        '--password-file', $passwordPath, '--forward', "${port}:$RemotePort")) {
        $start.ArgumentList.Add($arg)
    }
    $handler = [Net.Http.HttpClientHandler]::new()
    $handler.UseProxy = $false
    $http = [Net.Http.HttpClient]::new($handler)
    $http.Timeout = [TimeSpan]::FromSeconds(2)
    $client = $null
    try {
        $client = [Diagnostics.Process]::Start($start)
        $errors = $client.StandardError.ReadToEndAsync()
        $stdout = $client.StandardOutput.ReadToEndAsync()
        $deadline = [DateTime]::UtcNow.AddSeconds(35)
        while ([DateTime]::UtcNow -lt $deadline) {
            if ($client.HasExited) { throw "lflare exited: $($errors.Result)" }
            try {
                $body = $http.GetStringAsync("http://127.0.0.1:$port/").GetAwaiter().GetResult()
            } catch [Net.Http.HttpRequestException], [OperationCanceledException] {
                Start-Sleep -Milliseconds 250
                continue
            }
            if ($body.Trim() -cne $Expected) { throw "Wrong HTTP marker for target $Fingerprint" }
            Write-Output "PASS $Fingerprint returned HTTP marker $Expected"
            return
        }
        throw "Timed out waiting for marker $Expected"
    } finally {
        if ($client) {
            if (-not $client.HasExited) { $client.Kill() }
            $client.WaitForExit()
            $client.Dispose()
        }
        $http.Dispose()
    }
}

Test-FlareMapping $FingerprintA $PasswordFileA 'A'
Test-FlareMapping $FingerprintB $PasswordFileB 'B'
