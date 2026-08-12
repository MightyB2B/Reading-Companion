<#
.SYNOPSIS
    Resets the PostgreSQL superuser password when it has been lost.

.DESCRIPTION
    The only way in without the password is to tell the server, briefly, not
    to ask for one. That is genuinely dangerous: while it is in that state
    anything on this machine can connect as the superuser. So the whole point
    of this script is that the window is short and cannot be left open --
    pg_hba.conf is restored in a finally block, so it goes back even if the
    password change fails, the service will not start, or you interrupt it.

    It works out the data directory from the running service rather than
    guessing at C:\Program Files, which is the usual reason hand-editing
    pg_hba.conf appears to do nothing: there is more than one copy of that
    file on most machines and only one of them is live.

    Steps:
      1. read the data directory out of the service's own command line
      2. back up pg_hba.conf next to itself, timestamped
      3. set every loopback line to trust -- IPv4 and IPv6 both, which is the
         other common miss, since localhost resolves to ::1 first on Windows
      4. restart, change the password, restore pg_hba.conf, restart again
      5. verify the new password actually works

    Deliberately ASCII-only: PowerShell 5.1 reads a BOM-less .ps1 as ANSI,
    and a stray smart quote becomes a parse error on someone else's box.

.PARAMETER NewPassword
    The password to set. One is generated if omitted.

.PARAMETER SuperUser
    Default postgres.

.PARAMETER ServiceName
    Windows service to operate on. Detected when omitted.

.PARAMETER Port
    Default 5432.

.EXAMPLE
    .\scripts\reset-postgres-password.ps1

.NOTES
    Run from an elevated PowerShell: it stops and starts a service and writes
    inside the PostgreSQL data directory.

    Nothing in your databases is touched. This changes one role's password.
#>

[CmdletBinding()]
param(
    [string]$NewPassword,
    [string]$SuperUser = 'postgres',
    [string]$ServiceName,
    [int]$Port = 5432
)

$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$Text) Write-Host "`n==> $Text" -ForegroundColor Cyan }
function Write-Ok   { param([string]$Text) Write-Host "    $Text" -ForegroundColor Green }
function Write-Warn { param([string]$Text) Write-Host "    $Text" -ForegroundColor Yellow }

# --- 0. Preconditions --------------------------------------------------------

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not $isAdmin) {
    throw "This stops a service and writes into the data directory. Re-run from an elevated PowerShell."
}

Write-Step "Finding the server"

if ($ServiceName) {
    $service = Get-CimInstance Win32_Service -Filter "Name = '$ServiceName'"
    if (-not $service) { throw "No service named '$ServiceName'." }
} else {
    $candidates = @(Get-CimInstance Win32_Service |
        Where-Object { $_.Name -like 'postgresql*' })

    if ($candidates.Count -eq 0) {
        throw "No PostgreSQL service found. Is it installed?"
    }
    if ($candidates.Count -gt 1) {
        Write-Warn "More than one PostgreSQL service:"
        $candidates | ForEach-Object { Write-Host "      $($_.Name)" -ForegroundColor Gray }
        Write-Warn "Pick one with -ServiceName."
        throw "Ambiguous service."
    }
    $service = $candidates[0]
}

Write-Ok "Service: $($service.Name)"

# The service command line carries -D <datadir>. That is the authoritative
# answer; anything else is a guess.
if ($service.PathName -notmatch '-D\s+"([^"]+)"' -and $service.PathName -notmatch '-D\s+(\S+)') {
    throw "Could not read the data directory from: $($service.PathName)"
}
$dataDir = $Matches[1]

$hba = Join-Path $dataDir 'pg_hba.conf'
if (-not (Test-Path $hba)) {
    throw "No pg_hba.conf at $hba"
}

Write-Ok "Data directory: $dataDir"
Write-Host "    This is the pg_hba.conf that counts. If you edited another, that is why" -ForegroundColor DarkGray
Write-Host "    nothing changed." -ForegroundColor DarkGray

# psql lives beside pg_ctl, which is what the service runs.
$binDir = Split-Path -Parent ($service.PathName -replace '^"([^"]+)".*$', '$1')
$psql = Join-Path $binDir 'psql.exe'
if (-not (Test-Path $psql)) {
    $onPath = Get-Command psql -ErrorAction SilentlyContinue
    if (-not $onPath) { throw "Could not find psql.exe near $binDir" }
    $psql = $onPath.Source
}

# --- 1. Password -------------------------------------------------------------

function New-Password {
    param([int]$Length = 32)
    $alphabet = 'abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789'
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $chars = New-Object char[] $Length
        $byte = New-Object byte[] 1
        # Rejection sampling rather than a modulo fold, which would make the
        # first few letters of the alphabet measurably more likely.
        $limit = 256 - (256 % $alphabet.Length)
        for ($i = 0; $i -lt $Length; $i++) {
            do { $rng.GetBytes($byte) } while ($byte[0] -ge $limit)
            $chars[$i] = $alphabet[$byte[0] % $alphabet.Length]
        }
        return -join $chars
    } finally {
        $rng.Dispose()
    }
}

$generated = $false
if (-not $NewPassword) {
    $NewPassword = New-Password
    $generated = $true
}
$escaped = $NewPassword.Replace("'", "''")

# --- 2. Back up, then open the window ---------------------------------------

Write-Step "Backing up pg_hba.conf"

$backup = "$hba.backup-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
Copy-Item -Path $hba -Destination $backup -Force
Write-Ok $backup

$original = Get-Content -Path $hba -Raw

$restored = $false
try {
    Write-Step "Trusting loopback connections, briefly"

    $patched = New-Object System.Collections.Generic.List[string]
    $changed = 0

    foreach ($line in (Get-Content -Path $hba)) {
        # Comments and blanks pass through untouched.
        if ($line -match '^\s*(#|$)') {
            $patched.Add($line)
            continue
        }

        # host TYPE DATABASE USER ADDRESS METHOD  -- six fields, method last.
        # Only loopback is touched: a rule allowing the whole LAN in without a
        # password, even for a minute, is a different level of exposure.
        $fields = $line -split '\s+' | Where-Object { $_ -ne '' }

        $isLocal = $fields[0] -eq 'local'
        $isLoopback = $fields[0] -match '^host' -and
                      $fields.Count -ge 5 -and
                      ($fields[3] -eq '127.0.0.1/32' -or $fields[3] -eq '::1/128' -or
                       $fields[4] -eq '127.0.0.1/32' -or $fields[4] -eq '::1/128')

        if ($isLocal -or $isLoopback) {
            # Replace the final field, whatever it is.
            $patched.Add(($line -replace '(\S+)(\s*)$', 'trust$2'))
            $changed++
        } else {
            $patched.Add($line)
        }
    }

    if ($changed -eq 0) {
        throw "Found no loopback rules to change in $hba. Nothing was modified."
    }

    Set-Content -Path $hba -Value $patched -Encoding ascii
    Write-Ok "$changed rule(s) set to trust (IPv4 and IPv6)."

    Restart-Service -Name $service.Name -Force
    Write-Ok "Restarted."

    Write-Host "    Waiting for the server..." -NoNewline
    $up = $false
    for ($i = 0; $i -lt 30; $i++) {
        # No PGPASSWORD: trust means it must not be asked for. If this still
        # fails, the config did not take and the password is not the problem.
        $env:PGPASSWORD = ''
        & $psql -h 127.0.0.1 -p $Port -U $SuperUser -d postgres -q -c "SELECT 1;" | Out-Null
        if ($LASTEXITCODE -eq 0) { $up = $true; break }
        Start-Sleep -Seconds 1
        Write-Host "." -NoNewline
    }
    Write-Host ""

    if (-not $up) {
        throw "Still could not connect with trust enabled. Something else is wrong; pg_hba.conf is being restored."
    }
    Write-Ok "Connected without a password."

    Write-Step "Setting the password"
    & $psql -h 127.0.0.1 -p $Port -U $SuperUser -d postgres -q `
        -c "ALTER USER ""$SuperUser"" WITH PASSWORD '$escaped';"
    if ($LASTEXITCODE -ne 0) {
        throw "ALTER USER failed."
    }
    Write-Ok "Changed."
} finally {
    # The whole reason this script exists. Restoring here means the server
    # cannot be left trusting anything, whatever went wrong above.
    Write-Step "Restoring pg_hba.conf"
    Set-Content -Path $hba -Value $original -NoNewline -Encoding ascii
    $restored = $true
    Restart-Service -Name $service.Name -Force
    Write-Ok "Restored from memory and restarted. Backup kept at:"
    Write-Host "    $backup" -ForegroundColor DarkGray
}

# --- 3. Verify ---------------------------------------------------------------

Write-Step "Checking the new password"

$env:PGPASSWORD = $NewPassword
$ok = $false
for ($i = 0; $i -lt 20; $i++) {
    & $psql -h 127.0.0.1 -p $Port -U $SuperUser -d postgres -q --no-password -c "SELECT 1;" | Out-Null
    if ($LASTEXITCODE -eq 0) { $ok = $true; break }
    Start-Sleep -Seconds 1
}

if (-not $ok) {
    Write-Warn "The new password did not work. Restore $backup by hand if needed."
    throw "Verification failed."
}
Write-Ok "Authenticated as $SuperUser."

# --- 4. Report ---------------------------------------------------------------

Write-Host ""
Write-Host "  ------------------------------------------------------------" -ForegroundColor Green
Write-Host "  $SuperUser password:" -ForegroundColor Green
Write-Host ""
Write-Host "      $NewPassword" -ForegroundColor White
Write-Host ""
Write-Host "  ------------------------------------------------------------" -ForegroundColor Green
Write-Host ""

if ($generated) {
    Write-Host "  Printed once. Put it in a password manager now." -ForegroundColor Yellow
    Write-Host ""
}

Write-Host "  It is already set in this shell, so you can go straight on:" -ForegroundColor DarkGray
Write-Host "      .\scripts\setup-postgres.ps1" -ForegroundColor Gray
Write-Host ""
Write-Host "  Clear it when you are done:" -ForegroundColor DarkGray
Write-Host "      Remove-Item Env:PGPASSWORD" -ForegroundColor DarkGray
Write-Host ""
