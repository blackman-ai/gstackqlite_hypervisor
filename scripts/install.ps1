param(
    [string]$Repo = $(if ($env:GSTACKQLITE_HYPERVISOR_REPO) { $env:GSTACKQLITE_HYPERVISOR_REPO } elseif ($env:GSTACK_HYPERVISOR_REPO) { $env:GSTACK_HYPERVISOR_REPO } else { "blackman-ai/gstackqlite_hypervisor" }),
    [string]$Version = $(if ($env:GSTACKQLITE_HYPERVISOR_VERSION) { $env:GSTACKQLITE_HYPERVISOR_VERSION } elseif ($env:GSTACK_HYPERVISOR_VERSION) { $env:GSTACK_HYPERVISOR_VERSION } else { "latest" }),
    [string]$InstallDir = $(if ($env:GSTACKQLITE_HYPERVISOR_INSTALL_DIR) { $env:GSTACKQLITE_HYPERVISOR_INSTALL_DIR } elseif ($env:GSTACK_HYPERVISOR_INSTALL_DIR) { $env:GSTACK_HYPERVISOR_INSTALL_DIR } else { (Join-Path $env:LOCALAPPDATA "Programs/gstackqlite-hypervisor/bin") }),
    [string]$AgentInstall = $(if ($env:GSTACKQLITE_HYPERVISOR_AGENT_INSTALL) { $env:GSTACKQLITE_HYPERVISOR_AGENT_INSTALL } elseif ($env:GSTACK_HYPERVISOR_AGENT_INSTALL) { $env:GSTACK_HYPERVISOR_AGENT_INSTALL } else { "prompt" }),
    [switch]$NoPathUpdate
)

$ErrorActionPreference = "Stop"

$BinaryName = "gstackqlite-hypervisor.exe"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$LocalBinary = Join-Path $ScriptDir $BinaryName

function Command-Exists {
    param([string]$Name)

    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

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

function Resolve-Version {
    if ($Version -ne "latest") {
        return $Version
    }

    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
    if (-not $release.tag_name) {
        throw "Failed to resolve latest release tag for $Repo"
    }
    return [string]$release.tag_name
}

function Add-UserPathSegment {
    param([string]$PathSegment)

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $segments = @()
    if ($userPath) {
        $segments = $userPath -split ';' | Where-Object { $_ -ne "" }
    }
    if ($segments -notcontains $PathSegment) {
        $updated = if ($userPath) { "$userPath;$PathSegment" } else { $PathSegment }
        [Environment]::SetEnvironmentVariable("Path", $updated, "User")
        Write-Host "Added $PathSegment to the user PATH"
    }
}

function Ensure-BunInPath {
    $bunRoot = Join-Path $env:USERPROFILE ".bun"
    $bunBin = Join-Path $bunRoot "bin"
    if ($env:Path -notlike "*$bunBin*") {
        $env:Path = "$bunBin;$env:Path"
    }
    Add-UserPathSegment $bunBin
}

function Install-BunIfNeeded {
    if (Command-Exists "bun") {
        return
    }

    Write-Host "Bun was not found. Installing Bun..."
    $installer = Invoke-RestMethod -Uri "https://bun.sh/install.ps1"
    & ([scriptblock]::Create($installer))
    Ensure-BunInPath

    if (-not (Command-Exists "bun")) {
        throw "Bun installed, but 'bun' is still not available on PATH."
    }
}

function Resolve-AgentSelection {
    $normalized = $AgentInstall.Trim().ToLowerInvariant()
    if ($normalized -and $normalized -ne "prompt") {
        if ($normalized -notin @("claude", "codex", "both", "none")) {
            throw "Unsupported GSTACKQLITE_HYPERVISOR_AGENT_INSTALL value: $AgentInstall"
        }
        return $normalized
    }

    if (-not [Environment]::UserInteractive) {
        Write-Warning "Skipping Claude/Codex bootstrap because the session is not interactive. Set GSTACKQLITE_HYPERVISOR_AGENT_INSTALL to override."
        return "none"
    }

    while ($true) {
        $selection = (Read-Host "Neither Claude nor Codex is installed. Install which agent(s)? [claude/codex/both/none]").Trim().ToLowerInvariant()
        if ($selection -in @("claude", "codex", "both", "none")) {
            return $selection
        }
        Write-Host "Enter one of: claude, codex, both, none."
    }
}

function Install-ClaudeIfNeeded {
    if (Command-Exists "claude") {
        return
    }
    Ensure-BunInPath
    Write-Host "Installing Claude Code with Bun..."
    & bun install --global @anthropic-ai/claude-code
}

function Install-CodexIfNeeded {
    if (Command-Exists "codex") {
        return
    }
    Ensure-BunInPath
    Write-Host "Installing Codex CLI with Bun..."
    & bun install --global @openai/codex
}

function Maybe-InstallAgents {
    if ((Command-Exists "claude") -or (Command-Exists "codex")) {
        return
    }

    $selection = Resolve-AgentSelection
    switch ($selection) {
        "claude" { Install-ClaudeIfNeeded }
        "codex" { Install-CodexIfNeeded }
        "both" {
            Install-ClaudeIfNeeded
            Install-CodexIfNeeded
        }
        "none" {
            Write-Host "Skipping Claude/Codex bootstrap."
        }
    }
}

function Install-Binary {
    param([string]$SourcePath)

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item $SourcePath (Join-Path $InstallDir $BinaryName) -Force

    if (-not $NoPathUpdate) {
        Add-UserPathSegment $InstallDir
    }

    Install-BunIfNeeded
    Maybe-InstallAgents
    Write-Host "Installed $BinaryName to $(Join-Path $InstallDir $BinaryName)"
    Write-Host "Open a new terminal and run 'gstackqlite-hypervisor --help'."
}

if (Test-Path $LocalBinary) {
    Write-Host "Installing gstackqlite-hypervisor from local package..."
    Install-Binary $LocalBinary
    exit 0
}

$target = "x86_64-pc-windows-msvc"
$resolvedVersion = Resolve-Version
$normalizedVersion = $resolvedVersion.TrimStart("v")
$archiveName = "gstackqlite-hypervisor-$normalizedVersion-$target.zip"
$tempDir = New-Item -ItemType Directory -Force -Path (Join-Path ([System.IO.Path]::GetTempPath()) ("gstackqlite-hypervisor-install-" + [guid]::NewGuid().ToString("N")))
$archivePath = Join-Path $tempDir.FullName $archiveName
$checksumsPath = Join-Path $tempDir.FullName "SHA256SUMS"

$releaseUrl = if ($Version -eq "latest") {
    "https://github.com/$Repo/releases/latest/download"
} else {
    "https://github.com/$Repo/releases/download/$resolvedVersion"
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
