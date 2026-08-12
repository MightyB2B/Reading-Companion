<#
.SYNOPSIS
    Stands up a Reading Companion library server from nothing. One script,
    no prerequisites beyond Windows.

.DESCRIPTION
    Copy this file to the server, with reading-server.exe and dict.sqlite
    beside it, and run it from an elevated PowerShell. It will:

      1. install PostgreSQL if it is missing, with a password it generates,
         so there is nothing for you to remember or type;
      2. create the role, the database, and a second generated password;
      3. install the binary and dictionary into their own directory;
      4. write .env beside the binary, readable only by Administrators;
      5. open the port to your subnet, not to the world;
      6. register a service that starts automatically and restarts on failure;
      7. check it actually answers, and print the address to type into the app.

    Every check runs before anything is changed, so a missing prerequisite
    costs a message rather than a half-built server. Safe to re-run: an
    existing database is left alone unless you pass -Fresh, and the service is
    replaced rather than duplicated.

    If reading-server.exe is not beside this script but a checkout is, and
    cargo is installed, it will build it.

    Deliberately ASCII-only: PowerShell 5.1 reads a BOM-less .ps1 as ANSI, and
    a stray smart quote becomes a parse error on someone else's box.

.PARAMETER InstallDir
    Where the server lives. Default C:\ReadingCompanion.

.PARAMETER Port
    The server's port. Default 7878.

.PARAMETER Subnet
    CIDR allowed through the firewall. Detected when omitted. 'none' installs
    without opening the port at all.

.PARAMETER SuperUserPassword
    PostgreSQL's superuser password, if it is already installed and you know
    it. Taken from $env:PGPASSWORD when omitted. Not needed at all when this
    script installs PostgreSQL itself.

.PARAMETER Fresh
    DESTRUCTIVE. Drops the existing database first, discarding every book,
    page, and summary in it.

.PARAMETER Loopback
    Bind to 127.0.0.1 only, for a server reached through a reverse proxy on
    the same machine.

.PARAMETER SkipPostgresInstall
    Never install PostgreSQL, even if it is missing. Fail instead.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\install-server.ps1

.EXAMPLE
    .\install-server.ps1 -Subnet 192.168.4.0/24 -InstallDir D:\ReadingCompanion

.NOTES
    There is no TLS on this port. On a trusted LAN that is a considered
    choice; if you expose it to the internet, put a reverse proxy in front,
    the way scripts/secure-ollama-wan.ps1 does for Ollama.
#>

[CmdletBinding()]
param(
    [string]$InstallDir = 'C:\ReadingCompanion',
    [int]$Port = 7878,
    [string]$Subnet,
    [string]$SuperUserPassword,
    [switch]$Fresh,
    [switch]$Loopback,
    [switch]$SkipPostgresInstall
)

$ErrorActionPreference = 'Stop'

$ServiceName = 'reading-server'
$Database    = 'reading_companion'
$DbUser      = 'reading_app'
$DbPort      = 5432

function Write-Head { param([string]$T) Write-Host "`n=== $T ===" -ForegroundColor Cyan }
function Write-Ok   { param([string]$T) Write-Host "  [ok]   $T" -ForegroundColor Green }
function Write-Bad  { param([string]$T) Write-Host "  [--]   $T" -ForegroundColor Red }
function Write-Hmm  { param([string]$T) Write-Host "  [~~]   $T" -ForegroundColor Yellow }
function Write-Note { param([string]$T) Write-Host "         $T" -ForegroundColor DarkGray }

# A password from the OS CSPRNG.
#
# Alphanumeric without the characters people misread: this ends up inside a
# URL, where + / = @ : would all need percent-encoding, and it will be typed
# by hand at least once. Rejection sampling rather than a modulo fold, which
# would make the first few letters measurably more likely.
function New-Password {
    param([int]$Length = 32)
    $alphabet = 'abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789'
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $chars = New-Object char[] $Length
        $byte = New-Object byte[] 1
        $limit = 256 - (256 % $alphabet.Length)
        for ($i = 0; $i -lt $Length; $i++) {
            do { $rng.GetBytes($byte) } while ($byte[0] -ge $limit)
            $chars[$i] = $alphabet[$byte[0] % $alphabet.Length]
        }
        return -join $chars
    } finally { $rng.Dispose() }
}

function Find-Psql {
    $found = Get-ChildItem 'C:\Program Files\PostgreSQL' -Directory -ErrorAction SilentlyContinue |
        Sort-Object { [int]($_.Name -replace '\D', '0') } -Descending |
        ForEach-Object { Join-Path $_.FullName 'bin\psql.exe' } |
        Where-Object { Test-Path $_ } |
        Select-Object -First 1
    if ($found) { return $found }
    $onPath = Get-Command psql -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    return $null
}

# Restrict a file to Administrators and SYSTEM. Used for anything holding a
# password.
function Protect-File {
    param([string]$Path)
    $acl = Get-Acl $Path
    $acl.SetAccessRuleProtection($true, $false)
    @($acl.Access) | ForEach-Object { $acl.RemoveAccessRule($_) | Out-Null }
    foreach ($who in @('BUILTIN\Administrators', 'NT AUTHORITY\SYSTEM')) {
        $acl.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule(
            $who, 'FullControl', 'Allow')))
    }
    Set-Acl -Path $Path -AclObject $acl
}

$here = $PSScriptRoot
$problems = New-Object System.Collections.Generic.List[string]

# =============================================================================
# 1. Preflight
# =============================================================================

Write-Head "Checking this machine"

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if ($isAdmin) {
    Write-Ok "Running elevated"
} else {
    Write-Bad "Not elevated"
    Write-Note "Installing software, a service, and a firewall rule all need it."
    Write-Note "Right-click PowerShell, choose Run as administrator, try again."
    $problems.Add("not elevated")
}

# --- The binary. Built here if it is missing and that is possible. ---
$binary = Join-Path $here 'reading-server.exe'
if (Test-Path $binary) {
    Write-Ok "reading-server.exe"
} else {
    # A checkout beside or above this script, with cargo available.
    $repo = $null
    foreach ($candidate in @($here, (Split-Path -Parent $here))) {
        if ($candidate -and (Test-Path (Join-Path $candidate 'crates\reading-server\Cargo.toml'))) {
            $repo = $candidate
            break
        }
    }
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue

    if ($repo -and $cargo) {
        Write-Hmm "reading-server.exe is missing; building it from $repo"
        Write-Note "This takes a few minutes the first time."
        Push-Location $repo
        try {
            & cargo build --release -p reading-server
            if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
        } finally { Pop-Location }

        $built = Join-Path $repo 'target\release\reading-server.exe'
        if (Test-Path $built) {
            Copy-Item $built $binary -Force
            Write-Ok "Built and copied here"
        } else {
            Write-Bad "cargo finished but produced no binary"
            $problems.Add("no reading-server.exe")
        }
    } else {
        Write-Bad "reading-server.exe is missing, and it cannot be built here"

        # Which of the two is absent changes the answer entirely, so say so
        # rather than printing one set of instructions for both cases.
        if ($repo) {
            Write-Note "Found a checkout at $repo, but cargo is not installed."
        } else {
            Write-Note "No checkout found here or one directory up."
        }
        if (-not $cargo) {
            Write-Note "No cargo on PATH."
        }

        Write-Note ""
        Write-Note "Easiest: build it on a machine that has Rust and copy two"
        Write-Note "files next to this script:"
        Write-Note "    cargo build --release -p reading-server"
        Write-Note "    npm run dict:build"
        Write-Note "then copy"
        Write-Note "    target\release\reading-server.exe"
        Write-Note "    src-tauri\resources\dict.sqlite"
        Write-Note "to $here"
        Write-Note ""
        Write-Note "Or install Rust here and re-run, and this will build it:"
        Write-Note "    winget install Rustlang.Rustup"
        Write-Note "(that also needs the MSVC C++ build tools, several GB)"

        $problems.Add("no reading-server.exe")
    }
}

# --- The dictionary. Optional, but the application is poorer without it. ---
$dict = Join-Path $here 'dict.sqlite'
if (Test-Path $dict) {
    Write-Ok ("dict.sqlite ({0} MB)" -f [Math]::Round((Get-Item $dict).Length / 1MB, 1))
} else {
    Write-Hmm "dict.sqlite is missing"
    Write-Note "Word lookup will be unavailable and hyphenation falls back to a"
    Write-Note "heuristic. Build it with 'npm run dict:build' and copy"
    Write-Note "src-tauri\resources\dict.sqlite here to have it."
}

# --- The port. ---
$existingService = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
$inUse = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
if ($inUse -and -not $existingService) {
    Write-Bad "Something is already listening on $Port"
    Write-Note "Choose another with -Port, or stop whatever holds it."
    $problems.Add("port $Port in use")
} elseif ($inUse) {
    Write-Ok "Port $Port is this service's already; it will be replaced"
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

# =============================================================================
# 2. PostgreSQL
# =============================================================================

Write-Head "PostgreSQL"

$psql = Find-Psql
$installedByUs = $false
$superPasswordFile = Join-Path $InstallDir 'postgres-superuser-password.txt'

if ($psql) {
    Write-Ok "Already installed ($psql)"
} elseif ($SkipPostgresInstall) {
    Write-Bad "Not installed, and -SkipPostgresInstall was given"
    exit 1
} else {
    $winget = Get-Command winget -ErrorAction SilentlyContinue
    if (-not $winget) {
        Write-Bad "Not installed, and winget is not available to install it"
        Write-Note "Install PostgreSQL by hand from https://www.postgresql.org/download/windows/"
        Write-Note "choosing a superuser password, then re-run with:"
        Write-Note "    `$env:PGPASSWORD = 'that-password'"
        exit 1
    }

    # Generated rather than asked for. A password nobody chose is a password
    # nobody reuses, and it is written to a file only Administrators can read
    # rather than passed on a command line that lands in history.
    $SuperUserPassword = New-Password
    Write-Hmm "Not installed. Installing PostgreSQL 17 -- several minutes."
    Write-Note "A superuser password is being generated; it will be saved for you."

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null

    $override = "--mode unattended --unattendedmodeui none " +
                "--superpassword `"$SuperUserPassword`" --serverport $DbPort"
    & winget install --id PostgreSQL.PostgreSQL.17 --exact --silent `
        --accept-package-agreements --accept-source-agreements `
        --override $override
    if ($LASTEXITCODE -ne 0) {
        Write-Bad "winget could not install PostgreSQL (exit $LASTEXITCODE)"
        Write-Note "Install it by hand, then re-run this script."
        exit 1
    }

    # PATH is not refreshed in this process, so it is found by path.
    $psql = Find-Psql
    if (-not $psql) {
        Write-Bad "PostgreSQL installed but psql.exe cannot be found"
        Write-Note "Open a new PowerShell and run this script again."
        exit 1
    }
    $installedByUs = $true
    Write-Ok "Installed ($psql)"

    Set-Content -Path $superPasswordFile -Value $SuperUserPassword -Encoding ascii
    Protect-File $superPasswordFile
    Write-Ok "Superuser password saved to $superPasswordFile"
}

# The service has to be up before anything can be created in it.
$pgService = Get-Service -Name 'postgresql*' -ErrorAction SilentlyContinue | Select-Object -First 1
if ($pgService -and $pgService.Status -ne 'Running') {
    Write-Hmm "$($pgService.Name) is $($pgService.Status); starting it"
    Start-Service -Name $pgService.Name
    Start-Sleep -Seconds 3
}
if ($pgService) { Write-Ok "$($pgService.Name) is running" }

# Wait for it to actually accept connections, which lags the service state on
# a fresh install.
if ($installedByUs) {
    Write-Host "         Waiting for it to accept connections" -NoNewline -ForegroundColor DarkGray
    $env:PGPASSWORD = $SuperUserPassword
    $ready = $false
    for ($i = 0; $i -lt 60; $i++) {
        & $psql -h localhost -p $DbPort -U postgres -d postgres --no-password -q -c 'SELECT 1;' 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { $ready = $true; break }
        Start-Sleep -Seconds 2
        Write-Host "." -NoNewline -ForegroundColor DarkGray
    }
    Write-Host ""
    if (-not $ready) {
        Write-Bad "PostgreSQL is installed but not accepting connections."
        Write-Note "Its password is in $superPasswordFile"
        exit 1
    }
    Write-Ok "Accepting connections"
}

# --- The superuser password, for the case where we did not install it. ---
if (-not $SuperUserPassword) {
    if ($env:PGPASSWORD) {
        $SuperUserPassword = $env:PGPASSWORD
        Write-Ok "Superuser password taken from the environment"
    } elseif (Test-Path $superPasswordFile) {
        $SuperUserPassword = (Get-Content $superPasswordFile -Raw).Trim()
        Write-Ok "Superuser password read from an earlier run"
    }
}

$envPath = Join-Path $InstallDir '.env'
$haveDatabase = (Test-Path $envPath) -and -not $Fresh

if (-not $haveDatabase -and -not $SuperUserPassword) {
    Write-Bad "PostgreSQL is installed, but its superuser password is unknown"
    Write-Note "Set it and run again:"
    Write-Note "    `$env:PGPASSWORD = 'your-postgres-password'"
    Write-Note "Lost it? Run reset-postgres-password.ps1 from the repository."
    exit 1
}

# =============================================================================
# 3. The database
# =============================================================================

Write-Head "Database"

# The schema is embedded in the server binary and applied on its first run.
# Nothing here creates tables: migrations have one owner, and two things
# applying them means two sets of bookkeeping that disagree.

if ($haveDatabase) {
    Write-Ok "Already set up; leaving it alone"
    Write-Note "Pass -Fresh to drop it. That discards every book in it."
    $DatabaseUrl = (Get-Content $envPath | Where-Object { $_ -match '^DATABASE_URL=' }) -replace "^DATABASE_URL='?", '' -replace "'$", ''
} else {
    $env:PGPASSWORD = $SuperUserPassword

    function Invoke-Super {
        param([string]$Sql, [switch]$AllowFailure)
        $out = & $psql -h localhost -p $DbPort -U postgres -d postgres `
            --no-password -q -t --set ON_ERROR_STOP=1 -c $Sql
        if ($LASTEXITCODE -ne 0 -and -not $AllowFailure) {
            throw "psql failed on: $Sql"
        }
        return $out
    }

    try {
        Invoke-Super "SELECT 1;" | Out-Null
        Write-Ok "Connected as the superuser"
    } catch {
        Write-Bad "Could not connect as postgres. Is the password right?"
        Write-Note "psql said why, just above."
        exit 1
    }

    if ($Fresh) {
        Write-Hmm "Dropping the existing database (-Fresh)"
        Write-Note "Every book, page, and summary in it is being discarded."
        Invoke-Super "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '$Database' AND pid <> pg_backend_pid();" -AllowFailure | Out-Null
        Invoke-Super "DROP DATABASE IF EXISTS ""$Database"";"
        Invoke-Super "DROP ROLE IF EXISTS ""$DbUser"";"
    }

    $DbPassword = New-Password
    $escaped = $DbPassword.Replace("'", "''")

    if ((Invoke-Super "SELECT 1 FROM pg_roles WHERE rolname = '$DbUser';") -match '1') {
        Invoke-Super "ALTER ROLE ""$DbUser"" WITH LOGIN PASSWORD '$escaped';"
        Write-Ok "Role $DbUser existed; its password has been reset"
    } else {
        Invoke-Super "CREATE ROLE ""$DbUser"" WITH LOGIN PASSWORD '$escaped';"
        Write-Ok "Role $DbUser created"
    }

    if ((Invoke-Super "SELECT 1 FROM pg_database WHERE datname = '$Database';") -match '1') {
        Write-Ok "Database $Database already exists"
    } else {
        # CREATE DATABASE cannot run inside a transaction block, hence its own
        # statement rather than part of a script.
        Invoke-Super "CREATE DATABASE ""$Database"" OWNER ""$DbUser"" ENCODING 'UTF8';"
        Write-Ok "Database $Database created, owned by $DbUser"
    }

    # PostgreSQL 15 and later stopped letting everyone write to the public
    # schema, so the owner has to be said explicitly.
    & $psql -h localhost -p $DbPort -U postgres -d $Database --no-password -q --set ON_ERROR_STOP=1 `
        -c "ALTER SCHEMA public OWNER TO ""$DbUser""; GRANT ALL ON SCHEMA public TO ""$DbUser"";"
    if ($LASTEXITCODE -ne 0) {
        Write-Bad "Could not grant the public schema to $DbUser"
        exit 1
    }
    Write-Ok "Granted the public schema"

    $DatabaseUrl = "postgres://${DbUser}:${DbPassword}@localhost:${DbPort}/${Database}"

    # Confirm the role can actually get in, before a service is built on it.
    $env:PGPASSWORD = $DbPassword
    & $psql -h localhost -p $DbPort -U $DbUser -d $Database --no-password -q -c 'SELECT 1;' | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Bad "The new role cannot connect to $Database"
        exit 1
    }
    Write-Ok "$DbUser can connect"
}

Remove-Item Env:PGPASSWORD -ErrorAction SilentlyContinue

# =============================================================================
# 4. Which address to advertise
# =============================================================================

Write-Head "Network"

$bind = if ($Loopback) { "127.0.0.1:$Port" } else { "0.0.0.0:$Port" }

# The adapter carrying the default route. Interface metric alone is not the
# signal it looks like: a Hyper-V or WSL switch routinely outranks the
# physical NIC, and binding the firewall to 172.x would lock out the real
# network.
$lan = $null
$gateway = Get-NetRoute -DestinationPrefix '0.0.0.0/0' -AddressFamily IPv4 -ErrorAction SilentlyContinue |
    Sort-Object RouteMetric, InterfaceMetric | Select-Object -First 1
if ($gateway) {
    $lan = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
        Where-Object { $_.InterfaceIndex -eq $gateway.InterfaceIndex -and $_.IPAddress -ne '127.0.0.1' } |
        Select-Object -First 1
}

if (-not $Subnet -and $lan) {
    $ipBytes = ([System.Net.IPAddress]::Parse($lan.IPAddress)).GetAddressBytes()
    $netBytes = New-Object byte[] 4
    for ($i = 0; $i -lt 4; $i++) {
        $bits = [Math]::Min(8, [Math]::Max(0, $lan.PrefixLength - ($i * 8)))
        $mask = [byte](256 - [Math]::Pow(2, 8 - $bits))
        $netBytes[$i] = [byte]($ipBytes[$i] -band $mask)
    }
    $Subnet = "{0}.{1}.{2}.{3}/{4}" -f $netBytes[0], $netBytes[1], $netBytes[2], $netBytes[3], $lan.PrefixLength
}

if ($lan) {
    Write-Ok "This machine is $($lan.IPAddress) on $($lan.InterfaceAlias)"
    if ($Subnet) { Write-Note "Subnet: $Subnet" }
} else {
    Write-Hmm "No network address found; installing for loopback only"
    $bind = "127.0.0.1:$Port"
}

# =============================================================================
# 5. Install the files
# =============================================================================

Write-Head "Installing into $InstallDir"

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
$libraryDir = Join-Path $InstallDir 'library'
New-Item -ItemType Directory -Path $libraryDir -Force | Out-Null

# Stopped first: a running binary cannot be replaced while it holds itself.
if ($existingService -and $existingService.Status -eq 'Running') {
    Stop-Service -Name $ServiceName -Force
    Start-Sleep -Seconds 2
    Write-Ok "Stopped the running service"
}

Copy-Item $binary (Join-Path $InstallDir 'reading-server.exe') -Force
Write-Ok "reading-server.exe"

$dictPath = Join-Path $InstallDir 'dict.sqlite'
if (Test-Path $dict) {
    Copy-Item $dict $dictPath -Force
    Write-Ok "dict.sqlite"
}

# Beside the binary, not as machine-wide environment variables: this holds the
# database password, and a machine variable would put it where every account
# on the box can read it. A Windows service starts in system32, so beside the
# binary is also the only place it can find configuration at all.
#
# The paths are single-quoted, and that is load-bearing. A bare Windows path
# puts backslashes into a .env value, where the dotenv format reads them as
# escapes; the line then fails to parse -- but only after the lines above it
# have been applied, and the error is discarded. The server would come up with
# DATABASE_URL set and LIBRARY_DIR missing, fall back to a relative path,
# resolve it against system32, and die with "access denied" on a path nobody
# chose. Single quotes are taken literally.
$contents = @"
# Written by install-server.ps1. Not for version control.
#
# Paths are quoted so their backslashes are not read as escapes.
DATABASE_URL='$DatabaseUrl'
BIND_ADDRESS=$bind
LIBRARY_DIR='$libraryDir'
DICT_PATH='$dictPath'
"@

# WriteAllText, not Set-Content -Encoding utf8: on PowerShell 5.1 that writes
# a BOM, which dotenv parsers read as part of the first key's name.
[System.IO.File]::WriteAllText($envPath, $contents, (New-Object System.Text.UTF8Encoding($false)))
Protect-File $envPath
Write-Ok ".env, readable only by Administrators and SYSTEM"

# Read back before a service is built on top of it. The failure this catches
# is silent by nature.
$parsed = @{}
foreach ($line in (Get-Content $envPath)) {
    if ($line -match "^\s*([A-Z_]+)\s*=\s*'?([^']*)'?\s*$") { $parsed[$Matches[1]] = $Matches[2] }
}
foreach ($key in @('DATABASE_URL', 'BIND_ADDRESS', 'LIBRARY_DIR', 'DICT_PATH')) {
    if (-not $parsed.ContainsKey($key) -or -not $parsed[$key]) {
        Write-Bad "The .env just written is missing $key"
        Write-Note "Refusing to install a service that cannot start."
        exit 1
    }
}
Write-Ok "All four settings read back correctly"

# =============================================================================
# 6. Firewall
# =============================================================================

Write-Head "Firewall"

$ruleName = "Reading Companion ($Port)"
$old = Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue
if ($old) { Remove-NetFirewallRule -DisplayName $ruleName }

if ($Loopback -or $Subnet -eq 'none' -or -not $Subnet) {
    Write-Ok "No rule added; the port is not reachable from the network"
} else {
    $rule = @{
        DisplayName   = $ruleName
        Description   = 'Reading Companion library server'
        Direction     = 'Inbound'
        Action        = 'Allow'
        Protocol      = 'TCP'
        LocalPort     = $Port
        RemoteAddress = $Subnet
        Profile       = 'Private,Domain'
    }
    New-NetFirewallRule @rule | Out-Null
    Write-Ok "TCP $Port open to $Subnet on private and domain networks"
    Write-Note "Public networks stay closed."
}

# =============================================================================
# 7. Service
# =============================================================================

Write-Head "Service"

if ($existingService) {
    & sc.exe delete $ServiceName | Out-Null
    Start-Sleep -Seconds 2
    Write-Note "Replaced the existing service."
}

$exePath = Join-Path $InstallDir 'reading-server.exe'
& sc.exe create $ServiceName binPath= "`"$exePath`"" start= auto DisplayName= "Reading Companion library" | Out-Null
if ($LASTEXITCODE -ne 0) {
    Write-Bad "Could not create the service"
    exit 1
}
& sc.exe description $ServiceName "Holds the books, pages, and summaries for Reading Companion" | Out-Null

# Restart after a transient failure -- a database hiccup should not leave the
# library down until someone notices. 5s, then 5s, then every minute.
& sc.exe failure $ServiceName reset= 86400 actions= restart/5000/restart/5000/restart/60000 | Out-Null

Start-Service -Name $ServiceName
Write-Ok "Running, and set to start automatically"

# =============================================================================
# 8. Check it answers
# =============================================================================

Write-Head "Checking it works"

$health = $null
for ($i = 0; $i -lt 30; $i++) {
    try {
        $health = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -TimeoutSec 2
        if ($health.status -eq 'ok') { break }
    } catch { Start-Sleep -Seconds 1 }
}

if (-not $health -or $health.status -ne 'ok') {
    Write-Bad "No answer on 127.0.0.1:$Port"
    Write-Note "The service was installed but did not come up. Look at:"
    Write-Note "    Get-Service $ServiceName"
    Write-Note "    Get-EventLog -LogName Application -Newest 20"
    Write-Note "    & '$exePath'      (run it in the foreground to see why)"
    exit 1
}
Write-Ok "Healthy, version $($health.version)"

$setup = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/setup-state" -TimeoutSec 5
if ($setup.needs_setup) {
    Write-Ok "The library is empty and ready for its first account"
} else {
    Write-Ok "The library already has accounts"
}

# =============================================================================
# 9. Report
# =============================================================================

$address = if ($lan -and -not $Loopback) { "http://$($lan.IPAddress):$Port" } else { "http://127.0.0.1:$Port" }

Write-Host ""
Write-Host "  ------------------------------------------------------------" -ForegroundColor Green
Write-Host "  Done. In the application's sign-in screen:" -ForegroundColor Green
Write-Host ""
Write-Host "      Library server:  $address" -ForegroundColor White
Write-Host ""
if ($setup.needs_setup) {
    Write-Host "  It will offer to create an account. The first one is the" -ForegroundColor Green
    Write-Host "  administrator -- there is no default username or password." -ForegroundColor Green
} else {
    Write-Host "  Sign in with an account you already made." -ForegroundColor Green
}
Write-Host "  ------------------------------------------------------------" -ForegroundColor Green
Write-Host ""
Write-Host "  Service:   Get-Service $ServiceName" -ForegroundColor DarkGray
Write-Host "  Restart:   Restart-Service $ServiceName" -ForegroundColor DarkGray
Write-Host "  Config:    $envPath" -ForegroundColor DarkGray
Write-Host "  Books:     $libraryDir" -ForegroundColor DarkGray
if (Test-Path $superPasswordFile) {
    Write-Host "  Postgres:  $superPasswordFile" -ForegroundColor DarkGray
}
Write-Host ""
Write-Host "  Set the inference server's address in Settings after signing" -ForegroundColor DarkGray
Write-Host "  in. It defaults to this machine's own Ollama." -ForegroundColor DarkGray
Write-Host ""
if (-not $Loopback -and $Subnet -and $Subnet -ne 'none') {
    Write-Host "  There is no TLS on this port. That is fine on a LAN; if you" -ForegroundColor Yellow
    Write-Host "  expose it to the internet, put a reverse proxy in front the" -ForegroundColor Yellow
    Write-Host "  way secure-ollama-wan.ps1 does for Ollama." -ForegroundColor Yellow
    Write-Host ""
}
