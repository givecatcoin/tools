# ============================================================================
# Snapline E2E tests aligned with docs/test-spec.md (section I).
# Exit code 0 only when every case passes.
# ============================================================================
$ErrorActionPreference = "Stop"
$exe = Resolve-Path (Join-Path $PSScriptRoot "..\target\release\snapline.exe")
$root = Join-Path $env:TEMP ("snapline_e2e_" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $root | Out-Null

$results = @()
function Pass([string]$id, [string]$detail) {
    $script:results += [pscustomobject]@{ Id = $id; Result = "PASS"; Detail = $detail }
    Write-Host "PASS $id  $detail"
}
function Fail([string]$id, [string]$detail) {
    $script:results += [pscustomobject]@{ Id = $id; Result = "FAIL"; Detail = $detail }
    Write-Host "FAIL $id  $detail"
    throw "E2E failed: $id"
}
function Invoke-Snapline {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$CliArgs)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $exe @CliArgs 2>&1 | Out-Null
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
    }
}
function Invoke-SnaplineOutput {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$CliArgs)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = & $exe @CliArgs 2>&1
        $global:LASTEXITCODE = $LASTEXITCODE
        return $output
    } finally {
        $ErrorActionPreference = $prev
    }
}
function Invoke-SnaplineExpectFail {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$CliArgs)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $exe @CliArgs 2>&1 | Out-Null
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
    }
}

try {
    # --- I-01 init ---
    $tree = Join-Path $root "tree"
    New-Item -ItemType Directory -Path $tree | Out-Null
    if ((Invoke-Snapline init $tree) -ne 0) { Fail "I-01" "init failed" }
    if (-not (Test-Path (Join-Path $tree ".snapline\config.json"))) {
        Fail "I-01" "config.json missing"
    }
    Pass "I-01" "init created local store"

    # --- I-15 no auto-init ---
    $bare = Join-Path $root "bare"
    New-Item -ItemType Directory -Path $bare | Out-Null
    $code = Invoke-SnaplineExpectFail --target $bare snap
    if ($code -eq 0) { Fail "I-15" "snap succeeded without init" }
    if (Test-Path (Join-Path $bare ".snapline")) { Fail "I-15" "store was created unexpectedly" }
    Pass "I-15" "snap without init fails and creates nothing"

    # --- I-14 double init ---
    $code = Invoke-SnaplineExpectFail init $tree
    if ($code -eq 0) { Fail "I-14" "second init succeeded" }
    Pass "I-14" "second init rejected"

    # --- content for snap ---
    $nested = Join-Path $tree "project\src"
    New-Item -ItemType Directory -Path $nested -Force | Out-Null
    Set-Content -Path (Join-Path $nested "hello.txt") -Value "hello-snapline" -NoNewline
    Set-Content -Path (Join-Path $tree "secret.env") -Value "TOKEN=keep-me" -NoNewline
    Set-Content -Path (Join-Path $tree ".gitignore") -Value "secret.env`n" -NoNewline

    # child .snapline (nested store content)
    $childStore = Join-Path $tree "app\.snapline"
    New-Item -ItemType Directory -Path $childStore -Force | Out-Null
    Set-Content -Path (Join-Path $childStore "child-marker.txt") -Value "child-store" -NoNewline

    # files for exclude settings (applied later in I-07; first snap keeps them for I-04/I-05)
    Set-Content -Path (Join-Path $tree "Thumbs.db") -Value "thumb" -NoNewline
    Set-Content -Path (Join-Path $tree "noise.log") -Value "logdata" -NoNewline
    Set-Content -Path (Join-Path $tree "keep.txt") -Value "keep" -NoNewline

    # --- I-02 snap / log / restore ---
    if ((Invoke-Snapline --target $tree snap -m "e2e-main") -ne 0) { Fail "I-02" "snap failed" }
    $logLines = @(Invoke-SnaplineOutput --target $tree log | Where-Object { $_ -match "\sentries\s" })
    if ($LASTEXITCODE -ne 0) { Fail "I-02" "log failed" }
    if ($logLines.Count -lt 1) { Fail "I-02" "no log rows" }
    $shortId = (($logLines | Select-Object -Last 1) -split '\s+')[0]
    if ($shortId.Length -ne 12) { Fail "I-02" "short id length=$($shortId.Length) line=$($logLines | Select-Object -Last 1)" }
    $restored = Join-Path $root "restored-main"
    if ((Invoke-Snapline --target $tree restore $shortId $restored) -ne 0) { Fail "I-02" "restore failed" }
    $got = Get-Content -Raw (Join-Path $restored "project\src\hello.txt")
    if ($got -ne "hello-snapline") { Fail "I-02" "content mismatch: $got" }
    Pass "I-02" "snap/log/restore ok shortId=$shortId"

    # --- I-04 gitignore ignored ---
    if (-not (Test-Path (Join-Path $restored "secret.env"))) {
        Fail "I-04" "secret.env missing after restore"
    }
    Pass "I-04" ".gitignore did not exclude secret.env"

    # --- I-05 child .snapline included ---
    if (-not (Test-Path (Join-Path $restored "app\.snapline\child-marker.txt"))) {
        Fail "I-05" "child .snapline not restored"
    }
    Pass "I-05" "child .snapline included in parent snap"

    # --- I-06 parent store excluded ---
    if (Test-Path (Join-Path $restored ".snapline\config.json")) {
        Fail "I-06" "parent store leaked into restore"
    }
    if (Test-Path (Join-Path $restored ".snapline\objects")) {
        Fail "I-06" "parent objects leaked into restore"
    }
    Pass "I-06" "parent store excluded from snap"

    # --- I-03 nested cwd discovery ---
    Push-Location $nested
    try {
        $fromNested = Invoke-SnaplineOutput log
        if ($LASTEXITCODE -ne 0) { Fail "I-03" "log from nested dir failed" }
        if (-not ($fromNested -match $shortId)) { Fail "I-03" "expected snap missing" }
        Pass "I-03" "discovered store from subdirectory"
    } finally {
        Pop-Location
    }

    # --- I-07 file/ext excludes ---
    $configPath = Join-Path $tree ".snapline\config.json"
    $config = Get-Content -Raw $configPath | ConvertFrom-Json
    $config.settings.exclude_file_names = @("Thumbs.db")
    $config.settings.exclude_extensions = @(".log")
    $json = $config | ConvertTo-Json -Depth 8
    [System.IO.File]::WriteAllText($configPath, $json)
    if ((Invoke-Snapline --target $tree snap -m "e2e-exclude") -ne 0) { Fail "I-07" "snap after exclude config failed" }
    $log2 = Invoke-SnaplineOutput --target $tree log
    $short2 = (($log2 | Select-Object -Last 1) -split '\s+')[0]
    $restored2 = Join-Path $root "restored-exclude"
    if ((Invoke-Snapline --target $tree restore $short2 $restored2) -ne 0) { Fail "I-07" "restore after exclude failed" }
    if (Test-Path (Join-Path $restored2 "Thumbs.db")) { Fail "I-07" "Thumbs.db was restored" }
    if (Test-Path (Join-Path $restored2 "noise.log")) { Fail "I-07" "noise.log was restored" }
    if (-not (Test-Path (Join-Path $restored2 "keep.txt"))) { Fail "I-07" "keep.txt missing" }
    Pass "I-07" "file name and extension excludes applied"

    # --- I-08 external store ---
    $extTree = Join-Path $root "ext-tree"
    $extStoreParent = Join-Path $root "ext-store-parent"
    New-Item -ItemType Directory -Path $extTree, $extStoreParent | Out-Null
    Set-Content -Path (Join-Path $extTree "a.txt") -Value "external" -NoNewline
    if ((Invoke-Snapline --target $extTree --store $extStoreParent init) -ne 0) { Fail "I-08" "external init failed" }
    $marker = Join-Path $extTree ".snapline"
    if ((Get-Item $marker).PSIsContainer) { Fail "I-08" "tree marker should be pointer file" }
    if (-not (Test-Path (Join-Path $extStoreParent ".snapline\config.json"))) {
        Fail "I-08" "external store body missing"
    }
    if ((Invoke-Snapline --target $extTree snap -m "ext") -ne 0) { Fail "I-08" "external snap failed" }
    if ((Invoke-Snapline --target $extTree log) -ne 0) { Fail "I-08" "external log failed" }
    Pass "I-08" "external store with pointer works"

    # --- I-09 non-empty restore destination ---
    $busy = Join-Path $root "busy-dest"
    New-Item -ItemType Directory -Path $busy | Out-Null
    Set-Content -Path (Join-Path $busy "x.txt") -Value "x"
    $code = Invoke-SnaplineExpectFail --target $tree restore $shortId $busy
    if ($code -eq 0) { Fail "I-09" "restore into non-empty dir succeeded" }
    Pass "I-09" "non-empty restore destination rejected"

    # --- I-10 missing short id ---
    $code = Invoke-SnaplineExpectFail --target $tree restore deadbeefdead (Join-Path $root "missing-id-dest")
    if ($code -eq 0) { Fail "I-10" "missing id restore succeeded" }
    Pass "I-10" "missing short id rejected"

    # --- I-11 verify ---
    $verifyOut = Invoke-SnaplineOutput --target $tree verify
    if ($LASTEXITCODE -ne 0) { Fail "I-11" "verify failed" }
    if (-not ($verifyOut -match "verified")) { Fail "I-11" "unexpected verify output: $verifyOut" }
    Pass "I-11" "$verifyOut"

    # --- I-21 snap (raw) then care (compress) ---
    $careTree = Join-Path $root "care-tree"
    New-Item -ItemType Directory -Path $careTree | Out-Null
    if ((Invoke-Snapline init $careTree) -ne 0) { Fail "I-21" "care-tree init failed" }
    $repeat = New-Object char[] 65536
    for ($i = 0; $i -lt $repeat.Length; $i++) { $repeat[$i] = [char]'A' }
    [IO.File]::WriteAllText((Join-Path $careTree "compress-me.txt"), (-join $repeat))
    if ((Invoke-Snapline --target $careTree snap -m "before-care") -ne 0) { Fail "I-21" "snap before care failed" }
    $careManifests = @(Get-ChildItem -Path (Join-Path $careTree ".snapline\snapshots") -Filter "*.json" -File | Sort-Object Name)
    if ($careManifests.Count -lt 1) { Fail "I-21" "no manifest after snap" }
    $careManifest = Get-Content -Raw $careManifests[-1].FullName | ConvertFrom-Json
    $careEntry = @($careManifest.entries | Where-Object { $_.path -match "compress-me" })[0]
    if ($null -eq $careEntry -or [string]::IsNullOrWhiteSpace($careEntry.object)) {
        Fail "I-21" "compress-me entry missing"
    }
    $careHash = [string]$careEntry.object
    $careObject = Join-Path $careTree (".snapline\objects\{0}\{1}" -f $careHash.Substring(0, 2), $careHash.Substring(2))
    $rawBytes = [IO.File]::ReadAllBytes($careObject)
    if ($rawBytes.Length -lt 10) { Fail "I-21" "object too short before care" }
    if ($rawBytes[8] -ne 0) { Fail "I-21" "expected raw codec before care, got $($rawBytes[8])" }
    $careOut = Invoke-SnaplineOutput --target $careTree care
    if ($LASTEXITCODE -ne 0) { Fail "I-21" "care failed: $careOut" }
    if (-not ($careOut -match "compressed")) { Fail "I-21" "unexpected care output: $careOut" }
    $zstdBytes = [IO.File]::ReadAllBytes($careObject)
    if ($zstdBytes[8] -ne 1) { Fail "I-21" "expected zstd codec after care, got $($zstdBytes[8])" }
    if ($zstdBytes.Length -ge $rawBytes.Length) { Fail "I-21" "object did not shrink after care" }
    Pass "I-21" "snap raw then care compressed"

    # --- I-12 --background both positions ---
    if ((Invoke-Snapline --target $tree --background snap -m "bg-before") -ne 0) { Fail "I-12" "--background before snap failed" }
    if ((Invoke-Snapline --target $tree snap --background -m "bg-after") -ne 0) { Fail "I-12" "snap --background failed" }
    Pass "I-12" "both --background positions work"

    # --- I-13 --background on log ---
    $code = Invoke-SnaplineExpectFail --target $tree --background log
    if ($code -eq 0) { Fail "I-13" "--background log succeeded" }
    Pass "I-13" "--background log rejected"

    # --- I-16 background verify ---
    if ((Invoke-Snapline --target $tree --background verify) -ne 0) { Fail "I-16" "--background verify failed" }
    Pass "I-16" "--background verify ok"

    # --- I-17 background restore ---
    $bgRestore = Join-Path $root "restored-background"
    if ((Invoke-Snapline --target $tree --background restore $shortId $bgRestore) -ne 0) { Fail "I-17" "--background restore failed" }
    if (-not (Test-Path (Join-Path $bgRestore "project\src\hello.txt"))) {
        Fail "I-17" "background restore missing content"
    }
    Pass "I-17" "--background restore ok"

    # --- I-18 background snap under CPU load (high threshold) ---
    $cpuJob = Start-Job -ScriptBlock {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.Elapsed.TotalSeconds -lt 20) {
            [void](1..5000 | ForEach-Object { $_ * $_ })
        }
    }
    try {
        if ((Invoke-Snapline --target $tree --background --cpu-busy-percent 99 snap -m "bg-under-load") -ne 0) {
            Fail "I-18" "--background snap under load failed"
        }
        Pass "I-18" "--background snap under CPU load ok"
    } finally {
        Stop-Job $cpuJob -ErrorAction SilentlyContinue | Out-Null
        Remove-Job $cpuJob -Force -ErrorAction SilentlyContinue | Out-Null
    }

    # --- I-19 summary sidecar written on snap ---
    $snapDir = Join-Path $tree ".snapline\snapshots"
    $sumDir = Join-Path $tree ".snapline\summaries"
    $manifestFiles = @(Get-ChildItem -Path $snapDir -Filter "*.json" -File)
    if ($manifestFiles.Count -lt 1) { Fail "I-19" "no manifests found" }
    foreach ($mf in $manifestFiles) {
        $summaryPath = Join-Path $sumDir $mf.Name
        if (-not (Test-Path $summaryPath)) { Fail "I-19" "missing summary for $($mf.Name)" }
        $summary = Get-Content -Raw $summaryPath | ConvertFrom-Json
        if ($null -eq $summary.entry_count) { Fail "I-19" "entry_count missing in $($mf.Name)" }
    }
    Pass "I-19" "summaries present for $($manifestFiles.Count) snapshots"

    # --- I-20 migrate missing summaries + log -1 ---
    Remove-Item -Recurse -Force $sumDir
    $migrateLog = Invoke-SnaplineOutput --target $tree log
    if ($LASTEXITCODE -ne 0) { Fail "I-20" "log after summary delete failed" }
    if (-not (Test-Path $sumDir)) { Fail "I-20" "summaries dir not recreated" }
    $rebuilt = @(Get-ChildItem -Path $sumDir -Filter "*.json" -File)
    if ($rebuilt.Count -ne $manifestFiles.Count) {
        Fail "I-20" "summary count=$($rebuilt.Count) expected=$($manifestFiles.Count)"
    }
    if ((Invoke-Snapline --target $tree snap -m "for-log-minus-one") -ne 0) {
        Fail "I-20" "extra snap for log -1 failed"
    }
    $onlyLatest = @(Invoke-SnaplineOutput --target $tree log -1)
    if ($LASTEXITCODE -ne 0) { Fail "I-20" "log -1 failed" }
    $latestLines = @($onlyLatest | Where-Object { $_ -match "entries" })
    if ($latestLines.Count -ne 1) { Fail "I-20" "log -1 returned $($latestLines.Count) lines" }
    if (-not ($latestLines[0] -match "for-log-minus-one")) {
        Fail "I-20" "log -1 did not show newest message: $($latestLines[0])"
    }
    Pass "I-20" "summary migration and log -1 ok"

    # --- I-22 tree / find / restore --path / --dry-run ---
    $treeOut = Invoke-SnaplineOutput --target $tree tree $shortId --path project
    if ($LASTEXITCODE -ne 0) { Fail "I-22" "tree failed" }
    if (-not ($treeOut -match "project")) { Fail "I-22" "tree missing project: $treeOut" }
    $findOut = Invoke-SnaplineOutput --target $tree find $shortId hello
    if ($LASTEXITCODE -ne 0) { Fail "I-22" "find failed" }
    if (-not ($findOut -match "hello")) { Fail "I-22" "find missing hello: $findOut" }
    $partialDest = Join-Path $root "restored-partial"
    $dryOut = Invoke-SnaplineOutput --target $tree restore $shortId $partialDest --path project --dry-run
    if ($LASTEXITCODE -ne 0) { Fail "I-22" "dry-run failed: $dryOut" }
    if (-not ($dryOut -match "dry-run")) { Fail "I-22" "dry-run output missing: $dryOut" }
    if (Test-Path $partialDest) { Fail "I-22" "dry-run created destination" }
    if ((Invoke-Snapline --target $tree restore $shortId $partialDest --path project) -ne 0) {
        Fail "I-22" "partial restore failed"
    }
    if (-not (Test-Path (Join-Path $partialDest "project\src\hello.txt"))) {
        Fail "I-22" "partial restore missing project file"
    }
    if (Test-Path (Join-Path $partialDest "secret.env")) {
        Fail "I-22" "partial restore leaked root file"
    }
    Pass "I-22" "tree/find/partial restore ok"

    # --- I-23 init --config-only / --force ---
    $configPath = Join-Path $tree ".snapline\config.json"
    $code = Invoke-SnaplineExpectFail --target $tree init --config-only
    if ($code -eq 0) { Fail "I-23" "config-only without --force succeeded" }
    if ((Invoke-Snapline --target $tree init --config-only --force) -ne 0) {
        Fail "I-23" "config-only --force failed"
    }
    if (-not (Test-Path $configPath)) { Fail "I-23" "config.json missing after force" }
    Remove-Item -Force $configPath
    if ((Invoke-Snapline --target $tree init --config-only) -ne 0) {
        Fail "I-23" "config-only recreate failed"
    }
    if (-not (Test-Path $configPath)) { Fail "I-23" "config.json missing after recreate" }
    if ((Invoke-Snapline --target $tree log) -ne 0) { Fail "I-23" "log after config-only failed" }
    Pass "I-23" "init --config-only / --force ok"

    Write-Host ""
    Write-Host "All E2E cases passed ($($results.Count))"
    $results | Format-Table -AutoSize | Out-String | Write-Host
}
finally {
    if (Test-Path $root) {
        Remove-Item -Recurse -Force $root
    }
}
