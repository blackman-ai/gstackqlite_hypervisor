param(
    [string]$Repo = $(if ($env:GSTACKQLITE_HYPERVISOR_REPO) { $env:GSTACKQLITE_HYPERVISOR_REPO } elseif ($env:GSTACK_HYPERVISOR_REPO) { $env:GSTACK_HYPERVISOR_REPO } else { "blackman-ai/gstackqlite_hypervisor" }),
    [string]$Version = $(if ($env:GSTACKQLITE_HYPERVISOR_VERSION) { $env:GSTACKQLITE_HYPERVISOR_VERSION } elseif ($env:GSTACK_HYPERVISOR_VERSION) { $env:GSTACK_HYPERVISOR_VERSION } else { "latest" }),
    [string]$InstallDir = $(if ($env:GSTACKQLITE_HYPERVISOR_INSTALL_DIR) { $env:GSTACKQLITE_HYPERVISOR_INSTALL_DIR } elseif ($env:GSTACK_HYPERVISOR_INSTALL_DIR) { $env:GSTACK_HYPERVISOR_INSTALL_DIR } else { (Join-Path $env:LOCALAPPDATA "Programs/gstackqlite-hypervisor/bin") }),
    [switch]$NoPathUpdate
)

$ErrorActionPreference = "Stop"

$BinaryName = "gstackqlite-hypervisor.exe"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$LocalBinary = Join-Path $ScriptDir $BinaryName

function Get-ChecksumValue {
    param(
        [string]$ChecksumsPath,
        [string]$FileName
    )

    foreach ($line in Get-Content $ChecksumsPath) {
        $parts = $line -split '\s+', 2
        if ($parts.Length -eq 2 -and $parts[1].Trim() -eq $FileName) {
            return $parts[0].Trim()
        }
    }

    throw "Checksum entry not found for $FileName"
}

function Install-Binary {
    param([string]$SourcePath)

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item $SourcePath (Join-Path $InstallDir $BinaryName) -Force

    if (-not $NoPathUpdate) {
        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        $segments = @()
        if ($userPath) {
            $segments = $userPath -split ';' | Where-Object { $_ -ne "" }
        }
        if ($segments -notcontains $InstallDir) {
            $updated = if ($userPath) { "$userPath;$InstallDir" } else { $InstallDir }
            [Environment]::SetEnvironmentVariable("Path", $updated, "User")
            Write-Host "Added $InstallDir to the user PATH"
        }
    }

    Write-Host "Installed $BinaryName to $(Join-Path $InstallDir $BinaryName)"
    Write-Host "Open a new terminal and run 'gstackqlite-hypervisor --help'."
}

if (Test-Path $LocalBinary) {
    Write-Host "Installing gstackqlite-hypervisor from local package..."
    Install-Binary $LocalBinary
    exit 0
}

$target = "x86_64-pc-windows-msvc"
$normalizedVersion = if ($Version -eq "latest") { "latest" } else { $Version.TrimStart("v") }
$archiveName = "gstackqlite-hypervisor-$normalizedVersion-$target.zip"
$tempDir = New-Item -ItemType Directory -Force -Path (Join-Path ([System.IO.Path]::GetTempPath()) ("gstackqlite-hypervisor-install-" + [guid]::NewGuid().ToString("N")))
$archivePath = Join-Path $tempDir.FullName $archiveName
$checksumsPath = Join-Path $tempDir.FullName "SHA256SUMS"

$releaseUrl = if ($Version -eq "latest") {
    "https://github.com/$Repo/releases/latest/download"
} else {
    "https://github.com/$Repo/releases/download/$Version"
}

Write-Host "Downloading $archiveName from $Repo..."
Invoke-WebRequest -Uri "$releaseUrl/$archiveName" -OutFile $archivePath
Invoke-WebRequest -Uri "$releaseUrl/SHA256SUMS" -OutFile $checksumsPath

$expected = Get-ChecksumValue -ChecksumsPath $checksumsPath -FileName $archiveName
$actual = (Get-FileHash -Algorithm SHA256 $archivePath).Hash.ToLowerInvariant()
if ($actual -ne $expected.ToLowerInvariant()) {
    throw "Checksum verification failed for $archiveName"
}

Expand-Archive -Path $archivePath -DestinationPath $tempDir.FullName -Force
Install-Binary (Join-Path $tempDir.FullName "gstackqlite-hypervisor-$normalizedVersion-$target/$BinaryName")
