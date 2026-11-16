# Parqueter installer for Windows.
#
#   irm https://raw.githubusercontent.com/clarityai-eng/parqueter/master/install.ps1 | iex
#
# Installs the parqueter.exe binary into ~/.local/bin (or $env:PARQUETER_BIN_DIR).
#
# Environment overrides:
#   PARQUETER_VERSION   release tag to install, e.g. v0.2.0 (default: latest)
#   PARQUETER_BIN_DIR   where to put parqueter.exe (default: ~/.local/bin)

$ErrorActionPreference = "Stop"

$Repo    = "clarityai-eng/parqueter"
$Version = if ($env:PARQUETER_VERSION) { $env:PARQUETER_VERSION } else { "latest" }
$BinDir  = if ($env:PARQUETER_BIN_DIR) { $env:PARQUETER_BIN_DIR } else { Join-Path $HOME ".local\bin" }

if ($Version -eq "latest") {
    $BaseUrl = "https://github.com/$Repo/releases/latest/download"
} else {
    $BaseUrl = "https://github.com/$Repo/releases/download/$Version"
}

$Asset = "parqueter-windows-x86_64.zip"
$Url   = "$BaseUrl/$Asset"

$Tmp = New-TemporaryFile | ForEach-Object {
    Remove-Item $_
    New-Item -ItemType Directory -Path "$($_.FullName)_dir"
}
try {
    $ZipPath = Join-Path $Tmp $Asset

    Write-Host "Downloading $Asset"
    Invoke-WebRequest -Uri $Url -OutFile $ZipPath -UseBasicParsing

    Expand-Archive -Path $ZipPath -DestinationPath $Tmp -Force

    if (-not (Test-Path (Join-Path $Tmp "parqueter.exe"))) {
        throw "archive does not contain parqueter.exe"
    }

    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
    Copy-Item (Join-Path $Tmp "parqueter.exe") (Join-Path $BinDir "parqueter.exe") -Force
    Write-Host "Installed $BinDir\parqueter.exe"

    # Check if BinDir is in PATH.
    $UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
    if ($UserPath -notlike "*$BinDir*") {
        Write-Host ""
        Write-Host "$BinDir is not in your PATH. To add it, run:"
        Write-Host "  [Environment]::SetEnvironmentVariable('PATH', `"$BinDir;`$env:PATH`", 'User')"
    }
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
