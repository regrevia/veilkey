param(
    [Parameter(Mandatory)]
    [string]$InstallerPath,
    [Parameter(Mandatory)]
    [string]$ExpectedExecutableSha256
)

$ErrorActionPreference = 'Stop'
$InstallerPath = (Resolve-Path -LiteralPath $InstallerPath -PathType Leaf).Path
$temporaryRoot = if ([string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
    [System.IO.Path]::GetTempPath()
} else {
    $env:RUNNER_TEMP
}
$installDirectory = Join-Path $temporaryRoot "VeilKey-installer-smoke-$([guid]::NewGuid().ToString('N'))"

& $InstallerPath '/VERYSILENT' '/SUPPRESSMSGBOXES' '/NORESTART' '/TASKS=' "/DIR=$installDirectory"
if ($LASTEXITCODE -ne 0) {
    throw "VeilKey installer failed with exit code $LASTEXITCODE."
}

$installedExecutable = Join-Path $installDirectory 'veilkey.exe'
$uninstaller = Join-Path $installDirectory 'unins000.exe'
if (-not (Test-Path -LiteralPath $installedExecutable -PathType Leaf)) {
    throw 'Installer completed but did not install veilkey.exe.'
}
$installedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installedExecutable).Hash
if ($installedHash -ne $ExpectedExecutableSha256) {
    throw "Installed executable hash mismatch. Expected $ExpectedExecutableSha256, got $installedHash."
}
if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
    throw 'Installer did not create its uninstaller.'
}

& $uninstaller '/VERYSILENT' '/SUPPRESSMSGBOXES' '/NORESTART'
if ($LASTEXITCODE -ne 0) {
    throw "VeilKey uninstaller failed with exit code $LASTEXITCODE."
}
if (Test-Path -LiteralPath $installedExecutable) {
    throw 'Uninstaller completed but left veilkey.exe behind.'
}

Write-Output "Installer install/uninstall smoke test passed. SHA256=$installedHash"
