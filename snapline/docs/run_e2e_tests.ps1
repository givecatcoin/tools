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
function Invoke-SnaplineExpectFail {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$CliArgs)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $exe @CliArgs 2>$null | Out-Null
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
    }
}

try {
    # --- I-01 init ---
    $tree = Join-Path $root "tree"
    New-Item -ItemType Directory -Path $tree | Out-Null
    & $exe init $tree | Out-Null
    if (-not (Test-Path (Join-Path $tree ".snapline\config.json"))) {
        Fail "I-01" "config.json missing"
    }
    Pass "I-01" "init created local store"

    # --- I-15 no auto-init ---
    $bare = Join-Path $root "bare"
    New-Item -ItemType Directory -Path $bare | Out-Null
    $code = Invoke-SnaplineExpectFail --tree $bare snapshot
    if ($code -eq 0) { Fail "I-15" "snapshot succeeded without init" }
    if (Test-Path (Join-Path $bare ".snapline")) { Fail "I-15" "store was created unexpectedly" }
    Pass "I-15" "snapshot without init fails and creates nothing"

    # --- I-14 double init ---
    $code = Invoke-SnaplineExpectFail init $tree
    if ($code -eq 0) { Fail "I-14" "second init succeeded" }
    Pass "I-14" "second init rejected"

    # --- content for snapshot ---
    $nested = Join-Path $tree "project\src"
    New-Item -ItemType Directory -Path $nested -Force | Out-Null
    Set-Content -Path (Join-Path $nested "hello.txt") -Value "hello-snapline" -NoNewline
    Set-Content -Path (Join-Path $tree "secret.env") -Value "TOKEN=keep-me" -NoNewline
    Set-Content -Path (Join-Path $tree ".gitignore") -Value "secret.env`n" -NoNewline

    # child .snapline (nested store content)
    $childStore = Join-Path $tree "app\.snapline"
    New-Item -ItemType Directory -Path $childStore -Force | Out-Null
    Set-Content -Path (Join-Path $childStore "child-marker.txt") -Value "child-store" -NoNewline

    # files for exclude settings (applied later in I-07; first snapshot keeps them for I-04/I-05)
    Set-Content -Path (Join-Path $tree "Thumbs.db") -Value "thumb" -NoNewline
    Set-Content -Path (Join-Path $tree "noise.log") -Value "logdata" -NoNewline
    Set-Content -Path (Join-Path $tree "keep.txt") -Value "keep" -NoNewline

    # --- I-02 snapshot / log / restore ---
    & $exe --tree $tree snapshot -m "e2e-main" | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "I-02" "snapshot failed" }
    $logLines = & $exe --tree $tree log
    if ($LASTEXITCODE -ne 0) { Fail "I-02" "log failed" }
    $shortId = (($logLines | Select-Object -Last 1) -split '\s+')[0]
    if ($shortId.Length -ne 12) { Fail "I-02" "short id length=$($shortId.Length)" }
    $restored = Join-Path $root "restored-main"
    & $exe --tree $tree restore $shortId $restored | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "I-02" "restore failed" }
    $got = Get-Content -Raw (Join-Path $restored "project\src\hello.txt")
    if ($got -ne "hello-snapline") { Fail "I-02" "content mismatch: $got" }
    Pass "I-02" "snapshot/log/restore ok shortId=$shortId"

    # --- I-04 gitignore ignored ---
    if (-not (Test-Path (Join-Path $restored "secret.env"))) {
        Fail "I-04" "secret.env missing after restore"
    }
    Pass "I-04" ".gitignore did not exclude secret.env"

    # --- I-05 child .snapline included ---
    if (-not (Test-Path (Join-Path $restored "app\.snapline\child-marker.txt"))) {
        Fail "I-05" "child .snapline not restored"
    }
    Pass "I-05" "child .snapline included in parent snapshot"

    # --- I-06 parent store excluded ---
    if (Test-Path (Join-Path $restored ".snapline\config.json")) {
        Fail "I-06" "parent store leaked into restore"
    }
    if (Test-Path (Join-Path $restored ".snapline\objects")) {
        Fail "I-06" "parent objects leaked into restore"
    }
    Pass "I-06" "parent store excluded from snapshot"

    # --- I-03 nested cwd discovery ---
    Push-Location $nested
    try {
        $fromNested = & $exe log
        if ($LASTEXITCODE -ne 0) { Fail "I-03" "log from nested dir failed" }
        if (-not ($fromNested -match $shortId)) { Fail "I-03" "expected snapshot missing" }
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
    & $exe --tree $tree snapshot -m "e2e-exclude" | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "I-07" "snapshot after exclude config failed" }
    $log2 = & $exe --tree $tree log
    $short2 = (($log2 | Select-Object -Last 1) -split '\s+')[0]
    $restored2 = Join-Path $root "restored-exclude"
    & $exe --tree $tree restore $short2 $restored2 | Out-Null
    if (Test-Path (Join-Path $restored2 "Thumbs.db")) { Fail "I-07" "Thumbs.db was restored" }
    if (Test-Path (Join-Path $restored2 "noise.log")) { Fail "I-07" "noise.log was restored" }
    if (-not (Test-Path (Join-Path $restored2 "keep.txt"))) { Fail "I-07" "keep.txt missing" }
    Pass "I-07" "file name and extension excludes applied"

    # --- I-08 external store ---
    $extTree = Join-Path $root "ext-tree"
    $extStoreParent = Join-Path $root "ext-store-parent"
    New-Item -ItemType Directory -Path $extTree, $extStoreParent | Out-Null
    Set-Content -Path (Join-Path $extTree "a.txt") -Value "external" -NoNewline
    & $exe --tree $extTree --store $extStoreParent init | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "I-08" "external init failed" }
    $marker = Join-Path $extTree ".snapline"
    if ((Get-Item $marker).PSIsContainer) { Fail "I-08" "tree marker should be pointer file" }
    if (-not (Test-Path (Join-Path $extStoreParent ".snapline\config.json"))) {
        Fail "I-08" "external store body missing"
    }
    & $exe --tree $extTree snapshot -m "ext" | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "I-08" "external snapshot failed" }
    & $exe --tree $extTree log | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "I-08" "external log failed" }
    Pass "I-08" "external store with pointer works"

    # --- I-09 non-empty restore destination ---
    $busy = Join-Path $root "busy-dest"
    New-Item -ItemType Directory -Path $busy | Out-Null
    Set-Content -Path (Join-Path $busy "x.txt") -Value "x"
    $code = Invoke-SnaplineExpectFail --tree $tree restore $shortId $busy
    if ($code -eq 0) { Fail "I-09" "restore into non-empty dir succeeded" }
    Pass "I-09" "non-empty restore destination rejected"

    # --- I-10 missing short id ---
    $code = Invoke-SnaplineExpectFail --tree $tree restore deadbeefdead (Join-Path $root "missing-id-dest")
    if ($code -eq 0) { Fail "I-10" "missing id restore succeeded" }
    Pass "I-10" "missing short id rejected"

    # --- I-11 verify ---
    $verifyOut = & $exe --tree $tree verify
    if ($LASTEXITCODE -ne 0) { Fail "I-11" "verify failed" }
    if (-not ($verifyOut -match "verified")) { Fail "I-11" "unexpected verify output: $verifyOut" }
    Pass "I-11" "$verifyOut"

    # --- I-12 --background both positions ---
    & $exe --tree $tree --background snapshot -m "bg-before" | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "I-12" "--background before snapshot failed" }
    & $exe --tree $tree snapshot --background -m "bg-after" | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "I-12" "snapshot --background failed" }
    Pass "I-12" "both --background positions work"

    # --- I-13 --background on log ---
    $code = Invoke-SnaplineExpectFail --tree $tree --background log
    if ($code -eq 0) { Fail "I-13" "--background log succeeded" }
    Pass "I-13" "--background log rejected"

    Write-Host ""
    Write-Host "All E2E cases passed ($($results.Count))"
    $results | Format-Table -AutoSize | Out-String | Write-Host
}
finally {
    if (Test-Path $root) {
        Remove-Item -Recurse -Force $root
    }
}
