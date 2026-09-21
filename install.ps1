<#
.SYNOPSIS
    Install NOIDA on Windows.

.DESCRIPTION
    Run this straight from the website:

        irm https://its-banana-coder.github.io/noida/install.ps1 | iex

    It downloads the latest release, puts noida.exe in your user profile,
    puts that directory on PATH for this session and the next one, and tells
    you whether the tools NOIDA drives (git, claude, codex) are installed.

    Nothing here needs administrator rights: everything lands under
    %LOCALAPPDATA%, and only the user PATH is touched.
#>

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repo = 'its-banana-coder/noida'
$dest = Join-Path $env:LOCALAPPDATA 'Programs\noida'

function Write-Step($text) { Write-Host "==> $text" -ForegroundColor Cyan }
function Write-Note($text) { Write-Host "    $text" -ForegroundColor DarkGray }

# Only x86_64 is published today; say so plainly rather than installing a
# binary that cannot run.
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne 'AMD64') {
    Write-Host "NOIDA ships an x86_64 Windows build; this machine reports $arch." -ForegroundColor Yellow
    Write-Host "Build from source instead: cargo install noida" -ForegroundColor Yellow
    return
}

$asset = 'noida-x86_64-pc-windows-msvc.zip'
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("noida-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

try {
    Write-Step "Finding the latest release"
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    # NOIDA is still in alpha, and every release is marked pre-release, so
    # /releases/latest has nothing to point at: ask for the list instead and
    # take the newest release that ships a Windows build.
    $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases?per_page=20" -UseBasicParsing -Headers @{ 'User-Agent' = 'noida-installer' }
    $release = $releases | Where-Object { $_.assets.name -contains $asset } | Select-Object -First 1
    if (-not $release) { throw "no release with $asset found" }
    $url = ($release.assets | Where-Object { $_.name -eq $asset }).browser_download_url
    Write-Note "$($release.tag_name)"

    Write-Step "Downloading $asset"
    $zip = Join-Path $tmp $asset
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing

    Write-Step "Installing to $dest"
    New-Item -ItemType Directory -Path $dest -Force | Out-Null
    # -Force so re-running this upgrades in place.
    Expand-Archive -Path $zip -DestinationPath $dest -Force

    $exe = Join-Path $dest 'noida.exe'
    if (-not (Test-Path $exe)) { throw "noida.exe is missing from $asset" }

    Write-Step "Putting it on PATH"
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($null -eq $userPath) { $userPath = '' }
    $already = $userPath.Split(';') | Where-Object { $_.TrimEnd('\') -ieq $dest.TrimEnd('\') }
    if (-not $already) {
        $joined = if ($userPath.Length -gt 0) { "$userPath;$dest" } else { $dest }
        [Environment]::SetEnvironmentVariable('Path', $joined, 'User')
        Write-Note "added $dest to your user PATH"
    } else {
        Write-Note "already on your user PATH"
    }
    # ...and for the shell running this, so `noida` works without reopening it.
    if (-not ($env:Path.Split(';') | Where-Object { $_.TrimEnd('\') -ieq $dest.TrimEnd('\') })) {
        $env:Path = "$env:Path;$dest"
    }

    $version = (& $exe --version) 2>&1
    Write-Host ""
    Write-Host "  $version installed" -ForegroundColor Green
    Write-Host ""

    # NOIDA drives other command-line tools; missing ones are the usual reason
    # a first run looks broken, so name them now rather than at startup.
    $missing = @()
    foreach ($tool in 'git', 'claude', 'codex') {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { $missing += $tool }
    }
    if ($missing -contains 'git') {
        Write-Host "  Git is not installed; NOIDA uses it for the diff and review views:" -ForegroundColor Yellow
        Write-Host "      winget install Git.Git" -ForegroundColor Yellow
    }
    if (($missing -contains 'claude') -and ($missing -contains 'codex')) {
        Write-Host "  No agent CLI found. Install at least one, then sign in once:" -ForegroundColor Yellow
        Write-Host "      npm install -g @anthropic-ai/claude-code" -ForegroundColor Yellow
        Write-Host "      claude" -ForegroundColor Yellow
        Write-Host ""
    }

    Write-Host "  Open a repository with:" -ForegroundColor White
    Write-Host "      noida C:\path\to\your\repo" -ForegroundColor White
    Write-Host ""
    Write-Note "Use Windows Terminal for truecolor and mouse support."
    Write-Note "Alt+x opens the command palette; Ctrl+] then a key is the fallback."
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
