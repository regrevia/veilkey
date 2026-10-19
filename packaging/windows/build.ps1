param(
    [string]$Version,
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '..\..\packaging\output'),
    [string]$CompilerPath
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoManifest = Join-Path $repositoryRoot 'Cargo.toml'

if ([string]::IsNullOrWhiteSpace($Version)) {
    $versionMatch = Select-String -Path $cargoManifest -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($null -eq $versionMatch) {
        throw "Could not read the package version from '$cargoManifest'."
    }
    $Version = $versionMatch.Matches.Groups[1].Value
}

$binaryPath = Join-Path $repositoryRoot 'target\x86_64-pc-windows-msvc\release\veilkey.exe'
if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
    throw "Release executable not found at '$binaryPath'. Build the Windows MSVC release target first."
}

if ([string]::IsNullOrWhiteSpace($CompilerPath)) {
    $compilerCandidates = @(
        (Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Inno Setup 6\ISCC.exe'),
        (Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Inno Setup 6\ISCC.exe')
    )
    $CompilerPath = $compilerCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}
if ([string]::IsNullOrWhiteSpace($CompilerPath) -or -not (Test-Path -LiteralPath $CompilerPath -PathType Leaf)) {
    throw 'Inno Setup 6 compiler ISCC.exe was not found. Install Inno Setup 6 or pass -CompilerPath.'
}

if (-not [System.IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path $repositoryRoot $OutputDirectory
}
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path

$installerScript = Join-Path $PSScriptRoot 'veilkey.iss'
$compilerArguments = @(
    '/Qp'
    "/O$OutputDirectory"
    "/DAppVersion=$Version"
    "/DSourceDir=$repositoryRoot"
    "/DBinaryPath=$binaryPath"
    $installerScript
)
& $CompilerPath @compilerArguments
if ($LASTEXITCODE -ne 0) {
    throw "Inno Setup failed with exit code $LASTEXITCODE."
}

$portablePath = Join-Path $OutputDirectory "VeilKey-$Version-windows-x64.exe"
Copy-Item -LiteralPath $binaryPath -Destination $portablePath -Force
$installerPath = Join-Path $OutputDirectory "VeilKey-Setup-$Version-windows-x64.exe"
if (-not (Test-Path -LiteralPath $installerPath -PathType Leaf)) {
    throw "Inno Setup completed without producing '$installerPath'."
}

Get-FileHash -Algorithm SHA256 -LiteralPath $installerPath, $portablePath
