<#
.SYNOPSIS
    Puts an authenticating HTTPS reverse proxy in front of Ollama so it can be
    exposed to the internet without handing strangers your GPU.

    NOT NEEDED BY READING COMPANION ANY MORE. The application talks to
    reading-server, which talks to Ollama on its own machine; Ollama itself
    should be on loopback and reachable by nothing. TLS and authentication now
    live in reading-server, which install-server.ps1 sets up.

    Kept for exposing Ollama to something else -- another tool, another
    machine -- where you still need a proxy that can authenticate, because
    Ollama cannot.

.DESCRIPTION
    Ollama has no authentication of its own, and its API can pull, create, and
    delete models. Exposed directly, port 11434 is a remote wipe button and an
    unmetered GPU for anyone who finds it. This script builds the arrangement
    that makes WAN exposure survivable:

      1. rebinds Ollama to 127.0.0.1, so the proxy is the ONLY way in and the
         token cannot be bypassed by talking to 11434 directly;
      2. generates a 256-bit bearer token;
      3. writes a Caddyfile that terminates TLS with a real certificate,
         requires that token, and allows only the two endpoints Reading
         Companion actually calls -- GET /api/tags and POST /api/generate;
      4. closes 11434 at the firewall and opens 80 and 443;
      5. installs Caddy as a Windows service so it survives a reboot.

    Everything else Ollama exposes -- /api/pull, /api/create, /api/delete,
    /api/push, /api/copy -- is refused by the proxy. A stolen token gets an
    attacker inference, not the ability to delete your models or fill the disk.

    Requires a domain name whose A record already points at this machine's
    public IP, because the certificate is issued over HTTP-01 and that needs
    port 80 reachable from the internet.

.PARAMETER Domain
    Public hostname, e.g. ollama.example.com. Its A record must already
    resolve to this machine.

.PARAMETER Email
    Address for the certificate authority to send expiry warnings to.

.PARAMETER Token
    Bearer token to require. A new 256-bit one is generated when omitted,
    which is what you want unless you are rotating a known value.

.PARAMETER Port
    Port Ollama listens on locally. Default 11434. Never exposed.

.PARAMETER ConfigDir
    Where the Caddyfile lives. Default C:\ProgramData\Caddy.

.PARAMETER CaddyPath
    Full path to caddy.exe. Use when Caddy is not on PATH -- a downloaded
    binary, or a fresh install in a shell that has not been restarted.

.PARAMETER SkipService
    Write the config and leave the service alone. Use when Caddy already runs
    under something else.

.EXAMPLE
    .\secure-ollama-wan.ps1 -Domain ollama.example.com -Email you@example.com

.NOTES
    Run from an elevated PowerShell: it changes firewall rules and installs a
    service.

    Run setup-ollama-server.ps1 first. This script assumes Ollama is installed
    and the models are pulled; it only changes how the outside world reaches
    it.

    What this does NOT give you: rate limiting. Caddy needs a plugin for that
    (github.com/mholt/caddy-ratelimit, via xcaddy). Without it, a valid token
    can queue work as fast as your GPU accepts it. The token is the control.

    Still true after all this: an exposed service is a standing risk. Rotate
    the token if it ever lands somewhere it should not, and keep Ollama
    updated, because you are now its internet-facing attack surface.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Domain,
    [Parameter(Mandatory = $true)][string]$Email,
    [string]$Token,
    [int]$Port = 11434,
    [string]$ConfigDir = 'C:\ProgramData\Caddy',
    [string]$CaddyPath,
    [switch]$SkipService
)

$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$Text) Write-Host "`n==> $Text" -ForegroundColor Cyan }
function Write-Ok   { param([string]$Text) Write-Host "    $Text" -ForegroundColor Green }
function Write-Warn { param([string]$Text) Write-Host "    $Text" -ForegroundColor Yellow }

# --- 0. Preconditions --------------------------------------------------------

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not $isAdmin) {
    throw "This changes firewall rules and installs a service. Re-run from an elevated PowerShell."
}

$ollama = Get-Command ollama -ErrorAction SilentlyContinue
if (-not $ollama) {
    throw "ollama is not on PATH. Run setup-ollama-server.ps1 first."
}

# PATH first, then the places an install actually lands. A fresh winget or
# choco install is not on PATH until the shell is restarted, and a zip
# extracted to the Desktop never will be -- neither is a reason to stop.
$caddy = $null

if ($CaddyPath) {
    if (-not (Test-Path $CaddyPath)) {
        throw "-CaddyPath $CaddyPath does not exist."
    }
    $caddy = Get-Command $CaddyPath
} else {
    $caddy = Get-Command caddy -ErrorAction SilentlyContinue
}

if (-not $caddy) {
    $guesses = @(
        (Join-Path $env:ProgramFiles 'Caddy\caddy.exe'),
        (Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Links\caddy.exe'),
        'C:\ProgramData\chocolatey\bin\caddy.exe',
        (Join-Path $PWD 'caddy.exe'),
        (Join-Path $HOME 'Desktop\caddy.exe'),
        (Join-Path $HOME 'Downloads\caddy.exe')
    )
    foreach ($g in $guesses) {
        if (Test-Path $g) {
            $caddy = Get-Command $g
            Write-Warn "Found Caddy at $g (not on PATH)."
            break
        }
    }
}

if (-not $caddy) {
    $hasWinget = $null -ne (Get-Command winget -ErrorAction SilentlyContinue)
    $hasChoco  = $null -ne (Get-Command choco -ErrorAction SilentlyContinue)

    Write-Host ""
    Write-Warn "Caddy is not installed. Install it, then re-run this script."
    Write-Host ""

    if ($hasWinget) {
        # Publisher is CaddyServer, not Caddy. 'winget search caddy' if this
        # ever moves.
        Write-Host "      winget install CaddyServer.Caddy" -ForegroundColor Gray
    }
    if ($hasChoco) {
        Write-Host "      choco install caddy -y" -ForegroundColor Gray
    }
    if (-not $hasWinget -and -not $hasChoco) {
        Write-Host "    Neither winget nor choco is on this machine, which is normal" -ForegroundColor DarkGray
        Write-Host "    for Windows Server. Download it by hand instead:" -ForegroundColor DarkGray
        Write-Host ""
        Write-Host "      1. https://caddyserver.com/download" -ForegroundColor Gray
        Write-Host "         Platform windows / amd64, no extra modules needed." -ForegroundColor DarkGray
        Write-Host "      2. Save it as caddy.exe" -ForegroundColor Gray
        Write-Host "      3. Re-run this script with:" -ForegroundColor Gray
        Write-Host "           -CaddyPath C:\path\to\caddy.exe" -ForegroundColor Gray
    } else {
        Write-Host ""
        Write-Host "    Close and reopen PowerShell afterwards so PATH updates," -ForegroundColor DarkGray
        Write-Host "    or pass -CaddyPath to skip PATH entirely." -ForegroundColor DarkGray
    }
    Write-Host ""
    throw "caddy not found."
}

Write-Host "Ollama: $($ollama.Source)" -ForegroundColor DarkGray
Write-Host "Caddy:  $($caddy.Source)" -ForegroundColor DarkGray

# The certificate is issued over HTTP-01, so the name has to resolve here
# before Caddy will get anywhere. Checking now turns a confusing retry loop in
# the service log into one clear message.
Write-Step "Checking DNS for $Domain"
try {
    $resolved = @((Resolve-DnsName -Name $Domain -Type A -ErrorAction Stop |
        Where-Object { $_.QueryType -eq 'A' }).IPAddress)
} catch {
    throw "$Domain does not resolve. Point an A record at this machine's public IP first."
}

if ($resolved.Count -eq 0) {
    throw "$Domain has no A record. Point one at this machine's public IP first."
}
Write-Ok "$Domain -> $($resolved -join ', ')"

try {
    $public = (Invoke-RestMethod -Uri 'https://api.ipify.org?format=json' -TimeoutSec 10).ip
    if ($resolved -contains $public) {
        Write-Ok "Matches this machine's public IP ($public)."
    } else {
        Write-Warn "This machine's public IP is $public, which is not in that list."
        Write-Warn "Certificate issuance will fail unless traffic for $Domain reaches here."
    }
} catch {
    Write-Host "    Could not determine the public IP. Carrying on." -ForegroundColor DarkGray
}

# --- 1. Token ----------------------------------------------------------------

Write-Step "Bearer token"

if (-not $Token) {
    # 32 bytes from the OS CSPRNG. Not Get-Random, which is seeded predictably
    # and has no business generating a credential.
    $bytes = New-Object byte[] 32
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
    # Hex rather than base64: no +, / or = to be mangled by a shell or a
    # config file somewhere down the line.
    $Token = -join ($bytes | ForEach-Object { $_.ToString('x2') })
    Write-Ok "Generated a new 256-bit token."
} else {
    Write-Ok "Using the token you supplied."
}

# --- 2. Rebind Ollama to loopback -------------------------------------------

Write-Step "Rebinding Ollama to 127.0.0.1"

# This is the load-bearing step. While Ollama answers on 0.0.0.0, anyone who
# can reach port 11434 skips the proxy and every check in it.
[Environment]::SetEnvironmentVariable('OLLAMA_HOST', "127.0.0.1:$Port", 'Machine')
[Environment]::SetEnvironmentVariable('OLLAMA_HOST', "127.0.0.1:$Port", 'User')
Write-Ok "OLLAMA_HOST = 127.0.0.1:$Port"

$running = @(Get-Process -Name 'ollama', 'ollama app' -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    $running | Stop-Process -Force
    Start-Sleep -Milliseconds 1500
}

$tray = Join-Path $env:LOCALAPPDATA 'Programs\Ollama\ollama app.exe'
if (Test-Path $tray) {
    Start-Process -FilePath $tray
} else {
    Start-Process -FilePath $ollama.Source -ArgumentList 'serve' -WindowStyle Hidden
}

$env:OLLAMA_HOST = "127.0.0.1:$Port"
Write-Host "    Waiting for Ollama..." -NoNewline
$up = $false
for ($i = 0; $i -lt 30; $i++) {
    try {
        Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/tags" -TimeoutSec 2 | Out-Null
        $up = $true
        break
    } catch {
        Start-Sleep -Seconds 1
        Write-Host "." -NoNewline
    }
}
Write-Host ""
if (-not $up) {
    throw "Ollama did not come back up on 127.0.0.1:$Port."
}
Write-Ok "Answering on loopback."

# --- 3. Caddyfile ------------------------------------------------------------

Write-Step "Writing the proxy config"

if (-not (Test-Path $ConfigDir)) {
    New-Item -ItemType Directory -Path $ConfigDir -Force | Out-Null
}
$caddyfile = Join-Path $ConfigDir 'Caddyfile'

# Allowlist, not blocklist. Reading Companion calls exactly two endpoints, so
# everything else -- including /api/pull, /api/delete and /api/create -- gets a
# 404 without ever reaching Ollama. A blocklist would need updating every time
# Ollama grows an endpoint; this does not.
# Written as a single `route` on purpose. Caddy normally sorts directives into
# its own order, which makes "reject first, then proxy" a question of whether
# respond happens to sort before reverse_proxy. Inside a route, order is
# exactly as written, so the auth check demonstrably runs first.
#
# No `encode` here either: responses are streamed token by token, and
# compression re-buffers what flush_interval is trying to push out.
$config = @"
{
	email $Email
}

$Domain {
	log {
		output file $ConfigDir\access.log {
			roll_size 10MiB
			roll_keep 10
		}
	}

	# Pages travel as base64 inside the JSON body, so this has to be
	# generous. It is still a bound: without one a single request can be made
	# arbitrarily large.
	request_body {
		max_size 48MB
	}

	header {
		Strict-Transport-Security "max-age=31536000; includeSubDomains"
		X-Content-Type-Options "nosniff"
		# Do not announce what is running here.
		-Server
	}

	route {
		# Anything without the token stops here, whatever it was asking for.
		@unauthorized not header Authorization "Bearer $Token"
		respond @unauthorized "unauthorized" 401

		@tags {
			method GET
			path /api/tags
		}
		@generate {
			method POST
			path /api/generate
		}

		reverse_proxy @tags 127.0.0.1:$Port

		reverse_proxy @generate 127.0.0.1:$Port {
			# Transcription streams token by token. Without this Caddy
			# buffers the response and the progress display in the app sits
			# empty until the whole page is finished.
			flush_interval -1

			# A cold model load plus a full page can run to several minutes.
			# The app gives up at 300s; the proxy must not give up first.
			transport http {
				read_timeout 600s
				write_timeout 600s
				dial_timeout 10s
			}
		}

		# Everything else, including every destructive Ollama endpoint:
		# /api/pull, /api/create, /api/delete, /api/push, /api/copy.
		respond "not found" 404
	}
}
"@

# Not Set-Content -Encoding utf8: on PowerShell 5.1 that writes a BOM, and a
# BOM at the top of a Caddyfile is a parse error in something that otherwise
# looks perfectly correct.
[System.IO.File]::WriteAllText($caddyfile, $config, (New-Object System.Text.UTF8Encoding($false)))
Write-Ok "Wrote $caddyfile"

# The token is in that file in plain text, so take Users off it. Administrators
# and SYSTEM keep access because the service runs as SYSTEM.
$acl = Get-Acl $caddyfile
$acl.SetAccessRuleProtection($true, $false)
# Materialised with @() first: removing from the live collection while
# enumerating it skips entries.
@($acl.Access) | ForEach-Object { $acl.RemoveAccessRule($_) | Out-Null }
foreach ($who in @('BUILTIN\Administrators', 'NT AUTHORITY\SYSTEM')) {
    $acl.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule(
        $who, 'FullControl', 'Allow')))
}
Set-Acl -Path $caddyfile -AclObject $acl
Write-Ok "Restricted it to Administrators and SYSTEM."

& $caddy.Source validate --config $caddyfile --adapter caddyfile
if ($LASTEXITCODE -ne 0) {
    throw "Caddy rejected the config. It is at $caddyfile."
}
Write-Ok "Caddy validated the config."

# --- 4. Firewall -------------------------------------------------------------

Write-Step "Firewall"

# The LAN rule from setup-ollama-server.ps1 is now both useless and misleading:
# Ollama is on loopback, so nothing can reach it that way regardless.
$old = Get-NetFirewallRule -DisplayName "Ollama ($Port)" -ErrorAction SilentlyContinue
if ($old) {
    Remove-NetFirewallRule -DisplayName "Ollama ($Port)"
    Write-Ok "Removed the old inbound rule for $Port."
}

foreach ($p in @(80, 443)) {
    $name = "Caddy ($p)"
    $existing = Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue
    if ($existing) { Remove-NetFirewallRule -DisplayName $name }

    $rule = @{
        DisplayName = $name
        Description = 'HTTPS reverse proxy in front of Ollama'
        Direction   = 'Inbound'
        Action      = 'Allow'
        Protocol    = 'TCP'
        LocalPort   = $p
        Profile     = 'Any'
    }
    New-NetFirewallRule @rule | Out-Null
    Write-Ok "Opened TCP $p."
}

Write-Host "    80 is needed for certificate issuance and renewal, not just setup." -ForegroundColor DarkGray

# --- 5. Service --------------------------------------------------------------

if ($SkipService) {
    Write-Step "Leaving the service alone (-SkipService)"
} else {
    Write-Step "Installing Caddy as a service"

    $svc = Get-Service -Name 'caddy' -ErrorAction SilentlyContinue
    if ($svc) {
        Stop-Service -Name 'caddy' -Force -ErrorAction SilentlyContinue
        & sc.exe delete caddy | Out-Null
        Start-Sleep -Seconds 2
        Write-Host "    Replaced the existing service." -ForegroundColor DarkGray
    }

    $bin = "`"$($caddy.Source)`" run --config `"$caddyfile`" --adapter caddyfile"
    & sc.exe create caddy binPath= "$bin" start= auto DisplayName= "Caddy (Ollama proxy)" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Could not create the service."
    }
    & sc.exe description caddy "HTTPS reverse proxy authenticating access to Ollama" | Out-Null
    Start-Service -Name 'caddy'
    Write-Ok "Service 'caddy' is running and set to start automatically."
}

# --- 6. Verify ---------------------------------------------------------------

Write-Step "Checking it works"

Write-Host "    Waiting for the certificate (can take 30s on first run)..." -NoNewline
$ok = $false
for ($i = 0; $i -lt 45; $i++) {
    try {
        $headers = @{ Authorization = "Bearer $Token" }
        Invoke-RestMethod -Uri "https://$Domain/api/tags" -Headers $headers -TimeoutSec 5 | Out-Null
        $ok = $true
        break
    } catch {
        Start-Sleep -Seconds 2
        Write-Host "." -NoNewline
    }
}
Write-Host ""

if ($ok) {
    Write-Ok "https://$Domain/api/tags answered with the token."
} else {
    Write-Warn "Could not reach https://$Domain/api/tags yet."
    Write-Warn "Check: ports 80 and 443 forwarded on the router, DNS propagated."
    Write-Warn "Logs:  Get-Content $ConfigDir\access.log -Tail 40"
}

# A request with no token must be refused. If this passes, the proxy is not
# protecting anything and the whole exercise is theatre.
try {
    Invoke-RestMethod -Uri "https://$Domain/api/tags" -TimeoutSec 5 | Out-Null
    Write-Warn "SERIOUS: an unauthenticated request succeeded. Do not forward ports yet."
} catch {
    if ($_.Exception.Response.StatusCode.value__ -eq 401) {
        Write-Ok "An unauthenticated request is refused with 401."
    } else {
        Write-Host "    Unauthenticated request failed as expected." -ForegroundColor DarkGray
    }
}

# Destructive endpoints must not be reachable even with a valid token.
try {
    $headers = @{ Authorization = "Bearer $Token" }
    Invoke-RestMethod -Uri "https://$Domain/api/delete" -Method Delete -Headers $headers -TimeoutSec 5 | Out-Null
    Write-Warn "SERIOUS: /api/delete was reachable. Check the Caddyfile."
} catch {
    Write-Ok "/api/delete is refused even with the token."
}

# --- 7. Report ---------------------------------------------------------------

Write-Host ""
Write-Host "  ------------------------------------------------------------" -ForegroundColor Green
Write-Host "  Reading Companion -> Settings" -ForegroundColor Green
Write-Host ""
Write-Host "    Ollama address:  https://$Domain" -ForegroundColor White
Write-Host "    API key:         $Token" -ForegroundColor White
Write-Host ""
Write-Host "  ------------------------------------------------------------" -ForegroundColor Green
Write-Host ""
Write-Host "  On the router, forward ONLY 80 and 443 to this machine." -ForegroundColor Yellow
Write-Host "  Never forward $Port. Ollama is on loopback now, so the proxy" -ForegroundColor Yellow
Write-Host "  is the only way in and the token cannot be sidestepped." -ForegroundColor Yellow
Write-Host ""
Write-Host "  That token is the whole of your security. It is in" -ForegroundColor Yellow
Write-Host "  $caddyfile, readable only by Administrators." -ForegroundColor Yellow
Write-Host "  Copy it now; this is the only time it is printed." -ForegroundColor Yellow
Write-Host ""
Write-Host "  To rotate it later:" -ForegroundColor DarkGray
Write-Host "      .\secure-ollama-wan.ps1 -Domain $Domain -Email $Email" -ForegroundColor DarkGray
Write-Host ""
Write-Host "  Service:  Get-Service caddy" -ForegroundColor DarkGray
Write-Host "  Logs:     Get-Content $ConfigDir\access.log -Tail 40 -Wait" -ForegroundColor DarkGray
Write-Host ""
