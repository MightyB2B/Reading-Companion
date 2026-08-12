<#
.SYNOPSIS
    Creates the Reading Companion database, a role with a randomly generated
    password, and applies the schema.

.DESCRIPTION
    Run once per machine that needs a library database -- your development
    box and the server both. It:

      1. finds psql, whether or not it is on PATH;
      2. generates a 32-character password from the OS CSPRNG;
      3. creates the role and database, owned by that role;
      4. applies every file in crates/reading-core/migrations in order;
      5. writes .env with the connection string, readable only by you.

    The generated password is printed once and written to .env. It is not
    stored anywhere else and cannot be recovered -- re-run with -Force to
    issue a new one.

    Connecting as the superuser needs the password you set when installing
    PostgreSQL. Set it in the environment rather than passing it as an
    argument, so it does not land in your shell history:

        $env:PGPASSWORD = 'your-postgres-password'

    Deliberately ASCII-only: PowerShell 5.1 reads a BOM-less .ps1 as ANSI,
    and a stray smart quote becomes a parse error on someone else's box.

.PARAMETER Database
    Database to create. Default reading_companion.

.PARAMETER User
    Role to create and own it. Default reading_app.

.PARAMETER Password
    Use this password instead of generating one. Intended for reproducing an
    existing setup on a second machine, not for normal use -- a password you
    typed is a password in your shell history.

.PARAMETER DbHost
    Server to connect to. Default localhost.

.PARAMETER Port
    Default 5432.

.PARAMETER SuperUser
    Role with permission to create databases. Default postgres.

.PARAMETER PsqlPath
    Full path to psql.exe, when it is neither on PATH nor in the usual place.

.PARAMETER Force
    DESTRUCTIVE. Drops the database and role first, discarding every book,
    page, and summary in it, then recreates them with a new password.

.EXAMPLE
    $env:PGPASSWORD = 'your-postgres-password'
    .\scripts\setup-postgres.ps1

.EXAMPLE
    # Point the app at a database on the server instead of localhost
    .\scripts\setup-postgres.ps1 -DbHost 192.168.4.252

.NOTES
    Applies schema only. Your existing SQLite library is not touched and not
    imported -- that is a separate migration.
#>

[CmdletBinding()]
param(
    [string]$Database = 'reading_companion',
    [string]$User = 'reading_app',
    [string]$Password,
    [string]$DbHost = 'localhost',
    [int]$Port = 5432,
    [string]$SuperUser = 'postgres',
    [string]$PsqlPath,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$Text) Write-Host "`n==> $Text" -ForegroundColor Cyan }
function Write-Ok   { param([string]$Text) Write-Host "    $Text" -ForegroundColor Green }
function Write-Warn { param([string]$Text) Write-Host "    $Text" -ForegroundColor Yellow }

# --- 0. Find psql ------------------------------------------------------------

$psql = $null
if ($PsqlPath) {
    if (-not (Test-Path $PsqlPath)) { throw "-PsqlPath $PsqlPath does not exist." }
    $psql = $PsqlPath
} else {
    $onPath = Get-Command psql -ErrorAction SilentlyContinue
    if ($onPath) {
        $psql = $onPath.Source
    } else {
        # The Windows installer does not add itself to PATH. Newest version first.
        $psql = Get-ChildItem 'C:\Program Files\PostgreSQL' -Directory -ErrorAction SilentlyContinue |
            Sort-Object { [int]($_.Name -replace '\D', '0') } -Descending |
            ForEach-Object { Join-Path $_.FullName 'bin\psql.exe' } |
            Where-Object { Test-Path $_ } |
            Select-Object -First 1
    }
}

if (-not $psql) {
    Write-Host ""
    Write-Warn "psql not found. Install PostgreSQL, then re-run:"
    Write-Host ""
    Write-Host "      winget install PostgreSQL.PostgreSQL.17" -ForegroundColor Gray
    Write-Host ""
    Write-Host "    Or pass -PsqlPath C:\path\to\psql.exe" -ForegroundColor DarkGray
    Write-Host ""
    throw "psql not found."
}

Write-Host "psql: $psql" -ForegroundColor DarkGray

if (-not $env:PGPASSWORD) {
    Write-Host ""
    Write-Warn "The superuser password is not set. Set it and re-run:"
    Write-Host ""
    Write-Host "      `$env:PGPASSWORD = 'your-postgres-password'" -ForegroundColor Gray
    Write-Host ""
    Write-Host "    This is the password you chose when installing PostgreSQL." -ForegroundColor DarkGray
    Write-Host "    Setting it in the environment keeps it out of your history." -ForegroundColor DarkGray
    Write-Host ""
    throw "PGPASSWORD not set."
}

# --- 1. Locate the migrations ------------------------------------------------

$repoRoot = Split-Path -Parent $PSScriptRoot
$migrationDir = Join-Path $repoRoot 'crates\reading-core\migrations'

if (-not (Test-Path $migrationDir)) {
    throw "No migrations directory at $migrationDir. Run this from the repository."
}

$migrations = @(Get-ChildItem $migrationDir -Filter '*.sql' | Sort-Object Name)
if ($migrations.Count -eq 0) {
    throw "No .sql files in $migrationDir."
}

Write-Host "Migrations: $($migrations.Count) file(s) in $migrationDir" -ForegroundColor DarkGray

# --- 2. Password -------------------------------------------------------------

function New-Password {
    param([int]$Length = 32)

    # Alphanumeric only, and without the characters people misread. Two
    # reasons: this ends up inside a URL, where + / = @ : would all need
    # percent-encoding, and it will get typed by hand at least once.
    $alphabet = 'abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789'

    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $chars = New-Object char[] $Length
        $byte = New-Object byte[] 1
        # Largest multiple of the alphabet size that fits in a byte. Values at
        # or above it are drawn again rather than folded with a modulo, which
        # would make the first few letters measurably more likely.
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

Write-Step "Password"
$generated = $false
if (-not $Password) {
    $Password = New-Password
    $generated = $true
    Write-Ok "Generated 32 characters from the OS CSPRNG."
} else {
    Write-Warn "Using the password you supplied."
}

# Doubled for SQL string literals. Generated passwords contain no quotes, but
# a supplied one might.
$escaped = $Password.Replace("'", "''")

# --- 3. Create the role and database ----------------------------------------

# Run a statement against the maintenance database as the superuser.
function Invoke-Super {
    param([string]$Sql, [switch]$AllowFailure)

    $out = & $psql --host $DbHost --port $Port --username $SuperUser `
        --dbname postgres --no-password --quiet --tuples-only `
        --set ON_ERROR_STOP=1 --command $Sql

    if ($LASTEXITCODE -ne 0 -and -not $AllowFailure) {
        throw "psql failed on: $Sql"
    }
    return $out
}

Write-Step "Checking the server"
$version = Invoke-Super "SELECT version();"
Write-Ok ($version | Select-Object -First 1).ToString().Trim()

if ($Force) {
    Write-Step "Dropping the existing database and role (-Force)"
    Write-Warn "Every book, page, and summary in '$Database' is being discarded."
    # Sessions on the database would block the drop.
    Invoke-Super "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '$Database' AND pid <> pg_backend_pid();" -AllowFailure | Out-Null
    Invoke-Super "DROP DATABASE IF EXISTS ""$Database"";"
    Invoke-Super "DROP ROLE IF EXISTS ""$User"";"
    Write-Ok "Dropped."
}

Write-Step "Creating role '$User'"
$roleExists = (Invoke-Super "SELECT 1 FROM pg_roles WHERE rolname = '$User';") -match '1'
if ($roleExists) {
    Invoke-Super "ALTER ROLE ""$User"" WITH LOGIN PASSWORD '$escaped';"
    Write-Ok "Role already existed; its password has been reset."
} else {
    Invoke-Super "CREATE ROLE ""$User"" WITH LOGIN PASSWORD '$escaped';"
    Write-Ok "Created."
}

Write-Step "Creating database '$Database'"
$dbExists = (Invoke-Super "SELECT 1 FROM pg_database WHERE datname = '$Database';") -match '1'
if ($dbExists) {
    Write-Warn "Database already exists. Migrations below may fail if it is already"
    Write-Warn "populated -- use -Force to start clean."
} else {
    # CREATE DATABASE cannot run inside a transaction block, which is why this
    # is its own call rather than part of a script.
    Invoke-Super "CREATE DATABASE ""$Database"" OWNER ""$User"" ENCODING 'UTF8';"
    Write-Ok "Created, owned by $User."
}

# In PostgreSQL 15 and later the public schema is no longer writable by
# everyone, so the owner needs saying explicitly.
& $psql --host $DbHost --port $Port --username $SuperUser --dbname $Database `
    --no-password --quiet --set ON_ERROR_STOP=1 `
    --command "ALTER SCHEMA public OWNER TO ""$User""; GRANT ALL ON SCHEMA public TO ""$User"";"
if ($LASTEXITCODE -ne 0) { throw "Could not grant the public schema to $User." }
Write-Ok "Granted the public schema."

# --- 4. Apply the migrations -------------------------------------------------

Write-Step "Applying migrations"

# As the new role, so every table is owned by the account the app connects as.
$superPassword = $env:PGPASSWORD
try {
    $env:PGPASSWORD = $Password
    foreach ($m in $migrations) {
        & $psql --host $DbHost --port $Port --username $User --dbname $Database `
            --no-password --quiet --set ON_ERROR_STOP=1 --file $m.FullName
        if ($LASTEXITCODE -ne 0) {
            throw "Migration failed: $($m.Name)"
        }
        Write-Ok $m.Name
    }

    $tables = & $psql --host $DbHost --port $Port --username $User --dbname $Database `
        --no-password --quiet --tuples-only `
        --command "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public';"
    Write-Ok "$(($tables | Out-String).Trim()) tables in place."
} finally {
    $env:PGPASSWORD = $superPassword
}

# --- 5. Write .env -----------------------------------------------------------

Write-Step "Writing .env"

$url = "postgres://${User}:${Password}@${DbHost}:${Port}/${Database}"
$envPath = Join-Path $repoRoot '.env'

$contents = @"
# Written by scripts/setup-postgres.ps1. Not for version control.
DATABASE_URL=$url
"@

# WriteAllText rather than Set-Content -Encoding utf8: on PowerShell 5.1 that
# writes a BOM, which some dotenv parsers read as part of the first key name.
[System.IO.File]::WriteAllText($envPath, $contents, (New-Object System.Text.UTF8Encoding($false)))

# The password is in this file in plain text. Take everyone else off it.
$acl = Get-Acl $envPath
$acl.SetAccessRuleProtection($true, $false)
@($acl.Access) | ForEach-Object { $acl.RemoveAccessRule($_) | Out-Null }
$me = [Security.Principal.WindowsIdentity]::GetCurrent().Name
foreach ($who in @($me, 'BUILTIN\Administrators', 'NT AUTHORITY\SYSTEM')) {
    try {
        $acl.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule(
            $who, 'FullControl', 'Allow')))
    } catch {
        Write-Warn "Could not grant access to $who"
    }
}
Set-Acl -Path $envPath -AclObject $acl
Write-Ok "$envPath, readable only by you."

$gitignore = Join-Path $repoRoot '.gitignore'
$ignored = $false
if (Test-Path $gitignore) {
    $ignored = (Get-Content $gitignore) -contains '.env'
}
if (-not $ignored) {
    Add-Content -Path $gitignore -Value "`n# Database credentials, written by scripts/setup-postgres.ps1`n.env"
    Write-Ok "Added .env to .gitignore."
}

# --- 6. Report ---------------------------------------------------------------

Write-Host ""
Write-Host "  ------------------------------------------------------------" -ForegroundColor Green
Write-Host "  Database ready." -ForegroundColor Green
Write-Host ""
Write-Host "    Host:      $DbHost`:$Port" -ForegroundColor White
Write-Host "    Database:  $Database" -ForegroundColor White
Write-Host "    User:      $User" -ForegroundColor White
Write-Host "    Password:  $Password" -ForegroundColor White
Write-Host ""
Write-Host "  DATABASE_URL is in .env and the app reads it from there." -ForegroundColor Green
Write-Host "  ------------------------------------------------------------" -ForegroundColor Green
Write-Host ""

if ($generated) {
    Write-Host "  That password is printed once and stored only in .env." -ForegroundColor Yellow
    Write-Host "  Copy it now if you want it in a password manager. Re-running" -ForegroundColor Yellow
    Write-Host "  this script issues a new one." -ForegroundColor Yellow
    Write-Host ""
}

Write-Host "  Connect by hand with:" -ForegroundColor DarkGray
Write-Host "      `$env:PGPASSWORD = '<the password above>'" -ForegroundColor DarkGray
Write-Host "      & '$psql' -h $DbHost -p $Port -U $User -d $Database" -ForegroundColor DarkGray
Write-Host ""
Write-Host "  Remember to clear the superuser password from this shell:" -ForegroundColor DarkGray
Write-Host "      Remove-Item Env:PGPASSWORD" -ForegroundColor DarkGray
Write-Host ""
