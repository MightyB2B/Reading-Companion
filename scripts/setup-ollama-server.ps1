<#
.SYNOPSIS
    Opens an existing Windows Ollama install to other machines on your network,
    so Reading Companion can run its models there instead of on your laptop.

.DESCRIPTION
    Ollama listens on 127.0.0.1 out of the box, which means only the machine it
    is installed on can talk to it. This script:

      1. sets OLLAMA_HOST so it binds every interface, plus the two tuning
         variables the app benefits from;
      2. adds a firewall rule scoped to your local subnet, not to the whole
         internet;
      3. restarts Ollama so it picks up the new environment;
      4. pulls the three models the app uses;
      5. verifies the port actually answers, and prints the address to type
         into Settings.

    Nothing here installs Ollama. It assumes 'ollama' is already on PATH.

    Deliberately ASCII-only: PowerShell 5.1 reads a BOM-less .ps1 as ANSI, and
    a stray smart quote or dash becomes a parse error on someone else's box.

.PARAMETER Port
    Port to listen on. Default 11434.

.PARAMETER IPAddress
    Which of this machine's addresses to advertise. Detected from the default
    route when omitted, which is what skips WSL and Hyper-V virtual switches.
    Pass it if the wrong adapter still wins.

.PARAMETER Subnet
    CIDR range allowed through the firewall. Derived from the chosen address
    when omitted, e.g. 192.168.1.0/24. Widen it for a routed network.

.PARAMETER Models
    Models to pull. Defaults to the three Reading Companion uses, plus
    qwen3:8b, which is the better coaching model if you have the VRAM for it.

.PARAMETER MaxLoaded
    How many models may sit in VRAM at once. Default 3, which keeps every
    model the app uses resident so switching between them costs nothing.

.PARAMETER KeepAlive
    How long an idle model stays loaded. Default -1, meaning never unload.
    Right for a dedicated server; use something like 30m on a shared machine.

.PARAMETER BindLanOnly
    Listen only on the chosen LAN address instead of every interface. Tighter,
    but Ollama will not start if that address ever changes, so use it only with
    a static IP or a DHCP reservation.

.PARAMETER System
    Set the environment variables machine-wide instead of for the current user.
    Needed only if Ollama runs as a service or under a different account.
    Requires an elevated prompt.

.PARAMETER SkipPull
    Configure and restart, but do not download models.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\setup-ollama-server.ps1

.EXAMPLE
    # Wider than one subnet, e.g. a routed home network
    .\setup-ollama-server.ps1 -Subnet 10.0.0.0/8

.EXAMPLE
    # Force the adapter, when a virtual switch keeps winning
    .\setup-ollama-server.ps1 -IPAddress 192.168.4.252

.NOTES
    Ollama has no authentication of its own. Anyone who can reach this port can
    use your GPU and read every prompt sent to it. The firewall scope is the
    real protection: keep it as tight as your network allows, and do not
    forward this port on your router.

    To reach it from outside your network, put a reverse proxy in front that
    requires a bearer token, and give that token to the app's API key field.
    Exposing Ollama directly to the internet is not a supported setup.
#>

[CmdletBinding()]
param(
    [int]$Port = 11434,
    [string]$IPAddress,
    [string]$Subnet,
    [string[]]$Models = @('glm-ocr', 'qwen3:4b', 'qwen3:8b', 'qwen3-vl:4b'),
    [int]$MaxLoaded = 3,
    [string]$KeepAlive = '-1',
    [switch]$BindLanOnly,
    [switch]$System,
    [switch]$SkipPull
)

$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$Text) Write-Host "`n==> $Text" -ForegroundColor Cyan }
function Write-Ok   { param([string]$Text) Write-Host "    $Text" -ForegroundColor Green }
function Write-Warn { param([string]$Text) Write-Host "    $Text" -ForegroundColor Yellow }

# --- 0. Preconditions --------------------------------------------------------

$ollama = Get-Command ollama -ErrorAction SilentlyContinue
if (-not $ollama) {
    throw "ollama is not on PATH. Install it from https://ollama.com/download, then re-run."
}

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if ($System -and -not $isAdmin) {
    throw "-System writes machine-wide variables. Re-run this from an elevated PowerShell."
}

$scope = 'User'
if ($System) { $scope = 'Machine' }

Write-Host "Ollama: $($ollama.Source)" -ForegroundColor DarkGray

# --- 1. Work out what to let through the firewall ----------------------------

function Get-NetworkCidr {
    param([string]$IPAddress, [int]$PrefixLength)

    $ipBytes = ([System.Net.IPAddress]::Parse($IPAddress)).GetAddressBytes()
    $netBytes = New-Object byte[] 4
    for ($i = 0; $i -lt 4; $i++) {
        # Bits of the mask falling inside this byte: 8, a partial count, or 0.
        $bits = [Math]::Min(8, [Math]::Max(0, $PrefixLength - ($i * 8)))
        $mask = [byte](256 - [Math]::Pow(2, 8 - $bits))
        $netBytes[$i] = [byte]($ipBytes[$i] -band $mask)
    }
    return "{0}.{1}.{2}.{3}/{4}" -f $netBytes[0], $netBytes[1], $netBytes[2], $netBytes[3], $PrefixLength
}

# The address other machines will actually dial.
#
# Interface metric is not the signal it looks like: a Hyper-V or WSL virtual
# switch routinely gets a lower metric than the physical NIC, and binding to
# 172.x.x.x would leave the real network locked out. The adapter carrying the
# default route is the one that reaches anything at all, and a virtual switch
# does not carry one.
$candidates = @(Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
    Where-Object {
        $_.IPAddress -ne '127.0.0.1' -and
        $_.IPAddress -notlike '169.254.*' -and
        ($_.PrefixOrigin -eq 'Dhcp' -or $_.PrefixOrigin -eq 'Manual')
    })

if ($candidates.Count -eq 0) {
    throw "Could not find a network address on this machine. Is it connected?"
}

$lan = $null
$how = ''

if ($IPAddress) {
    $lan = $candidates | Where-Object { $_.IPAddress -eq $IPAddress } | Select-Object -First 1
    if (-not $lan) {
        throw "-IPAddress $IPAddress is not on this machine. Found: $(($candidates.IPAddress) -join ', ')"
    }
    $how = 'you chose it'
}

if (-not $lan) {
    $gateway = Get-NetRoute -DestinationPrefix '0.0.0.0/0' -AddressFamily IPv4 -ErrorAction SilentlyContinue |
        Sort-Object RouteMetric, InterfaceMetric |
        Select-Object -First 1
    if ($gateway) {
        $lan = $candidates |
            Where-Object { $_.InterfaceIndex -eq $gateway.InterfaceIndex } |
            Select-Object -First 1
        $how = 'carries the default route'
    }
}

if (-not $lan) {
    # No default route at all, so fall back to metric order with the virtual
    # switches taken out by name.
    $physical = $candidates | Where-Object {
        $_.InterfaceAlias -notmatch 'vEthernet|WSL|Hyper-V|VirtualBox|VMware|Loopback|Bluetooth'
    }
    if (-not $physical) { $physical = $candidates }
    $lan = $physical |
        Sort-Object { (Get-NetIPInterface -InterfaceIndex $_.InterfaceIndex -AddressFamily IPv4).InterfaceMetric } |
        Select-Object -First 1
    $how = 'lowest metric, virtual adapters skipped'
}

if (-not $lan) {
    throw "Could not work out which address to use. Re-run with -IPAddress."
}

if (-not $Subnet) {
    $Subnet = Get-NetworkCidr -IPAddress $lan.IPAddress -PrefixLength $lan.PrefixLength
}

Write-Host "Address: $($lan.IPAddress) on $($lan.InterfaceAlias)  [$how]" -ForegroundColor DarkGray
Write-Host "Subnet:  $Subnet" -ForegroundColor DarkGray

if ($candidates.Count -gt 1) {
    Write-Host "Not used:" -ForegroundColor DarkGray
    foreach ($c in $candidates) {
        if ($c.IPAddress -ne $lan.IPAddress) {
            Write-Host "         $($c.IPAddress) on $($c.InterfaceAlias)" -ForegroundColor DarkGray
        }
    }
    Write-Host "         Re-run with -IPAddress if the wrong one was picked." -ForegroundColor DarkGray
}

# --- 2. Environment ----------------------------------------------------------

Write-Step "Setting environment variables ($scope scope)"

# 0.0.0.0 means every interface, including ones that do not exist yet: a VPN, a
# second NIC, a public address if this box ever gets one. Binding to the single
# LAN address removes that category outright. The cost is that Ollama will fail
# to start if this address ever changes, so it is opt-in and wants a static
# lease or a DHCP reservation behind it.
$bind = '0.0.0.0'
if ($BindLanOnly) {
    $bind = $lan.IPAddress
}

$vars = [ordered]@{
    # Which interfaces to answer on. Without this Ollama talks only to itself.
    'OLLAMA_HOST'              = "${bind}:$Port"
    # Reading Companion switches between a transcription model and a text model
    # constantly, and reaches for a vision model when a page number will not
    # come out of the text. Holding all of them resident turns a 5-10s reload
    # into nothing.
    'OLLAMA_MAX_LOADED_MODELS' = "$MaxLoaded"
    # On a dedicated box there is nothing to give the memory back to, so never
    # unload. Note this is only a default: a client that sends its own
    # keep_alive on a request wins, and Reading Companion currently does.
    'OLLAMA_KEEP_ALIVE'        = $KeepAlive
    # One reader, so one slot. Ollama otherwise splits the context window
    # across parallel slots and duplicates the KV cache for each, which buys
    # nothing here and costs real VRAM.
    'OLLAMA_NUM_PARALLEL'      = '1'
}

foreach ($name in $vars.Keys) {
    [Environment]::SetEnvironmentVariable($name, $vars[$name], $scope)
    Write-Ok "$name = $($vars[$name])"
}

# --- 3. Firewall -------------------------------------------------------------

Write-Step "Opening TCP $Port to $Subnet"

$ruleName = "Ollama ($Port)"
if ($isAdmin) {
    $existing = Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue
    if ($existing) {
        Remove-NetFirewallRule -DisplayName $ruleName
        Write-Host "    Replaced the existing rule." -ForegroundColor DarkGray
    }

    $rule = @{
        DisplayName   = $ruleName
        Description   = 'Ollama API, for Reading Companion on other machines'
        Direction     = 'Inbound'
        Action        = 'Allow'
        Protocol      = 'TCP'
        LocalPort     = $Port
        RemoteAddress = $Subnet
        Profile       = 'Private,Domain'
    }
    New-NetFirewallRule @rule | Out-Null

    Write-Ok "Rule '$ruleName' allows $Subnet on private and domain networks."
    Write-Host "    Public networks stay closed, so cafe Wi-Fi cannot reach it." -ForegroundColor DarkGray
} else {
    Write-Warn "Skipped: needs an elevated prompt. Run this from an admin PowerShell:"
    Write-Host ""
    $hint = "New-NetFirewallRule -DisplayName '$ruleName' -Direction Inbound " +
            "-Action Allow -Protocol TCP -LocalPort $Port " +
            "-RemoteAddress $Subnet -Profile Private,Domain"
    Write-Host "      $hint" -ForegroundColor Gray
    Write-Host ""
}

# --- 4. Restart, so the new environment is actually read ---------------------

Write-Step "Restarting Ollama"

# A running process keeps the environment it started with, so the variables
# above mean nothing until it comes back up.
$running = @(Get-Process -Name 'ollama', 'ollama app' -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    $running | Stop-Process -Force
    Write-Ok "Stopped $($running.Count) process(es)."
    Start-Sleep -Milliseconds 1500
} else {
    Write-Host "    Was not running." -ForegroundColor DarkGray
}

$tray = Join-Path $env:LOCALAPPDATA 'Programs\Ollama\ollama app.exe'
if (Test-Path $tray) {
    Start-Process -FilePath $tray
    Write-Ok "Started the tray app."
} else {
    Start-Process -FilePath $ollama.Source -ArgumentList 'serve' -WindowStyle Hidden
    Write-Ok "Started 'ollama serve'."
}

# This shell still has the old environment, and the CLI reads OLLAMA_HOST to
# decide where to connect. Point it at the loopback explicitly so the pulls
# below do not depend on 0.0.0.0 being dialable.
$env:OLLAMA_HOST = "127.0.0.1:$Port"

Write-Host "    Waiting for the port to answer..." -NoNewline
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
    throw "Ollama did not come back up on port $Port. Start it by hand and re-run."
}
Write-Ok "Answering on $Port."

# --- 5. Models ---------------------------------------------------------------

if ($SkipPull) {
    Write-Step "Skipping model downloads (-SkipPull)"
} else {
    Write-Step "Pulling models"
    Write-Host "    First run downloads several GB. Later runs verify and exit." -ForegroundColor DarkGray

    foreach ($model in $Models) {
        Write-Host ""
        Write-Host "    --- $model ---" -ForegroundColor DarkGray
        & $ollama.Source pull $model
        if ($LASTEXITCODE -ne 0) {
            Write-Warn "'$model' failed to pull. Check the name at https://ollama.com/library"
        }
    }
}

# --- 6. Report ---------------------------------------------------------------

Write-Step "Installed"

$tags = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/tags" -TimeoutSec 10
foreach ($m in ($tags.models | Sort-Object name)) {
    $gb = [Math]::Round($m.size / 1GB, 1)
    $sees = ''
    if ($m.details.families -contains 'clip' -or $m.details.families -contains 'mllama') {
        $sees = '  (vision)'
    }
    Write-Host ("    {0,-28} {1,6} GB{2}" -f $m.name, $gb, $sees)
}

# --- 7. What will actually fit ----------------------------------------------

# With MAX_LOADED_MODELS above 1 and no unloading, whatever the app touches
# stays in VRAM. Worth seeing the arithmetic: the weights below are the whole
# budget minus context, and a few hundred MB of KV cache sits on top of each.
Write-Step "VRAM budget"

$gpu = Get-CimInstance Win32_VideoController -ErrorAction SilentlyContinue |
    Where-Object { $_.AdapterRAM -gt 0 } |
    Sort-Object AdapterRAM -Descending |
    Select-Object -First 1

if ($gpu) {
    # AdapterRAM is a uint32 and silently wraps above 4GB, so it is worth
    # nothing as a number. The name is the useful part.
    Write-Host "    GPU: $($gpu.Name)" -ForegroundColor DarkGray
}

function Get-ModelSizeGb {
    param([string]$Name)
    $hit = $tags.models | Where-Object { $_.name -eq $Name -or $_.name -eq "${Name}:latest" }
    if ($hit) { return [Math]::Round($hit.size / 1GB, 1) }
    return 0
}

$ocrGb    = Get-ModelSizeGb 'glm-ocr'
$smallGb  = Get-ModelSizeGb 'qwen3:4b'
$bigGb    = Get-ModelSizeGb 'qwen3:8b'
$visionGb = Get-ModelSizeGb 'qwen3-vl:4b'

Write-Host ""
Write-Host "    Coaching with qwen3:4b" -ForegroundColor White
Write-Host ("      glm-ocr {0} + qwen3:4b {1} + qwen3-vl:4b {2} = {3} GB resident" -f `
    $ocrGb, $smallGb, $visionGb, [Math]::Round($ocrGb + $smallGb + $visionGb, 1))
Write-Host ""
Write-Host "    Coaching with qwen3:8b" -ForegroundColor White
Write-Host ("      glm-ocr {0} + qwen3:8b {1} + qwen3-vl:4b {2} = {3} GB resident" -f `
    $ocrGb, $bigGb, $visionGb, [Math]::Round($ocrGb + $bigGb + $visionGb, 1))
Write-Host ""
Write-Host "    On 12 GB both fit, the second with less headroom. The coaching" -ForegroundColor DarkGray
Write-Host "    model is the one worth spending VRAM on: it grades your summaries" -ForegroundColor DarkGray
Write-Host "    and picks dictionary senses. Transcription barely benefits." -ForegroundColor DarkGray
Write-Host "    If Ollama starts spilling to CPU, re-run with -MaxLoaded 2 and" -ForegroundColor DarkGray
Write-Host "    let the rarely-used vision model load on demand." -ForegroundColor DarkGray

$address = "http://$($lan.IPAddress):$Port"

Write-Host ""
Write-Host "  ------------------------------------------------------------" -ForegroundColor Green
Write-Host "  Enter this in Reading Companion, Settings, Ollama address:" -ForegroundColor Green
Write-Host ""
Write-Host "      $address" -ForegroundColor White
Write-Host ""
Write-Host "  Leave the API key blank. This server has no authentication," -ForegroundColor Green
Write-Host "  which is why the firewall rule is scoped to $Subnet." -ForegroundColor Green
Write-Host "  Do not forward this port on your router." -ForegroundColor Green
Write-Host ""
Write-Host "  Then set 'Coaching and the dictionary' to qwen3:8b in the same" -ForegroundColor Green
Write-Host "  window. The dropdown lists whatever this server has." -ForegroundColor Green
Write-Host "  ------------------------------------------------------------" -ForegroundColor Green
Write-Host ""
Write-Host "  Check from the reading machine with:" -ForegroundColor DarkGray
Write-Host "      curl $address/api/tags" -ForegroundColor DarkGray
Write-Host ""
Write-Host "  Note: OLLAMA_KEEP_ALIVE above is only a default. A value sent on" -ForegroundColor DarkGray
Write-Host "  a request wins over it, and Reading Companion sends one every" -ForegroundColor DarkGray
Write-Host "  time. To match this server, set 'Keeping models loaded' to" -ForegroundColor DarkGray
Write-Host "  'Never unload' in Settings. Otherwise the models unload on the" -ForegroundColor DarkGray
Write-Host "  app's schedule, not this one." -ForegroundColor DarkGray
Write-Host ""
