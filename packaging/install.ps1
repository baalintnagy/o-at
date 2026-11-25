<#
.SYNOPSIS
Installs or uninstalls O-AT from the unpacked release folder.

.DESCRIPTION
Copies the VST3 and CLAP plugins into the system plugin folders (Windows asks for
administrator rights for that step only), the applications into
%LOCALAPPDATA%\Programs\O-AT, and adds Start Menu shortcuts for the applications.
-Uninstall removes all of it; the applications' settings in %APPDATA% stay.

Close plugin hosts and O-AT applications first: files in use cannot be replaced.

.EXAMPLE
.\install.ps1
.EXAMPLE
.\install.ps1 -Uninstall
.EXAMPLE
.\install.ps1 -SkipPlugins
#>
[CmdletBinding()]
param(
    # Removes what an installation added.
    [switch]$Uninstall,
    # Leaves the plugins alone; only the applications and shortcuts.
    [switch]$SkipPlugins,
    # Leaves the applications and shortcuts alone; only the plugins.
    [switch]$SkipApps,
    # Target folders. The defaults are the standard locations.
    [string]$Vst3Dir = (Join-Path $env:CommonProgramFiles 'VST3'),
    [string]$ClapDir = (Join-Path $env:CommonProgramFiles 'CLAP'),
    [string]$AppDir = (Join-Path $env:LOCALAPPDATA 'Programs\O-AT'),
    [string]$StartMenuDir = (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\O-AT'),
    # Internal: the elevated copy of this script that handles the plugin folders, and the
    # file where it reports an error.
    [switch]$PluginsOnly,
    [string]$ErrorLog
)

$ErrorActionPreference = 'Stop'
$source = $PSScriptRoot
$plugins = 'O-NEC', 'O-NR', 'O-AUD-TX', 'O-AUD-RX', 'O-AUD-A'
# Start Menu name and executable of each application.
$apps = [ordered]@{
    'O-AUD-RX' = 'O-AUD-RX.exe'
    'O-AUD-TX' = 'O-AUD-TX.exe'
    'O-AUD-A'  = 'O-AUD-A-standalone.exe'
    'O-NEC'    = 'O-NEC-standalone.exe'
    'O-NR'     = 'O-NR-standalone.exe'
}
# Files and shortcuts of earlier versions that the current ones replace.
$obsoleteApps = 'O-AUD-TX-standalone.exe'
$obsoleteShortcuts = 'O-AUD-TX-standalone.lnk', 'O-AUD-A-standalone.lnk',
    'O-NEC-standalone.lnk', 'O-NR-standalone.lnk'

function Test-Writable([string]$Directory) {
    try {
        New-Item -ItemType Directory -Force -Path $Directory | Out-Null
        $probe = Join-Path $Directory ".o-at-$PID"
        [IO.File]::WriteAllText($probe, '')
        Remove-Item -LiteralPath $probe
        return $true
    } catch {
        return $false
    }
}

function Set-Plugins {
    foreach ($name in $plugins) {
        $vst3 = Join-Path $Vst3Dir "$name.vst3"
        $clap = Join-Path $ClapDir "$name.clap"
        if (Test-Path -LiteralPath $vst3) { Remove-Item -LiteralPath $vst3 -Recurse -Force }
        if ($Uninstall) {
            if (Test-Path -LiteralPath $clap) { Remove-Item -LiteralPath $clap -Force }
            Write-Host "Removed $name plugins"
        } else {
            New-Item -ItemType Directory -Force -Path $Vst3Dir, $ClapDir | Out-Null
            Copy-Item -LiteralPath (Join-Path $source "VST3\$name.vst3") -Destination $vst3 -Recurse
            Copy-Item -LiteralPath (Join-Path $source "CLAP\$name.clap") -Destination $clap -Force
            Write-Host "Installed $name.vst3 and $name.clap"
        }
    }
}

function Set-Apps {
    $shell = New-Object -ComObject WScript.Shell
    foreach ($obsolete in $obsoleteShortcuts) {
        $path = Join-Path $StartMenuDir $obsolete
        if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
    }
    foreach ($obsolete in $obsoleteApps) {
        $path = Join-Path $AppDir $obsolete
        if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
    }
    if ($Uninstall) {
        foreach ($folder in $AppDir, $StartMenuDir) {
            if (Test-Path -LiteralPath $folder) { Remove-Item -LiteralPath $folder -Recurse -Force }
        }
        Write-Host "Removed the applications from $AppDir and their Start Menu shortcuts"
        return
    }
    New-Item -ItemType Directory -Force -Path $AppDir, $StartMenuDir | Out-Null
    foreach ($name in $apps.Keys) {
        $exe = Join-Path $AppDir $apps[$name]
        Copy-Item -LiteralPath (Join-Path $source $apps[$name]) -Destination $exe -Force
        $link = $shell.CreateShortcut((Join-Path $StartMenuDir "$name.lnk"))
        $link.TargetPath = $exe
        $link.WorkingDirectory = $AppDir
        $link.IconLocation = "$exe,0"
        $link.Save()
        Write-Host "Installed $name to $exe"
    }
}

if ($PluginsOnly) {
    # Elevated child, in a hidden window: report failures through the exit code and log.
    try {
        Set-Plugins
        exit 0
    } catch {
        if ($ErrorLog) { $_.Exception.Message | Set-Content -LiteralPath $ErrorLog }
        exit 1
    }
}

if (-not $Uninstall) {
    $missing = @($plugins | ForEach-Object { "VST3\$_.vst3"; "CLAP\$_.clap" }) + @($apps.Values) |
        Where-Object { -not (Test-Path -LiteralPath (Join-Path $source $_)) }
    if ($missing) {
        throw "Run this script from the unpacked O-AT folder; missing: $($missing -join ', ')"
    }
}

if (-not $SkipApps) {
    try {
        Set-Apps
    } catch {
        throw "Could not update the applications (close any running O-AT application): $($_.Exception.Message)"
    }
}

if (-not $SkipPlugins) {
    if ((Test-Writable $Vst3Dir) -and (Test-Writable $ClapDir)) {
        Set-Plugins
    } else {
        Write-Host 'The plugin folders need administrator rights; Windows will ask for them.'
        $log = Join-Path ([IO.Path]::GetTempPath()) "o-at-install-$PID.log"
        $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"",
            '-PluginsOnly', '-Vst3Dir', "`"$Vst3Dir`"", '-ClapDir', "`"$ClapDir`"",
            '-ErrorLog', "`"$log`"")
        if ($Uninstall) { $arguments += '-Uninstall' }
        try {
            $child = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru `
                -WindowStyle Hidden -ArgumentList $arguments
        } catch {
            throw 'The plugins were not changed: administrator rights were declined.'
        }
        if ($child.ExitCode -ne 0) {
            $reason = if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log -Raw } else { '' }
            Remove-Item -LiteralPath $log -ErrorAction SilentlyContinue
            throw "Could not update the plugins (close every plugin host and try again): $reason"
        }
        Write-Host "$(if ($Uninstall) { 'Removed' } else { 'Installed' }) the plugins in $Vst3Dir and $ClapDir"
    }
}

if ($Uninstall) {
    Write-Host 'O-AT is uninstalled. Settings in %APPDATA%\O-AUD-RX and %APPDATA%\O-AUD-TX stay.'
} else {
    Write-Host 'O-AT is installed. Rescan plugins in your host; they are listed under the vendor "o-at".'
}
