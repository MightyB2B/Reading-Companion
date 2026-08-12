<#
.SYNOPSIS
    Installs reading-server on this machine as a Windows service.

.DESCRIPTION
    Run this ON the server, after copying two files next to this script:

        reading-server.exe    built with: cargo build --release -p reading-server
        dict.sqlite           built with: npm run dict:build

    No Rust toolchain is needed here. The schema is embedded in the binary
    and applied on first run, so no repository is needed either.

    It:
      1. checks the database is reachable with the credentials in .env;
      2. installs the files into a directory of their own;
      3. writes .env beside the binary, readable only by Administrators;
      4. opens the port to your subnet, not to the world;
      5. registers a service set to start automatically, and starts it.

    Deliberately ASCII-only: PowerShell 5.1 reads a BOM-less .ps1 as ANSI,
    and a stray smart quote becomes a parse error on someone else's box.

.PARAMETER InstallDir
    Where the server lives. Default C:\ReadingCompanion.

.PARAMETER Port
    Default 7878.

.PARAMETER Subnet
    CIDR allowed through the firewall. Detected from this machine when
    omitted. Pass 'none' to install without opening the port at all.

.PARAMETER DatabaseUrl
    Connection string. Read from .env beside this script when omitted.

.PARAMETER Loopback
    Bind to 127.0.0.1 instead of every interface. For a server you intend to
    reach only through a reverse proxy on the same machine.

.EXAMPLE
    # After running setup-postgres.ps1, which writes .env
    .\deploy-server.ps1

.EXAMPLE
    .\deploy-server.ps1 -Subnet 192.168.4.0/24 -Port 7878

.NOTES
    Run from an elevated PowerShell: it writes outside your profile, changes
    the firewall, and installs a service.

    There is no TLS here. On a trusted LAN that is a considered choice; if
    this is reachable from the internet, put a reverse proxy in front of it
    the way scripts/secure-ollama-wan.ps1 does for Ollama.
#>

[CmdletBinding()]
param(
    [string]$InstallDir = 'C:\ReadingCompanion',
    [int]$Port = 7878,
    [string]$Subnet,
    [string]$DatabaseUrl,
    [switch]$Loopback
)

$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$Text) Write-Host "`n==> $Text" -ForegroundColor Cyan }
function Write-Ok   { param([string]$Text) Write-Host "    $Text" -ForegroundColor Green }
function Write-Warn { param([string]$Text) Write-Host "    $Text" -ForegroundColor Yellow }

$ServiceName = 'reading-server'

# --- 0. Preconditions --------------------------------------------------------

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not $isAdmin) {
    throw "This installs a service and changes the firewall. Re-run from an elevated PowerShell."
}

Write-Step "Looking for what to install"

$here = $PSScriptRoot
$binary = Join-Path $here 'reading-server.exe'
$dict = Join-Path $here 'dict.sqlite'

if (-not (Test-Path $binary)) {
    Write-Host ""
    Write-Warn "reading-server.exe is not beside this script."
    Write-Host ""
    Write-Host "    On your development machine:" -ForegroundColor DarkGray
    Write-Host "      cargo build --release -p reading-server" -ForegroundColor Gray
    Write-Host "      npm run dict:build" -ForegroundColor Gray
    Write-Host ""
    Write-Host "    Then copy these next to this script:" -ForegroundColor DarkGray
    Write-Host "      target\release\reading-server.exe" -ForegroundColor Gray
    Write-Host "      src-tauri\resources\dict.sqlite" -ForegroundColor Gray
    Write-Host ""
    throw "reading-server.exe not found."
}
Write-Ok "reading-server.exe"

if (Test-Path $dict) {
    $mb = [Math]::Round((Get-Item $dict).Length / 1MB, 1)
    Write-Ok "dict.sqlite ($mb MB)"
} else {
    Write-Warn "dict.sqlite is missing. The server will start, but word lookup"
    Write-Warn "will be unavailable and hyphenation falls back to a heuristic."
}

# --- 1. The database ---------------------------------------------------------

Write-Step "Database"

if (-not $DatabaseUrl) {
    $envFile = Join-Path $here '.env'
    if (Test-Path $envFile) {
        $DatabaseUrl = (Get-Content $envFile | Where-Object { $_ -match '^DATABASE_URL=' }) -replace '^DATABASE_URL=', ''
    }
}

if (-not $DatabaseUrl) {
    Write-Host ""
    Write-Warn "No DATABASE_URL. Create the database first:"
    Write-Host ""
    Write-Host "      `$env:PGPASSWORD = 'your-postgres-password'" -ForegroundColor Gray
    Write-Host "      .\setup-postgres.ps1" -ForegroundColor Gray
    Write-Host ""
    Write-Host "    That writes .env, which this script then reads. Or pass" -ForegroundColor DarkGray
    Write-Host "    -DatabaseUrl yourself." -ForegroundColor DarkGray
    Write-Host ""
    throw "DATABASE_URL not set."
}

if ($DatabaseUrl -notmatch '^postgres(ql)?://([^:]+):([^@]+)@([^:/]+):(\d+)/(.+)$') {
    throw "DATABASE_URL is not in the form postgres://user:password@host:port/database"
}
$dbUser = $Matches[2]; $dbPass = $Matches[3]
$dbHost = $Matches[4]; $dbPort = $Matches[5]; $dbName = $Matches[6]

Write-Host "    $dbUser@${dbHost}:$dbPort/$dbName" -ForegroundColor DarkGray

# Checked now, because a service that cannot reach its database fails in the
# event log rather than in front of you.
$psql = Get-ChildItem 'C:\Program Files\PostgreSQL' -Directory -ErrorAction SilentlyContinue |
    Sort-Object { [int]($_.Name -replace '\D', '0') } -Descending |
    ForEach-Object { Join-Path $_.FullName 'bin\psql.exe' } |
    Where-Object { Test-Path $_ } |
    Select-Object -First 1

if ($psql) {
    $saved = $env:PGPASSWORD
    try {
        $env:PGPASSWORD = $dbPass
        & $psql -h $dbHost -p $dbPort -U $dbUser -d $dbName --no-password -q -c 'SELECT 1;' | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "Could not connect to the database with the credentials in DATABASE_URL."
        }
        Write-Ok "Reachable."
    } finally {
        $env:PGPASSWORD = $saved
    }
} else {
    Write-Warn "psql not found, so the connection was not tested."
}

# --- 2. Which address to advertise ------------------------------------------

Write-Step "Network"

$bind = "0.0.0.0:$Port"
if ($Loopback) {
    $bind = "127.0.0.1:$Port"
    Write-Ok "Binding loopback only (-Loopback)."
}

# The adapter carrying the default route, for the same reason as the Ollama
# script: metric alone picks WSL and Hyper-V switches.
$lan = $null
$gateway = Get-NetRoute -DestinationPrefix '0.0.0.0/0' -AddressFamily IPv4 -ErrorAction SilentlyContinue |
    Sort-Object RouteMetric, InterfaceMetric |
    Select-Object -First 1
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
}

# --- 3. Install the files ----------------------------------------------------

Write-Step "Installing into $InstallDir"

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
$libraryDir = Join-Path $InstallDir 'library'
New-Item -ItemType Directory -Path $libraryDir -Force | Out-Null

# Stopped first: the running binary cannot be replaced while it holds itself.
$existing = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($existing -and $existing.Status -eq 'Running') {
    Stop-Service -Name $ServiceName -Force
    Start-Sleep -Seconds 2
    Write-Ok "Stopped the running service."
}

Copy-Item -Path $binary -Destination (Join-Path $InstallDir 'reading-server.exe') -Force
Write-Ok "reading-server.exe"

if (Test-Path $dict) {
    Copy-Item -Path $dict -Destination (Join-Path $InstallDir 'dict.sqlite') -Force
    Write-Ok "dict.sqlite"
}

# Beside the binary rather than as machine-wide environment variables: this
# file holds the database password, and a machine variable would put it where
# every account on the box can read it.
$envPath = Join-Path $InstallDir '.env'

# Paths are single-quoted, and that is load-bearing rather than tidiness.
#
# A bare Windows path puts backslashes into a .env value, where the dotenv
# format reads them as escape sequences. The parser then fails on that line --
# but only *after* setting the variables above it, and the error is discarded
# by every caller. The server comes up with DATABASE_URL set and LIBRARY_DIR
# missing, falls back to a relative "library", resolves it against the service
# working directory of system32, and dies with "access denied" on a path
# nobody chose. Single quotes are taken literally, so none of that happens.
#
# A value containing a single quote would break this in turn; generated
# passwords are alphanumeric, so it cannot arise from this script.
$dictPath = Join-Path $InstallDir 'dict.sqlite'
$contents = @"
# Written by deploy-server.ps1. Not for version control.
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

$acl = Get-Acl $envPath
$acl.SetAccessRuleProtection($true, $false)
@($acl.Access) | ForEach-Object { $acl.RemoveAccessRule($_) | Out-Null }
foreach ($who in @('BUILTIN\Administrators', 'NT AUTHORITY\SYSTEM')) {
    $acl.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule(
        $who, 'FullControl', 'Allow')))
}
Set-Acl -Path $envPath -AclObject $acl
Write-Ok ".env, readable only by Administrators and SYSTEM"

# Proves the file parses before a service is built on top of it. The failure
# this catches is silent: a bad line leaves earlier variables set, so the
# server starts and then misbehaves rather than refusing outright.
$parsed = @{}
foreach ($line in (Get-Content $envPath)) {
    if ($line -match "^\s*([A-Z_]+)\s*=\s*'?([^']*)'?\s*$") {
        $parsed[$Matches[1]] = $Matches[2]
    }
}
foreach ($key in @('DATABASE_URL', 'BIND_ADDRESS', 'LIBRARY_DIR', 'DICT_PATH')) {
    if (-not $parsed.ContainsKey($key) -or -not $parsed[$key]) {
        throw "The .env just written is missing $key. Refusing to install a service that cannot start."
    }
}
Write-Ok "All four settings read back correctly"

# --- 4. Firewall -------------------------------------------------------------

Write-Step "Firewall"

$ruleName = "Reading Companion ($Port)"
$old = Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue
if ($old) { Remove-NetFirewallRule -DisplayName $ruleName }

if ($Loopback -or $Subnet -eq 'none') {
    Write-Ok "No rule added; the port is not reachable from the network."
} elseif ($Subnet) {
    $rule = @{
        DisplayName = $ruleName
        Description = 'Reading Companion library server'
        Direction   = 'Inbound'
        Action      = 'Allow'
        Protocol    = 'TCP'
        LocalPort   = $Port
        RemoteAddress = $Subnet
        Profile     = 'Private,Domain'
    }
    New-NetFirewallRule @rule | Out-Null
    Write-Ok "TCP $Port open to $Subnet on private and domain networks."
    Write-Host "    Public networks stay closed." -ForegroundColor DarkGray
} else {
    Write-Warn "Could not work out a subnet. Pass -Subnet, or the port stays closed."
}

# --- 5. Service --------------------------------------------------------------

Write-Step "Service"

if ($existing) {
    & sc.exe delete $ServiceName | Out-Null
    Start-Sleep -Seconds 2
    Write-Host "    Replaced the existing service." -ForegroundColor DarkGray
}

$exePath = Join-Path $InstallDir 'reading-server.exe'
& sc.exe create $ServiceName binPath= "`"$exePath`"" start= auto DisplayName= "Reading Companion library" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Could not create the service." }
& sc.exe description $ServiceName "Holds the books, pages, and summaries for Reading Companion" | Out-Null

# Restart on failure rather than staying down after a transient database
# hiccup: 5s, then 5s, then every minute.
& sc.exe failure $ServiceName reset= 86400 actions= restart/5000/restart/5000/restart/60000 | Out-Null

Start-Service -Name $ServiceName
Write-Ok "Running, and set to start automatically."

# --- 6. Verify ---------------------------------------------------------------

Write-Step "Checking it answers"

$ok = $false
for ($i = 0; $i -lt 30; $i++) {
    try {
        $health = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -TimeoutSec 2
        if ($health.status -eq 'ok') { $ok = $true; break }
    } catch {
        Start-Sleep -Seconds 1
    }
}

if (-not $ok) {
    Write-Warn "No answer on 127.0.0.1:$Port yet."
    Write-Warn "Look at what it said:"
    Write-Host "      Get-EventLog -LogName Application -Source $ServiceName -Newest 20" -ForegroundColor Gray
    Write-Host "      Get-Service $ServiceName" -ForegroundColor Gray
    throw "The service did not come up."
}
Write-Ok "Healthy."

$setup = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/setup-state" -TimeoutSec 5

# --- 7. Report ---------------------------------------------------------------

$address = if ($lan -and -not $Loopback) { "http://$($lan.IPAddress):$Port" } else { "http://127.0.0.1:$Port" }

Write-Host ""
Write-Host "  ------------------------------------------------------------" -ForegroundColor Green
Write-Host "  In the application, on the sign-in screen:" -ForegroundColor Green
Write-Host ""
Write-Host "      Library server:  $address" -ForegroundColor White
Write-Host ""
if ($setup.needs_setup) {
    Write-Host "  This library is empty, so it will offer to create an account." -ForegroundColor Green
    Write-Host "  The first one is the administrator." -ForegroundColor Green
} else {
    Write-Host "  This library already has accounts. Sign in with one." -ForegroundColor Green
}
Write-Host "  ------------------------------------------------------------" -ForegroundColor Green
Write-Host ""
Write-Host "  Service:  Get-Service $ServiceName" -ForegroundColor DarkGray
Write-Host "  Restart:  Restart-Service $ServiceName" -ForegroundColor DarkGray
Write-Host "  Books:    $libraryDir" -ForegroundColor DarkGray
Write-Host ""
Write-Host "  Set the inference server's address in Settings once you have" -ForegroundColor DarkGray
Write-Host "  signed in. It defaults to this machine's own Ollama." -ForegroundColor DarkGray
Write-Host ""
if (-not $Loopback) {
    Write-Host "  There is no TLS on this port. That is fine on a LAN; if you" -ForegroundColor Yellow
    Write-Host "  expose it, put a reverse proxy in front the way" -ForegroundColor Yellow
    Write-Host "  secure-ollama-wan.ps1 does for Ollama." -ForegroundColor Yellow
    Write-Host ""
}
