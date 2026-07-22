# SoftMap test runner (docs/TEST_LIST.txt)
# Usage: powershell -ExecutionPolicy Bypass -File scripts\run_tests.ps1
$ErrorActionPreference = 'Continue'

$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $Root

$Exe = Join-Path $Root 'build\Release\softmap.exe'
$Fixture = Join-Path $Root 'tests\fixtures\sample_pc'
$OutDir = Join-Path $Root 'build\test_out'

if (-not (Test-Path $Exe)) {
    Write-Host "ERROR: missing $Exe (build Release first)"
    exit 2
}
if (-not (Test-Path $Fixture)) {
    Write-Host "ERROR: missing fixture $Fixture"
    exit 2
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Get-ChildItem $OutDir -Force -ErrorAction SilentlyContinue |
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$pass = 0
$fail = 0
$results = New-Object System.Collections.Generic.List[string]

function Invoke-SoftMap {
    param([string[]]$ArgList)
    if ($null -eq $ArgList) { $ArgList = @() }

    $stdout = Join-Path $OutDir '_stdout.txt'
    $stderr = Join-Path $OutDir '_stderr.txt'
    Remove-Item $stdout, $stderr -Force -ErrorAction SilentlyContinue

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.RedirectStandardInput = $true
    $psi.CreateNoWindow = $true
    # Quote args that may contain spaces
    $quoted = @()
    foreach ($a in $ArgList) {
        if ($a -match '[\s"]') {
            $quoted += ('"{0}"' -f ($a -replace '"', '\"'))
        } else {
            $quoted += $a
        }
    }
    $psi.Arguments = [string]::Join(' ', $quoted)

    $p = New-Object System.Diagnostics.Process
    $p.StartInfo = $psi
    [void]$p.Start()
    $out = $p.StandardOutput.ReadToEnd()
    $err = $p.StandardError.ReadToEnd()
    $p.WaitForExit()
    return @{ Code = $p.ExitCode; Out = ($out + $err) }
}

function Check {
    param(
        [string]$Id,
        [string]$Name,
        [bool]$Ok,
        [string]$Detail = ''
    )
    if ($Ok) {
        Write-Host ("PASS  {0}  {1}" -f $Id, $Name)
        $script:pass++
        $script:results.Add("PASS  $Id  $Name") | Out-Null
    } else {
        Write-Host ("FAIL  {0}  {1}  {2}" -f $Id, $Name, $Detail)
        $script:fail++
        $script:results.Add("FAIL  $Id  $Name  $Detail") | Out-Null
    }
}

Write-Host '=== SoftMap TEST_LIST runner ==='
Write-Host ("exe: {0}" -f $Exe)
Write-Host ("fixture: {0}" -f $Fixture)
Write-Host ''

# ---- A. CLI ----
$r = Invoke-SoftMap @('-h')
Check 'A-01' 'help' (($r.Code -eq 0) -and ($r.Out -match 'SoftMap') -and ($r.Out -match 'Usage'))

$r = Invoke-SoftMap @()
Check 'A-02' 'no args' (($r.Code -eq 1) -and ($r.Out -match 'Usage'))

$r = Invoke-SoftMap @('nosuchcmd')
Check 'A-03' 'unknown command' (($r.Code -eq 1) -and ($r.Out -match 'unknown|Usage|error'))

$r = Invoke-SoftMap @('report')
Check 'A-04' 'report without path' (($r.Code -eq 1) -and ($r.Out -match 'snapshot path'))

# ---- B. scan ----
$smb = Join-Path $OutDir 'fixture.smb'
$r = Invoke-SoftMap @('-q', 'scan', '-o', $smb, '--drive', $Fixture)
Check 'B-01' 'scan fixture -> .smb' (($r.Code -eq 0) -and (Test-Path $smb)) ($r.Out)

$smap = Join-Path $OutDir 'fixture.smap'
$r = Invoke-SoftMap @('-q', 'scan', '-o', $smap, '--drive', $Fixture)
$head = ''
if (Test-Path $smap) {
    $head = (Get-Content $smap -TotalCount 3 -Encoding UTF8) -join "`n"
}
Check 'B-02' 'scan fixture -> .smap' (($r.Code -eq 0) -and (Test-Path $smap) -and ($head -match 'SoftMap')) ($r.Out)

$softOnly = Join-Path $OutDir 'softonly.smb'
$r = Invoke-SoftMap @('-q', 'scan', '-o', $softOnly, '--software-only')
$infoSoft = ''
if ($r.Code -eq 0) {
    $ri = Invoke-SoftMap @('info', $softOnly)
    $infoSoft = $ri.Out
}
Check 'B-03' 'scan --software-only' (($r.Code -eq 0) -and ($infoSoft -match 'software:')) ($r.Out)

$light = Join-Path $OutDir 'light.smb'
$r = Invoke-SoftMap @('-q', 'scan', '-o', $light, '--light', '--drive', $Fixture)
Check 'B-04' 'scan --light' (($r.Code -eq 0) -and (Test-Path $light)) ($r.Out)

# B-05: auto drive detection runs (no --drive)
$detectOut = Join-Path $OutDir 'detect.smb'
$r = Invoke-SoftMap @('scan', '-o', $detectOut, '--software-only')
Check 'B-05' 'detect drives log' (($r.Code -eq 0) -and ($r.Out -match 'detected drives:')) ($r.Out)

# B-06: --light depth persisted in .smb
$r = Invoke-SoftMap @('info', $light)
Check 'B-06' 'light depth in .smb' (($r.Code -eq 0) -and ($r.Out -match 'folders_and_apps')) ($r.Out)

# ---- C. info / report ----
$r = Invoke-SoftMap @('info', $smb)
Check 'C-01' 'info' (($r.Code -eq 0) -and ($r.Out -match 'host:') -and ($r.Out -match 'nodes:')) ($r.Out)

$r = Invoke-SoftMap @('report', $smb)
Check 'C-02' 'report summary' (($r.Code -eq 0) -and ($r.Out -match 'SoftMap')) ($r.Out)

$r = Invoke-SoftMap @('report', $smb, '--software')
Check 'C-03' 'report --software' ($r.Code -eq 0) ($r.Out)

$r = Invoke-SoftMap @('report', $smb, '--tools')
Check 'C-04' 'report --tools has ffmpeg' (($r.Code -eq 0) -and ($r.Out -match 'ffmpeg\.exe')) ($r.Out)

$r = Invoke-SoftMap @('report', $smb, '--tree', '--depth', '10')
Check 'C-05' 'report --tree' (($r.Code -eq 0) -and ($r.Out -match 'Tools|docs|note|ffmpeg')) ($r.Out)

$r = Invoke-SoftMap @('report', $smb, '--checklist')
Check 'C-06' 'report --checklist' ($r.Code -eq 0) ($r.Out)

$reportTxt = Join-Path $OutDir 'report.txt'
$r = Invoke-SoftMap @('report', $smb, '--tools', '-O', $reportTxt)
Check 'C-07' 'report -O file' (
    ($r.Code -eq 0) -and (Test-Path $reportTxt) -and ((Get-Item $reportTxt).Length -gt 0)
) ($r.Out)

# ---- D. roundtrip ----
$r = Invoke-SoftMap @('report', $smap)
Check 'D-01' 'report .smap' ($r.Code -eq 0) ($r.Out)

$r = Invoke-SoftMap @('info', $smap)
Check 'D-02' 'info .smap' (($r.Code -eq 0) -and ($r.Out -match 'nodes:')) ($r.Out)

# D-03: .smap preserves scan timestamp from header
$scanHdr = ''
if (Test-Path $smap) {
    $scanHdr = (Get-Content $smap -Encoding UTF8 |
        Where-Object { $_ -match '^# scan:' } |
        Select-Object -First 1)
    if ($scanHdr) { $scanHdr = ($scanHdr -replace '^# scan:\s*', '').Trim() }
}
$r = Invoke-SoftMap @('info', $smap)
$scanInfo = ''
if ($r.Out -match 'scan:\s*(.+)') { $scanInfo = $Matches[1].Trim() }
Check 'D-03' 'smap scan time preserved' (
    ($r.Code -eq 0) -and ($scanHdr.Length -gt 0) -and ($scanInfo -eq $scanHdr)
) ("hdr='$scanHdr' info='$scanInfo' $($r.Out)")

# ---- E. restore ----
$restoreTarget = Join-Path $OutDir 'restored'
$r = Invoke-SoftMap @('restore', $smb, '--target', $restoreTarget, '--dry-run', '-y')
Check 'E-01' 'restore --dry-run' ($r.Code -eq 0) ($r.Out)

$r = Invoke-SoftMap @('restore', $smb, '--target', $restoreTarget, '-y')
$toolsDir = $null
if ($r.Code -eq 0 -and (Test-Path $restoreTarget)) {
    $toolsDir = Get-ChildItem $restoreTarget -Recurse -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -eq 'Tools' } |
        Select-Object -First 1
}
Check 'E-02' 'restore creates Tools' (($r.Code -eq 0) -and ($null -ne $toolsDir)) ($r.Out)

# E-03: absolute --map is not double-joined under --target
$mapDest = Join-Path $OutDir 'map_dest'
$wrongTarget = Join-Path $OutDir 'wrong_target'
# Normalize for softmap path matching (fixture paths use backslash)
$fixFrom = $Fixture
if (-not $fixFrom.EndsWith('\')) { $fixFrom = $fixFrom + '\' }
$mapTo = $mapDest
if (-not $mapTo.EndsWith('\')) { $mapTo = $mapTo + '\' }
$r = Invoke-SoftMap @(
    'restore', $smb, '--target', $wrongTarget,
    '--map', ($fixFrom + '=' + $mapTo),
    '--dry-run', '-y'
)
$badDouble = ($r.Out -match [regex]::Escape((Join-Path $wrongTarget 'map_dest')))
$hasMapped = ($r.Out -match [regex]::Escape($mapDest))
Check 'E-03' 'restore --map absolute no double join' (
    ($r.Code -eq 0) -and $hasMapped -and (-not $badDouble)
) ($r.Out)

# ---- F. softmap.conf ----
function Write-Utf8BomFile([string]$Path, [string]$Text) {
    $enc = New-Object System.Text.UTF8Encoding $true
    [System.IO.File]::WriteAllText($Path, $Text, $enc)
}

# F-01: drives= in conf (UTF-8 BOM) overrides auto-detect
$confDrives = Join-Path $OutDir 'conf_drives.conf'
Write-Utf8BomFile $confDrives @"
[tree]
drives = $Fixture
depth = all_files
"@
$smbConf = Join-Path $OutDir 'conf_drives.smb'
$r = Invoke-SoftMap @('scan', '-o', $smbConf, '-c', $confDrives)
$walkOnlyFix = ($r.Out -match [regex]::Escape($Fixture)) -and ($r.Out -notmatch 'walking [A-Z]:\\ \.\.\.')
$infoConf = ''
$nodesConf = 999999
if ($r.Code -eq 0) {
    $ri = Invoke-SoftMap @('info', $smbConf)
    $infoConf = $ri.Out
    if ($infoConf -match 'nodes:\s*(\d+)') { $nodesConf = [int]$Matches[1] }
}
Check 'F-01' 'conf drives=fixture (BOM)' (
    ($r.Code -eq 0) -and $walkOnlyFix -and ($nodesConf -lt 100)
) ("nodes=$nodesConf walkOk=$walkOnlyFix $($r.Out)")

# F-02: exclude=Tools appends and skips Tools
$confEx = Join-Path $OutDir 'conf_exclude.conf'
Write-Utf8BomFile $confEx @"
[tree]
drives = $Fixture
exclude = Tools
"@
$smbEx = Join-Path $OutDir 'conf_exclude.smb'
$r = Invoke-SoftMap @('-q', 'scan', '-o', $smbEx, '-c', $confEx)
$toolsEx = ''
$treeEx = ''
if ($r.Code -eq 0) {
    $toolsEx = (Invoke-SoftMap @('report', $smbEx, '--tools')).Out
    $treeEx = (Invoke-SoftMap @('report', $smbEx, '--tree', '--depth', '20')).Out
}
Check 'F-02' 'conf exclude=Tools' (
    ($r.Code -eq 0) -and ($toolsEx -notmatch 'ffmpeg') -and ($treeEx -notmatch 'Tools') -and ($treeEx -match 'note')
) ($r.Out)

# F-03: depth=folders_and_apps via conf
$confDepth = Join-Path $OutDir 'conf_depth.conf'
Write-Utf8BomFile $confDepth @"
[tree]
drives = $Fixture
depth = folders_and_apps
"@
$smbDepth = Join-Path $OutDir 'conf_depth.smb'
$r = Invoke-SoftMap @('-q', 'scan', '-o', $smbDepth, '-c', $confDepth)
$infoDepth = ''
if ($r.Code -eq 0) {
    $infoDepth = (Invoke-SoftMap @('info', $smbDepth)).Out
}
Check 'F-03' 'conf depth=folders_and_apps' (
    ($r.Code -eq 0) -and ($infoDepth -match 'folders_and_apps')
) ($infoDepth)

# ---- summary ----
Write-Host ''
Write-Host ("=== Result: {0} passed, {1} failed / {2} total ===" -f $pass, $fail, ($pass + $fail))

$summaryPath = Join-Path $OutDir 'TEST_RESULT.txt'
$lines = @(
    'SoftMap TEST_LIST result',
    ("date: {0}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')),
    ("exe: {0}" -f $Exe),
    ("passed: {0}" -f $pass),
    ("failed: {0}" -f $fail),
    ''
) + $results
$lines | Set-Content -Path $summaryPath -Encoding UTF8
Write-Host ("wrote: {0}" -f $summaryPath)

if ($fail -gt 0) { exit 1 } else { exit 0 }
