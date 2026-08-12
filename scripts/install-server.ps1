<#
.SYNOPSIS
    One command to stand up a Reading Companion library server.

.DESCRIPTION
    Checks everything first, then does the work by calling the two scripts
    that already know how:

        setup-postgres.ps1    creates the role, the database, and .env
        deploy-server.ps1     installs the files, the firewall rule, the service

    Every check runs before anything is changed, so a missing prerequisite
    costs a message rather than a half-installed server. Safe to re-run: an
    existing database is left alone unless you ask for -Fresh, and the service
    is replaced rather than duplicated.

    Copy these three files to the server and run this one:

        install-server.ps1  setup-postgres.ps1  deploy-server.ps1

    plus the two artefacts they install:

        reading-server.exe    cargo build --release -p reading-server
        dict.sqlite           npm run dict:build

    Deliberately ASCII-only: PowerShell 5.1 reads a BOM-less .ps1 as ANSI,
    and a stray smart quote becomes a parse error on someone else's box.

.PARAMETER SuperUserPassword
    The PostgreSQL superuser password, set when you installed it. Taken from
    $env:PGPASSWORD when omitted, which keeps it out of your shell history.

.PARAMETER InstallDir
    Where the server lives. Default C:\ReadingCompanion.

.PARAMETER Port
    Default 7878.

.PARAMETER Subnet
    CIDR allowed through the firewall. Detected when omitted. 'none' installs
    without opening the port.

.PARAMETER Fresh
    DESTRUCTIVE. Drops the existing database first, discarding every book,
    page, and summary in it.

.PARAMETER Loopback
    Bind to 127.0.0.1 only, for a server reached through a reverse proxy on
    the same machine.

.EXAMPLE
    $env:PGPASSWORD = 'your-postgres-password'
    powershell -ExecutionPolicy Bypass -File .\install-server.ps1

.NOTES
    Run from an elevated PowerShell.
#>

[CmdletBinding()]
param(
    [string]$SuperUserPassword,
    [string]$InstallDir = 'C:\ReadingCompanion',
    [int]$Port = 7878,
    [string]$Subnet,
    [switch]$Fresh,
    [switch]$Loopback
)

$ErrorActionPreference = 'Stop'

function Write-Head { param([string]$T) Write-Host "`n=== $T ===" -ForegroundColor Cyan }
function Write-Ok   { param([string]$T) Write-Host "  [ok]   $T" -ForegroundColor Green }
function Write-Bad  { param([string]$T) Write-Host "  [--]   $T" -ForegroundColor Red }
function Write-Note { param([string]$T) Write-Host "         $T" -ForegroundColor DarkGray }

$problems = New-Object System.Collections.Generic.List[string]

# --- Preflight ---------------------------------------------------------------
#
# Everything is checked before anything is changed. Half-installing and then
# discovering PostgreSQL is missing leaves a mess someone has to unpick.

Write-Head "Checking this machine"

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if ($isAdmin) {
    Write-Ok "Running elevated"
} else {
    Write-Bad "Not elevated"
    Write-Note "Installing a service and changing the firewall both need it."
    Write-Note "Right-click PowerShell, Run as administrator, and try again."
    $problems.Add("not elevated")
}

$here = $PSScriptRoot

# The two scripts this one drives.
foreach ($name in @('setup-postgres.ps1', 'deploy-server.ps1')) {
    if (Test-Path (Join-Path $here $name)) {
        Write-Ok $name
    } else {
        Write-Bad "$name is missing"
        Write-Note "Copy it here from the repository's scripts directory."
        $problems.Add("missing $name")
    }
}

# The artefacts.
$binary = Join-Path $here 'reading-server.exe'
if (Test-Path $binary) {
    Write-Ok "reading-server.exe"
} else {
    Write-Bad "reading-server.exe is missing"
    Write-Note "On your development machine:"
    Write-Note "    cargo build --release -p reading-server"
    Write-Note "then copy target\release\reading-server.exe next to this script."
    $problems.Add("missing reading-server.exe")
}

$dict = Join-Path $here 'dict.sqlite'
if (Test-Path $dict) {
    Write-Ok ("dict.sqlite ({0} MB)" -f [Math]::Round((Get-Item $dict).Length / 1MB, 1))
} else {
    # Not fatal: the application works without it, with less.
    Write-Host "  [~~]   dict.sqlite is missing" -ForegroundColor Yellow
    Write-Note "Word lookup will be unavailable and hyphenation falls back to"
    Write-Note "a heuristic. Build it with 'npm run dict:build' and copy"
    Write-Note "src-tauri\resources\dict.sqlite here to have it."
}

# PostgreSQL.
$psql = Get-ChildItem 'C:\Program Files\PostgreSQL' -Directory -ErrorAction SilentlyContinue |
    Sort-Object { [int]($_.Name -replace '\D', '0') } -Descending |
    ForEach-Object { Join-Path $_.FullName 'bin\psql.exe' } |
    Where-Object { Test-Path $_ } |
    Select-Object -First 1
if (-not $psql) {
    $onPath = Get-Command psql -ErrorAction SilentlyContinue
    if ($onPath) { $psql = $onPath.Source }
}

if ($psql) {
    Write-Ok "PostgreSQL ($psql)"
} else {
    Write-Bad "PostgreSQL is not installed"
    Write-Note "Install it, choosing a superuser password you will remember:"
    Write-Note "    winget install PostgreSQL.PostgreSQL.17"
    Write-Note "Then open a new PowerShell and run this again."
    $problems.Add("no PostgreSQL")
}

$pgService = Get-Service -Name 'postgresql*' -ErrorAction SilentlyContinue | Select-Object -First 1
if ($pgService) {
    if ($pgService.Status -eq 'Running') {
        Write-Ok "$($pgService.Name) is running"
    } else {
        Write-Host "  [~~]   $($pgService.Name) is $($pgService.Status); starting it" -ForegroundColor Yellow
        try {
            Start-Service -Name $pgService.Name
            Start-Sleep -Seconds 3
            Write-Ok "Started"
        } catch {
            Write-Bad "Could not start it: $_"
            $problems.Add("PostgreSQL not running")
        }
    }
} elseif ($psql) {
    Write-Host "  [~~]   No PostgreSQL service found; assuming it runs elsewhere" -ForegroundColor Yellow
}

# The superuser password.
if ($SuperUserPassword) {
    $env:PGPASSWORD = $SuperUserPassword
    Write-Ok "Superuser password supplied"
} elseif ($env:PGPASSWORD) {
    Write-Ok "Superuser password found in the environment"
} else {
    $existingEnv = Join-Path $here '.env'
    if (Test-Path $existingEnv) {
        # A previous run already made the database, so the superuser is not
        # needed again.
        Write-Ok ".env exists from an earlier run; the superuser is not needed"
    } else {
        Write-Bad "No superuser password"
        Write-Note "It is the one you chose when installing PostgreSQL:"
        Write-Note "    `$env:PGPASSWORD = 'your-postgres-password'"
        Write-Note "Lost it? Run reset-postgres-password.ps1."
        $problems.Add("no superuser password")
    }
}

# The port.
$inUse = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
$ours = Get-Service -Name 'reading-server' -ErrorAction SilentlyContinue
if ($inUse -and -not $ours) {
    Write-Bad "Something is already listening on $Port"
    Write-Note "Choose another with -Port, or stop whatever has it."
    $problems.Add("port $Port in use")
} elseif ($inUse) {
    Write-Ok "Port $Port is ours already; the service will be replaced"
} else {
    Write-Ok "Port $Port is free"
}

if ($problems.Count -gt 0) {
    Write-Host ""
    Write-Host "Nothing was changed. Fix these and run it again:" -ForegroundColor Red
    foreach ($p in $problems) { Write-Host "  - $p" -ForegroundColor Red }
    Write-Host ""
    exit 1
}

Write-Host ""
Write-Host "All checks passed." -ForegroundColor Green

# --- The database ------------------------------------------------------------

Write-Head "Database"

$envFile = Join-Path $here '.env'
$needDatabase = $Fresh -or -not (Test-Path $envFile)

if (-not $needDatabase) {
    Write-Host "  .env already exists; leaving the database alone." -ForegroundColor DarkGray
    Write-Note "Pass -Fresh to drop it and start over. That discards every book in it."
} else {
    # Not $args: that is a PowerShell automatic variable, and shadowing it
    # inside a script works until the day something else reads it.
    $pgArgs = @('-File', (Join-Path $here 'setup-postgres.ps1'), '-Port', '5432')
    if ($Fresh) { $pgArgs += '-Force' }

    & powershell -ExecutionPolicy Bypass @pgArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Host ""
        Write-Bad "setup-postgres.ps1 failed. Nothing further was installed."
        exit 1
    }
}

if (-not (Test-Path $envFile)) {
    Write-Bad "No .env was produced, so there is no connection string to deploy with."
    exit 1
}

# --- The server --------------------------------------------------------------

Write-Head "Server"

$deployArgs = @(
    '-File', (Join-Path $here 'deploy-server.ps1'),
    '-InstallDir', $InstallDir,
    '-Port', $Port
)
if ($Subnet) { $deployArgs += @('-Subnet', $Subnet) }
if ($Loopback) { $deployArgs += '-Loopback' }

& powershell -ExecutionPolicy Bypass @deployArgs
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Bad "deploy-server.ps1 failed."
    Write-Note "The database is fine; only the service did not come up."
    Write-Note "Look at: Get-EventLog -LogName Application -Newest 20"
    exit 1
}

# --- Done --------------------------------------------------------------------

Write-Head "Finished"

if ($env:PGPASSWORD) {
    Remove-Item Env:PGPASSWORD -ErrorAction SilentlyContinue
    Write-Note "Cleared the superuser password from this shell."
}

Write-Host ""
Write-Host "  The library server is installed and running." -ForegroundColor Green
Write-Host "  Its address is printed just above; put that into the" -ForegroundColor Green
Write-Host "  application's sign-in screen." -ForegroundColor Green
Write-Host ""
