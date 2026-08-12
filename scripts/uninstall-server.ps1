<#
.SYNOPSIS
    Removes the Reading Companion library server and everything it created.

.DESCRIPTION
    Finds what is actually installed, tells you what it is about to destroy
    and what that costs in books and pages, and only then asks. Nothing is
    removed until you confirm.

    Removed by default:
      - the reading-server Windows service
      - the install directory: binary, dictionary, .env, certificate, and the
        page photographs under it
      - the firewall rule
      - the reading_companion database and the reading_app role

    NOT removed unless you ask:
      - PostgreSQL itself      (-RemovePostgres)  other things may use it
      - Ollama and its models  (-RemoveOllama)    ~8GB to download again

    Run it with -DryRun first. It will show you the inventory and change
    nothing.

    Deliberately ASCII-only: PowerShell 5.1 reads a BOM-less .ps1 as ANSI,
    and a stray smart quote becomes a parse error on someone else's box.

.PARAMETER InstallDir
    Where the server was installed. Default C:\ReadingCompanion.

.PARAMETER DryRun
    List everything that would be removed, and remove nothing.

.PARAMETER Yes
    Skip the confirmation prompt. For an unattended teardown; think twice.

.PARAMETER Backup
    Write a pg_dump of the library to this path before dropping it. Strongly
    worth doing: it is the only copy of what you have read and written.

.PARAMETER KeepDatabase
    Leave the database and role alone. Removes the service and files only.

.PARAMETER KeepLibrary
    Leave the page photographs on disk.

.PARAMETER RemovePostgres
    Uninstall PostgreSQL entirely. Only if nothing else on this machine uses
    it.

.PARAMETER RemoveOllama
    Uninstall Ollama and delete its models. Several GB to download again.

.EXAMPLE
    .\uninstall-server.ps1 -DryRun

.EXAMPLE
    .\uninstall-server.ps1 -Backup C:\library-backup.sql

.NOTES
    Run from an elevated PowerShell: it removes a service, a firewall rule,
    and files outside your profile.

    This does not touch the desktop application on your reading machine. That
    is an ordinary program; uninstall it from Add or Remove Programs.
#>

[CmdletBinding()]
param(
    [string]$InstallDir = 'C:\ReadingCompanion',
    [switch]$DryRun,
    [switch]$Yes,
    [string]$Backup,
    [switch]$KeepDatabase,
    [switch]$KeepLibrary,
    [switch]$RemovePostgres,
    [switch]$RemoveOllama
)

$ErrorActionPreference = 'Stop'

$ServiceName = 'reading-server'
$Database    = 'reading_companion'
$DbUser      = 'reading_app'

function Write-Head { param([string]$T) Write-Host "`n=== $T ===" -ForegroundColor Cyan }
function Write-Ok   { param([string]$T) Write-Host "  [ok]   $T" -ForegroundColor Green }
function Write-Gone { param([string]$T) Write-Host "  [rm]   $T" -ForegroundColor Magenta }
function Write-Hmm  { param([string]$T) Write-Host "  [~~]   $T" -ForegroundColor Yellow }
function Write-Bad  { param([string]$T) Write-Host "  [--]   $T" -ForegroundColor Red }
function Write-Note { param([string]$T) Write-Host "         $T" -ForegroundColor DarkGray }

function Find-Psql {
    $found = Get-ChildItem 'C:\Program Files\PostgreSQL' -Directory -ErrorAction SilentlyContinue |
        Sort-Object { [int]($_.Name -replace '\D', '0') } -Descending |
        ForEach-Object { Join-Path $_.FullName 'bin\psql.exe' } |
        Where-Object { Test-Path $_ } | Select-Object -First 1
    if ($found) { return $found }
    $onPath = Get-Command psql -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    return $null
}

# --- Elevation ---------------------------------------------------------------

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin -and -not $DryRun) {
    Write-Bad "Not elevated"
    Write-Note "Removing a service and a firewall rule needs it."
    Write-Note "Or run with -DryRun to see what is here without changing anything."
    exit 1
}

# =============================================================================
# What is actually here
# =============================================================================
#
# Discovered rather than assumed, so this works on a half-finished install and
# says honestly what it found.

Write-Head "What is installed"

$found = @{}

$service = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($service) {
    $found.Service = $true
    Write-Ok "Service '$ServiceName' ($($service.Status))"
} else {
    Write-Note "No '$ServiceName' service"
}

# The install directory tells us where everything else lives.
$envPath = Join-Path $InstallDir '.env'
$DatabaseUrl = $null
$libraryDir = Join-Path $InstallDir 'library'

if (Test-Path $InstallDir) {
    $found.InstallDir = $true
    $size = (Get-ChildItem $InstallDir -Recurse -File -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    Write-Ok ("Install directory {0} ({1:N1} MB)" -f $InstallDir, ($size / 1MB))

    if (Test-Path $envPath) {
        $line = Get-Content $envPath | Where-Object { $_ -match '^DATABASE_URL=' }
        if ($line) {
            $DatabaseUrl = ($line -replace "^DATABASE_URL='?", '') -replace "'$", ''
        }
        $libLine = Get-Content $envPath | Where-Object { $_ -match '^LIBRARY_DIR=' }
        if ($libLine) {
            $libraryDir = ($libLine -replace "^LIBRARY_DIR='?", '') -replace "'$", ''
        }
    }
} else {
    Write-Note "No install directory at $InstallDir"
}

# A development checkout keeps its .env at the repository root.
if (-not $DatabaseUrl) {
    foreach ($candidate in @((Split-Path -Parent $PSScriptRoot), $PSScriptRoot)) {
        $devEnv = Join-Path $candidate '.env'
        if ($candidate -and (Test-Path $devEnv)) {
            $line = Get-Content $devEnv | Where-Object { $_ -match '^DATABASE_URL=' }
            if ($line) {
                $DatabaseUrl = ($line -replace "^DATABASE_URL='?", '') -replace "'$", ''
                Write-Hmm "Using DATABASE_URL from $devEnv"
                Write-Note "This looks like a development checkout."
                break
            }
        }
    }
}

if (Test-Path $libraryDir) {
    $pageCount = (Get-ChildItem $libraryDir -Recurse -File -ErrorAction SilentlyContinue).Count
    $pageSize = (Get-ChildItem $libraryDir -Recurse -File -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    $found.Library = $true
    Write-Ok ("Page images: {0} files ({1:N1} MB) in {2}" -f $pageCount, ($pageSize / 1MB), $libraryDir)
}

$rules = @(Get-NetFirewallRule -DisplayName 'Reading Companion (*' -ErrorAction SilentlyContinue)
if ($rules.Count -gt 0) {
    $found.Firewall = $true
    foreach ($r in $rules) { Write-Ok "Firewall rule '$($r.DisplayName)'" }
}

# --- The database, and what it holds ---
#
# Counted rather than described. "3 books, 412 pages, 96 summaries" is the
# only honest way to say what this costs.
$psql = Find-Psql
$dbInfo = $null

if ($DatabaseUrl -and $psql -and -not $KeepDatabase) {
    if ($DatabaseUrl -match '^postgres(ql)?://([^:]+):([^@]+)@([^:/]+):(\d+)/(.+)$') {
        $dbUserName = $Matches[2]; $dbPass = $Matches[3]
        $dbHost = $Matches[4]; $dbPort = $Matches[5]; $dbName = $Matches[6]

        $saved = $env:PGPASSWORD
        try {
            $env:PGPASSWORD = $dbPass
            $counts = & $psql -h $dbHost -p $dbPort -U $dbUserName -d $dbName --no-password -q -t -A -F '|' `
                -c "SELECT (SELECT count(*) FROM users), (SELECT count(*) FROM books), (SELECT count(*) FROM pages), (SELECT count(*) FROM summaries);" 2>$null
            if ($LASTEXITCODE -eq 0 -and $counts) {
                $parts = ($counts | Select-Object -First 1).Split('|')
                $dbInfo = [pscustomobject]@{
                    Users = $parts[0]; Books = $parts[1]; Pages = $parts[2]; Summaries = $parts[3]
                    Host = $dbHost; Port = $dbPort; Name = $dbName
                }
                $found.Database = $true
                Write-Ok "Database '$dbName' on ${dbHost}:$dbPort"
                Write-Note "$($dbInfo.Users) account(s), $($dbInfo.Books) book(s), $($dbInfo.Pages) page(s), $($dbInfo.Summaries) summar(ies)"
            } else {
                Write-Hmm "A DATABASE_URL was found but the database did not answer"
            }
        } finally {
            $env:PGPASSWORD = $saved
        }
    } else {
        Write-Hmm "DATABASE_URL is not in a shape this can parse"
    }
}

if ($RemovePostgres) {
    $pg = Get-Service -Name 'postgresql*' -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($pg) { $found.Postgres = $true; Write-Ok "PostgreSQL service '$($pg.Name)'" }
}
if ($RemoveOllama) {
    $ollama = Get-Command ollama -ErrorAction SilentlyContinue
    if ($ollama) {
        $models = Join-Path $env:USERPROFILE '.ollama\models'
        $modelSize = 0
        if (Test-Path $models) {
            $modelSize = (Get-ChildItem $models -Recurse -File -ErrorAction SilentlyContinue |
                Measure-Object -Property Length -Sum).Sum
        }
        $found.Ollama = $true
        Write-Ok ("Ollama, with {0:N1} GB of models" -f ($modelSize / 1GB))
    }
}

if ($found.Count -eq 0) {
    Write-Host ""
    Write-Host "Nothing to remove." -ForegroundColor Green
    Write-Host ""
    exit 0
}

# =============================================================================
# Confirm
# =============================================================================

Write-Host ""
Write-Host "  ------------------------------------------------------------" -ForegroundColor Yellow
Write-Host "  This will permanently delete:" -ForegroundColor Yellow
Write-Host ""
if ($found.Service)    { Write-Host "    - the $ServiceName service" -ForegroundColor Yellow }
if ($found.InstallDir) { Write-Host "    - $InstallDir and everything in it" -ForegroundColor Yellow }
if ($found.Library -and -not $KeepLibrary) {
                         Write-Host "    - every page photograph in $libraryDir" -ForegroundColor Yellow }
if ($found.Firewall)   { Write-Host "    - the firewall rule" -ForegroundColor Yellow }
if ($found.Database) {
    Write-Host "    - the '$($dbInfo.Name)' database:" -ForegroundColor Red
    Write-Host "        $($dbInfo.Books) book(s), $($dbInfo.Pages) page(s), $($dbInfo.Summaries) summar(ies)" -ForegroundColor Red
    Write-Host "        written by $($dbInfo.Users) account(s)" -ForegroundColor Red
}
if ($found.Postgres)   { Write-Host "    - PostgreSQL itself" -ForegroundColor Red }
if ($found.Ollama)     { Write-Host "    - Ollama and its models" -ForegroundColor Red }
Write-Host ""
if ($found.Database -and -not $Backup) {
    Write-Host "  There is no backup. Summaries are work you did by hand and" -ForegroundColor Red
    Write-Host "  cannot be regenerated. Consider -Backup <path> first." -ForegroundColor Red
    Write-Host ""
}
Write-Host "  ------------------------------------------------------------" -ForegroundColor Yellow
Write-Host ""

# =============================================================================
# Back up first
# =============================================================================
#
# Before the confirmation, and performed even on a dry run. A backup you have
# not seen work is not a backup, and the moment to find out that pg_dump is
# missing is not after the database has gone.

if ($Backup -and $found.Database) {
    Write-Head "Backing up"

    $pgDump = $psql -replace 'psql\.exe$', 'pg_dump.exe'
    if (-not (Test-Path $pgDump)) {
        Write-Bad "pg_dump not found beside psql; cannot back up"
        Write-Note "Stopping rather than going on towards an unbacked-up delete."
        exit 1
    }

    $saved = $env:PGPASSWORD
    try {
        $env:PGPASSWORD = $dbPass
        & $pgDump -h $dbInfo.Host -p $dbInfo.Port -U $dbUserName -d $dbInfo.Name --no-password -f $Backup
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path $Backup)) {
            Write-Bad "The backup failed. Nothing has been removed."
            exit 1
        }
        Write-Ok ("Wrote {0} ({1:N1} MB)" -f $Backup, ((Get-Item $Backup).Length / 1MB))
        Write-Note "Restore with: psql -d <database> -f $Backup"
    } finally {
        $env:PGPASSWORD = $saved
    }
}

if ($DryRun) {
    Write-Host ""
    Write-Host "Dry run. Nothing was removed." -ForegroundColor Green
    if ($Backup -and $found.Database) {
        Write-Host "The backup above is real, so you can check it before committing." -ForegroundColor Green
    }
    Write-Host ""
    exit 0
}

if (-not $Yes) {
    # A typed word rather than y/n. This is not a keystroke to make by
    # accident, and the pause is the point.
    $answer = Read-Host "Type REMOVE to go ahead, anything else to stop"
    if ($answer -cne 'REMOVE') {
        Write-Host ""
        Write-Host "Stopped. Nothing was changed." -ForegroundColor Green
        Write-Host ""
        exit 0
    }
}

# =============================================================================
# Remove
# =============================================================================
#
# Service first: it holds the binary open and the database connection, and
# both of the next two steps fail while it runs.

Write-Head "Removing"

if ($found.Service) {
    if ($service.Status -eq 'Running') {
        Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 2
    }
    & sc.exe delete $ServiceName | Out-Null
    Start-Sleep -Seconds 1
    Write-Gone "Service '$ServiceName'"
}

# Anything still holding the binary, service or not.
Get-Process -Name 'reading-server' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

if ($found.Database) {
    $superPassFile = Join-Path $InstallDir 'postgres-superuser-password.txt'
    $superPass = $env:PGPASSWORD
    if (-not $superPass -and (Test-Path $superPassFile)) {
        $superPass = (Get-Content $superPassFile -Raw).Trim()
    }

    if (-not $superPass) {
        Write-Hmm "No superuser password, so the database cannot be dropped"
        Write-Note "Set `$env:PGPASSWORD and run again, or drop it by hand:"
        Write-Note "    DROP DATABASE $($dbInfo.Name); DROP ROLE $DbUser;"
    } else {
        $saved = $env:PGPASSWORD
        try {
            $env:PGPASSWORD = $superPass
            # Sessions on the database block the drop.
            & $psql -h $dbInfo.Host -p $dbInfo.Port -U postgres -d postgres --no-password -q `
                -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '$($dbInfo.Name)' AND pid <> pg_backend_pid();" 2>$null | Out-Null
            & $psql -h $dbInfo.Host -p $dbInfo.Port -U postgres -d postgres --no-password -q `
                --set ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS ""$($dbInfo.Name)"";"
            if ($LASTEXITCODE -eq 0) {
                Write-Gone "Database '$($dbInfo.Name)' and every table in it"
            } else {
                Write-Bad "Could not drop the database"
            }
            & $psql -h $dbInfo.Host -p $dbInfo.Port -U postgres -d postgres --no-password -q `
                -c "DROP ROLE IF EXISTS ""$DbUser"";" 2>$null | Out-Null
            Write-Gone "Role '$DbUser'"
        } finally {
            $env:PGPASSWORD = $saved
        }
    }
}

if ($found.Firewall) {
    foreach ($r in $rules) {
        Remove-NetFirewallRule -DisplayName $r.DisplayName -ErrorAction SilentlyContinue
        Write-Gone "Firewall rule '$($r.DisplayName)'"
    }
}

if ($found.InstallDir) {
    if ($KeepLibrary -and (Test-Path $libraryDir) -and $libraryDir.StartsWith($InstallDir)) {
        # Move the photographs out before the directory goes.
        $rescued = Join-Path (Split-Path -Parent $InstallDir) 'ReadingCompanion-library'
        Move-Item $libraryDir $rescued -Force -ErrorAction SilentlyContinue
        Write-Ok "Kept the page images, moved to $rescued"
    }
    try {
        Remove-Item $InstallDir -Recurse -Force
        Write-Gone "$InstallDir"
    } catch {
        Write-Bad "Could not remove $InstallDir : $_"
        Write-Note "Something still has a file open. Reboot and delete it by hand."
    }
} elseif ($found.Library -and -not $KeepLibrary) {
    Remove-Item $libraryDir -Recurse -Force -ErrorAction SilentlyContinue
    Write-Gone "$libraryDir"
}

if ($found.Postgres) {
    Write-Hmm "Uninstalling PostgreSQL"
    $winget = Get-Command winget -ErrorAction SilentlyContinue
    if ($winget) {
        & winget uninstall --id PostgreSQL.PostgreSQL.17 --silent --accept-source-agreements
        Write-Gone "PostgreSQL"
        Write-Note "Its data directory may remain; remove it by hand if you want the space."
    } else {
        Write-Bad "winget not available; uninstall it from Add or Remove Programs"
    }
}

if ($found.Ollama) {
    Write-Hmm "Uninstalling Ollama"
    Get-Process -Name 'ollama', 'ollama app' -ErrorAction SilentlyContinue |
        Stop-Process -Force -ErrorAction SilentlyContinue
    $winget = Get-Command winget -ErrorAction SilentlyContinue
    if ($winget) {
        & winget uninstall --id Ollama.Ollama --silent --accept-source-agreements
    }
    $models = Join-Path $env:USERPROFILE '.ollama'
    if (Test-Path $models) {
        Remove-Item $models -Recurse -Force -ErrorAction SilentlyContinue
        Write-Gone "Ollama models"
    }
    # Set by setup-ollama-server.ps1; meaningless once Ollama is gone.
    foreach ($name in @('OLLAMA_HOST', 'OLLAMA_MAX_LOADED_MODELS', 'OLLAMA_KEEP_ALIVE', 'OLLAMA_NUM_PARALLEL')) {
        foreach ($scope in @('User', 'Machine')) {
            if ([Environment]::GetEnvironmentVariable($name, $scope)) {
                [Environment]::SetEnvironmentVariable($name, $null, $scope)
            }
        }
    }
    Write-Gone "Ollama environment variables"
}

# =============================================================================
# What is left
# =============================================================================

Write-Head "Done"

$leftovers = @()
if (-not $RemovePostgres -and (Get-Service -Name 'postgresql*' -ErrorAction SilentlyContinue)) {
    $leftovers += "PostgreSQL is still installed (-RemovePostgres to take it too)"
}
if (-not $RemoveOllama -and (Get-Command ollama -ErrorAction SilentlyContinue)) {
    $leftovers += "Ollama and its models are still here (-RemoveOllama)"
}
if ($Backup -and (Test-Path $Backup)) {
    $leftovers += "Your backup is at $Backup"
}
if ($KeepLibrary) {
    $leftovers += "Page images were kept"
}

if ($leftovers.Count -gt 0) {
    Write-Host ""
    foreach ($l in $leftovers) { Write-Host "  $l" -ForegroundColor DarkGray }
}

Write-Host ""
Write-Host "  The library server is gone from this machine." -ForegroundColor Green
Write-Host "  The desktop application is a separate program; uninstall it" -ForegroundColor DarkGray
Write-Host "  from Add or Remove Programs on the machine you read on." -ForegroundColor DarkGray
Write-Host ""
