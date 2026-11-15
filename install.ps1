# yd installer for Windows (works on PowerShell 7 "pwsh" and Windows PowerShell 5.1)
#   irm https://raw.githubusercontent.com/Slaviaaa2/yd/master/install.ps1 | iex
# From cmd.exe use install.cmd (it prefers pwsh, falls back to powershell).
# Env:
#   YD_INSTALL_DIR  install directory (default: %LOCALAPPDATA%\Programs\yd)
#   YD_VERSION      release tag, e.g. v1.0.0 (default: latest)
#   YD_MENU         yes / no: add a Start menu shortcut without asking
#
# Wrapped in a script block so `irm | iex` never leaks variables or calls `exit`
# on the caller's shell; failures `throw` (exit code 1 under -Command).
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'  # Invoke-WebRequest is very slow on 5.1 with the progress bar
    try {
        # Windows PowerShell 5.1 may default to TLS 1.0, which GitHub rejects
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {}

    $repo = 'Slaviaaa2/yd'
    $version = if ($env:YD_VERSION) { $env:YD_VERSION } else { 'latest' }
    $dir = if ($env:YD_INSTALL_DIR) { $env:YD_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\yd' }

    # 32-bit PowerShell on 64-bit Windows reports x86 here and AMD64 in PROCESSOR_ARCHITEW6432
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    switch ($arch) {
        'AMD64' { }
        'ARM64' { Write-Host '! ARM64: installing the x64 build (runs under emulation)' -ForegroundColor Yellow }
        default { throw "unsupported architecture: $arch" }
    }

    $asset = 'yd-windows-x64.zip'
    $url = if ($version -eq 'latest') {
        "https://github.com/$repo/releases/latest/download/$asset"
    } else {
        "https://github.com/$repo/releases/download/$version/$asset"
    }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('yd-install-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Write-Host "* Downloading $asset ($version)"
        $zip = Join-Path $tmp $asset
        Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
        Expand-Archive -Path $zip -DestinationPath $tmp -Force
        $bin = Get-ChildItem -Path $tmp -Recurse -Filter 'yd.exe' | Select-Object -First 1
        if (-not $bin) { throw 'yd.exe not found in the archive' }
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        $target = Join-Path $dir 'yd.exe'
        try {
            Copy-Item -Path $bin.FullName -Destination $target -Force
        } catch {
            throw "could not write $target (is yd running? close it and retry): $($_.Exception.Message)"
        }
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
    Write-Host "OK Installed $target" -ForegroundColor Green

    # Add to the user PATH. Read/write the raw registry value so entries like %USERPROFILE%\bin
    # keep their unexpanded form (SetEnvironmentVariable would flatten them).
    $key = 'HKCU:\Environment'
    $raw = (Get-Item -Path $key).GetValue('Path', '', 'DoNotExpandEnvironmentNames')
    $parts = @($raw -split ';' | Where-Object { $_ -ne '' })
    $already = $parts | Where-Object { [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -ieq $dir.TrimEnd('\') }
    if (-not $already) {
        Set-ItemProperty -Path $key -Name 'Path' -Value (($parts + $dir) -join ';') -Type ExpandString
        # Broadcast WM_SETTINGCHANGE so new terminals pick up the change
        [Environment]::SetEnvironmentVariable('YD_INSTALL_TMP', '1', 'User')
        [Environment]::SetEnvironmentVariable('YD_INSTALL_TMP', $null, 'User')
        Write-Host "  - Added $dir to your user PATH. Open a new terminal to use 'yd'."
    }
    if (-not (($env:Path -split ';') -contains $dir)) { $env:Path = "$env:Path;$dir" }

    # Start menu shortcut. Ask only when someone can answer; otherwise follow YD_MENU (default: skip).
    $menu = $env:YD_MENU
    if (-not $menu) {
        $interactive = [Environment]::UserInteractive -and -not [Console]::IsInputRedirected
        if ($interactive) {
            $answer = Read-Host 'Add yd to the Start menu? [Y/n]'
            $menu = if ($answer -match '^[nN]') { 'no' } else { 'yes' }
        } else {
            $menu = 'no'
        }
    }
    if ($menu -match '^(y|yes|1|true)$') {
        $programs = [Environment]::GetFolderPath('Programs')  # %APPDATA%\Microsoft\Windows\Start Menu\Programs
        $lnk = Join-Path $programs 'yd.lnk'
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut($lnk)
        $shortcut.TargetPath = $target
        $shortcut.WorkingDirectory = $env:USERPROFILE
        $shortcut.Description = 'Fast yt-dlp front-end: paste a URL to download video or audio'
        $shortcut.IconLocation = "$target,0"
        $shortcut.Save()
        [void][Runtime.InteropServices.Marshal]::ReleaseComObject($shell)
        Write-Host "OK Added to the Start menu" -ForegroundColor Green
        Write-Host "  - $lnk"
    }

    try { & $target --version } catch {}
}
