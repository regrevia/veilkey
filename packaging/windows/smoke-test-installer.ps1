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

Write-Output "Installing VeilKey into '$installDirectory'."
& $InstallerPath '/VERYSILENT' '/SUPPRESSMSGBOXES' '/NORESTART' "/DIR=$installDirectory"
if ($LASTEXITCODE -ne 0) {
    $message = "VeilKey installer failed with exit code $LASTEXITCODE."
    Write-Host "::error::$message"
    throw $message
}

$installedExecutable = Join-Path $installDirectory 'veilkey.exe'
$uninstaller = Join-Path $installDirectory 'unins000.exe'
if (-not (Test-Path -LiteralPath $installedExecutable -PathType Leaf)) {
    $message = 'Installer completed but did not install veilkey.exe.'
    Write-Host "::error::$message"
    throw $message
}
$installedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installedExecutable).Hash
if ($installedHash -ne $ExpectedExecutableSha256) {
    $message = "Installed executable hash mismatch. Expected $ExpectedExecutableSha256, got $installedHash."
    Write-Host "::error::$message"
    throw $message
}
if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
    $message = 'Installer did not create its uninstaller.'
    Write-Host "::error::$message"
    throw $message
}

Write-Output 'Running the generated uninstaller.'
& $uninstaller '/VERYSILENT' '/SUPPRESSMSGBOXES' '/NORESTART'
if ($LASTEXITCODE -ne 0) {
    $message = "VeilKey uninstaller failed with exit code $LASTEXITCODE."
    Write-Host "::error::$message"
    throw $message
}
if (Test-Path -LiteralPath $installedExecutable) {
    $message = 'Uninstaller completed but left veilkey.exe behind.'
    Write-Host "::error::$message"
    throw $message
}

Write-Output "Installer install/uninstall smoke test passed. SHA256=$installedHash"
