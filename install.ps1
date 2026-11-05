# Install alias-deck from a GitHub release on Windows:
#
#   irm https://raw.githubusercontent.com/christopher-goh-hub/alias-deck/main/install.ps1 | iex
#
# It downloads the Windows archive and the release's SHA256SUMS, refuses to install unless the
# archive's SHA-256 matches, and puts alias-deck.exe in a directory you own. No administrator.
#
#   ALIAS_DECK_VERSION      a release tag such as v0.1.0 (default: the latest release)
#   ALIAS_DECK_INSTALL_DIR  where the binary goes (default: %LOCALAPPDATA%\Programs\alias-deck\bin)
#   ALIAS_DECK_BASE_URL     the directory holding the archives, so tests can serve a local copy
#
# It throws instead of calling exit, because under `iex` exit would close your PowerShell window.
# Written for both PowerShell 7 and Windows PowerShell 5.1.

& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    $repo = 'christopher-goh-hub/alias-deck'
    $target = 'x86_64-pc-windows-msvc'

    if ($env:ALIAS_DECK_BASE_URL) {
        $base = $env:ALIAS_DECK_BASE_URL.TrimEnd('/')
    } elseif ($env:ALIAS_DECK_VERSION) {
        $base = "https://github.com/$repo/releases/download/$env:ALIAS_DECK_VERSION"
    } else {
        $base = "https://github.com/$repo/releases/latest/download"
    }
    if ($env:ALIAS_DECK_INSTALL_DIR) {
        $dir = $env:ALIAS_DECK_INSTALL_DIR
    } else {
        $dir = Join-Path $env:LOCALAPPDATA 'Programs\alias-deck\bin'
    }
    # Windows PowerShell 5.1 does not offer TLS 1.2 by default, and GitHub requires it.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
    $null = New-Item -ItemType Directory -Path $tmp
    try {
        $sums = Join-Path $tmp 'SHA256SUMS'
        Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums

        # SHA256SUMS names every archive, so it also gives the file name for this version.
        $expected = $null
        $archive = $null
        foreach ($line in Get-Content -LiteralPath $sums) {
            if ($line -match "^([0-9a-f]{64}) [ *](alias-deck-[A-Za-z0-9._-]+-$target\.zip)$") {
                $expected = $Matches[1]
                $archive = $Matches[2]
                break
            }
        }
        if (-not $archive) { throw "alias-deck install: the release has no build for $target" }

        $zip = Join-Path $tmp $archive
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$archive" -OutFile $zip
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLowerInvariant()
        if ($actual -ne $expected) {
            throw "alias-deck install: checksum mismatch for ${archive}: SHA256SUMS says $expected, the download is $actual. Nothing was installed."
        }

        $unpacked = Join-Path $tmp 'unpacked'
        Expand-Archive -LiteralPath $zip -DestinationPath $unpacked
        $null = New-Item -ItemType Directory -Force -Path $dir
        $exe = Join-Path $dir 'alias-deck.exe'
        # Copy next to the target, then rename, so a running alias-deck is never half-written.
        $new = Join-Path $dir '.alias-deck.exe.new'
        Copy-Item -LiteralPath (Join-Path $unpacked 'alias-deck.exe') -Destination $new -Force
        Move-Item -LiteralPath $new -Destination $exe -Force

        $onWindows = ($PSVersionTable.PSEdition -eq 'Desktop') -or $IsWindows
        $version = 'alias-deck'
        if ($onWindows) { $version = & $exe --version }
        Write-Output "Installed $version to $exe (sha256 $actual)."

        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $onPath = @(($env:Path, $userPath) -join ';' -split ';' | Where-Object { $_ -and ($_.TrimEnd('\') -eq $dir.TrimEnd('\')) }).Count -gt 0
        if (-not $onPath) {
            Write-Output ''
            Write-Output "$dir is not on your PATH. To add it for your user, run:"
            Write-Output "  [Environment]::SetEnvironmentVariable('Path', `"$dir;`" + [Environment]::GetEnvironmentVariable('Path', 'User'), 'User')"
        }
        Write-Output ''
        Write-Output 'Next:'
        Write-Output '  alias-deck search                    # browse the starter packs'
        Write-Output '  alias-deck add git-essentials        # see everything it adds, then confirm'
        Write-Output '  alias-deck setup                     # load your deck in every new PowerShell'
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}
