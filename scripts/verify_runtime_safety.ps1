[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# This source-based policy blocks known unsafe patterns. Behavioral tests remain
# the authority for lock ownership, cancellation and external-input handling.
$script:RuleIds = @{
    CloneProjection = "RUNTIME-HOT-CLONE"
    ChannelWorker   = "RUNTIME-CHANNEL-THREAD"
    ReplayBudget    = "RUNTIME-REPLAY-BUDGET"
    SourceTest      = "RUNTIME-SOURCE-TEST"
}

function ConvertTo-RelativePath {
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    $fullPath = [IO.Path]::GetFullPath($Path)
    $root = [IO.Path]::GetFullPath((Get-Location).Path)
    return ([IO.Path]::GetRelativePath($root, $fullPath) -replace "\\", "/")
}

function New-Diagnostic {
    param(
        [Parameter(Mandatory)]
        [ValidateSet("Error", "Warning", "Info")]
        [string]$Severity,
        [Parameter(Mandatory)]
        [string]$Rule,
        [Parameter(Mandatory)]
        [string]$Path,
        [int]$Line = 0,
        [Parameter(Mandatory)]
        [string]$Message
    )

    return [pscustomobject]@{
        Severity = $Severity
        Rule     = $Rule
        Path     = $Path
        Line     = $Line
        Message  = $Message
    }
}

function Get-RepositoryFiles {
    param(
        [Parameter(Mandatory)]
        [string[]]$Roots,
        [Parameter(Mandatory)]
        [string[]]$Extensions
    )

    $files = @()
    foreach ($root in $Roots) {
        if (-not (Test-Path -LiteralPath $root -PathType Container)) {
            continue
        }
        $files += @(
            Get-ChildItem -LiteralPath $root -Recurse -File |
                Where-Object {
                    $Extensions -contains $_.Extension.ToLowerInvariant() -and
                    $_.FullName -notmatch "[\\/]target[\\/]" -and
                    $_.FullName -notmatch "[\\/]node_modules[\\/]" -and
                    $_.FullName -notmatch "[\\/]generated[\\/]"
                }
        )
    }
    return @($files | Sort-Object FullName -Unique)
}

function Get-NearestRustFunction {
    param(
        [Parameter(Mandatory)]
        [AllowEmptyCollection()]
        [AllowEmptyString()]
        [string[]]$Lines,
        [Parameter(Mandatory)]
        [int]$Index
    )

    for ($cursor = $Index; $cursor -ge 0; $cursor--) {
        if ($Lines[$cursor] -match "\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(") {
            $isTest = $false
            for ($attribute = [Math]::Max(0, $cursor - 4); $attribute -lt $cursor; $attribute++) {
                if ($Lines[$attribute] -match "#\s*\[\s*test\s*\]") {
                    $isTest = $true
                    break
                }
            }
            return [pscustomobject]@{
                Name    = $Matches[1]
                Start   = $cursor
                IsTest  = $isTest
            }
        }
    }
    return [pscustomobject]@{ Name = ""; Start = 0; IsTest = $false }
}


function Test-HotProjectionContext {
    param(
        [Parameter(Mandatory)]
        [string]$RelativePath,
        [Parameter(Mandatory)]
        [AllowEmptyString()]
        [string]$FunctionName
    )

    if ($RelativePath -match "^src-tauri/src/(channels|contract|commands)/") {
        return $true
    }
    if ($RelativePath -eq "src/core/live_capture.rs" -and $FunctionName -match "(?i)(process_event|queue_abyss_archive)") {
        return $true
    }
    return $FunctionName -match "(?i)(presented|projection|snapshot|stream|detail|main_dps)"
}

function Test-HotCloneFingerprint {
    param(
        [Parameter(Mandatory)]
        [string]$Window,
        [Parameter(Mandatory)]
        [AllowEmptyString()]
        [string]$FunctionName
    )

    if ($Window -match "with_state\s*\(\s*Clone\s*::\s*clone\s*\)") {
        return $true
    }
    if ($FunctionName -match "(?i)main_round" -and $Window -match "\brecords\.clone\s*\(\)") {
        return $true
    }
    return $FunctionName -match "(?i)(process_event|queue_abyss_archive)" -and
        $Window -match "HistoryCombatDetails::from_state\s*\("
}

function Find-HotCloneOccurrences {
    param(
        [Parameter(Mandatory)]
        [System.IO.FileInfo[]]$Files
    )

    $occurrences = @()
    $seen = @{}
    foreach ($file in $Files) {
        $relative = ConvertTo-RelativePath $file.FullName
        if ($relative -notmatch "^(src-tauri/src/.*\.rs|src/core/live_capture\.rs)$") {
            continue
        }
        $lines = @(Get-Content -LiteralPath $file.FullName)
        for ($index = 0; $index -lt $lines.Count; $index++) {
            $windowEnd = [Math]::Min($lines.Count - 1, $index + 2)
            $window = ($lines[$index..$windowEnd] -join " ")
            $tokenLine = $index
            for ($candidate = $index; $candidate -le $windowEnd; $candidate++) {
                if ($lines[$candidate] -match "with_state|records\.clone|HistoryCombatDetails::from_state") {
                    $tokenLine = $candidate
                    break
                }
            }
            $function = Get-NearestRustFunction -Lines $lines -Index $index
            if (-not (Test-HotProjectionContext $relative $function.Name)) {
                continue
            }
            if (-not (Test-HotCloneFingerprint $window $function.Name)) {
                continue
            }
            $key = "$relative|$($tokenLine + 1)|$($function.Name)"
            if ($seen.ContainsKey($key)) {
                continue
            }
            $seen[$key] = $true
            $occurrences += [pscustomobject]@{
                Path     = $relative
                Line     = $tokenLine + 1
                Function = $function.Name
                Text     = $lines[$tokenLine].Trim()
            }
        }
    }
    return @($occurrences)
}

function Find-ChannelThreadSleepOccurrences {
    param(
        [Parameter(Mandatory)]
        [System.IO.FileInfo[]]$Files
    )

    $occurrences = @()
    foreach ($file in $Files) {
        $relative = ConvertTo-RelativePath $file.FullName
        if ($relative -notmatch "^src-tauri/src/channels/.*\.rs$") {
            continue
        }
        $lines = @(Get-Content -LiteralPath $file.FullName)
        $spawn = @(
            for ($index = 0; $index -lt $lines.Count; $index++) {
                if ($lines[$index] -match "\bthread::spawn\s*\(") { $index + 1 }
            }
        )
        $sleep = @(
            for ($index = 0; $index -lt $lines.Count; $index++) {
                if ($lines[$index] -match "\bthread::sleep\s*\(") { $index + 1 }
            }
        )
        if ($spawn.Count -gt 0 -and $sleep.Count -gt 0) {
            $source = $lines -join "`n"
            $occurrences += [pscustomobject]@{
                Path        = $relative
                SpawnCount  = $spawn.Count
                SleepCount  = $sleep.Count
                FirstSpawn  = $spawn[0]
                FirstSleep  = $sleep[0]
                SpawnLines  = @($spawn)
                SleepLines  = @($sleep)
                OwnerCancellation = $source -match "begin_stream" -and
                    $source -match "finish_stream" -and
                    $source -match "while\s*(?:!stop\.load|\(\s*!stop\.load)"
            }
        }
    }
    return @($occurrences)
}

function Test-ReplayImportValidation {
    param(
        [Parameter(Mandatory)]
        [AllowEmptyCollection()]
        [AllowEmptyString()]
        [string[]]$Lines,
        [Parameter(Mandatory)]
        [int]$ReadIndex,
        [Parameter(Mandatory)]
        [int]$FunctionStart
    )

    if ($ReadIndex -le $FunctionStart) {
        return $false
    }
    $prefix = ($Lines[$FunctionStart..($ReadIndex - 1)] -join "`n")
    return $prefix -match "(?i)(validate_[A-Za-z0-9_]*(import|size|budget|path)|file_size|MAX_[A-Z0-9_]*(IMPORT|REPLAY|SIZE)|metadata[\s\S]*?\.len\s*\(\))"
}

function Find-ReplayBudgetViolations {
    param(
        [Parameter(Mandatory)]
        [System.IO.FileInfo[]]$Files
    )

    $violations = @()
    foreach ($file in $Files) {
        $relative = ConvertTo-RelativePath $file.FullName
        if ($relative -notmatch "^src/(engine|core|storage)/.*\.rs$") {
            continue
        }
        $lines = @(Get-Content -LiteralPath $file.FullName)
        for ($index = 0; $index -lt $lines.Count; $index++) {
            if ($lines[$index] -notmatch "(?:std::fs::|fs::)?(?:read_to_string|read_to_end|read)\s*\(") {
                continue
            }
            $function = Get-NearestRustFunction -Lines $lines -Index $index
            $isReplayFunction = $function.Name -match "(?i)(import|replay|capture_json)"
            if (-not $isReplayFunction -or $function.IsTest) {
                continue
            }
            if (-not (Test-ReplayImportValidation -Lines $lines -ReadIndex $index -FunctionStart $function.Start)) {
                $violations += New-Diagnostic `
                    -Severity Error `
                    -Rule $script:RuleIds['ReplayBudget'] `
                    -Path $relative `
                    -Line ($index + 1) `
                    -Message "Replay/import function '$($function.Name)' reads file bytes without a preceding metadata/size validation."
            }
        }
    }
    return @($violations)
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repositoryRoot
try {
    $rustFiles = Get-RepositoryFiles @("src", "src-tauri/src") @(".rs")
    $diagnostics = [System.Collections.Generic.List[object]]::new()

    foreach ($file in $rustFiles) {
        $path = ConvertTo-RelativePath $file.FullName
        $lines = @(Get-Content -LiteralPath $file.FullName)
        for ($index = 0; $index -lt $lines.Count; $index++) {
            if ($lines[$index] -match 'include_str!\s*\([^)]*\.rs["'']\s*\)') {
                $diagnostics.Add((New-Diagnostic `
                            -Severity Error `
                            -Rule $script:RuleIds['SourceTest'] `
                            -Path $path `
                            -Line ($index + 1) `
                            -Message "Rust tests must verify behavior; mechanical source policy belongs in this repository-level check."))
            }
        }
    }

    $hotOccurrences = @(Find-HotCloneOccurrences $rustFiles)
    foreach ($occurrence in $hotOccurrences) {
        $diagnostics.Add((New-Diagnostic `
                    -Severity Error `
                    -Rule $script:RuleIds['CloneProjection'] `
                    -Path $occurrence.Path `
                    -Line $occurrence.Line `
                    -Message "High-frequency projection '$($occurrence.Function)' contains a full-state clone."))
    }

    $channelOccurrences = @(Find-ChannelThreadSleepOccurrences $rustFiles)
    foreach ($occurrence in $channelOccurrences) {
        if ($occurrence.Path -ne "src-tauri/src/channels/history.rs") {
            $diagnostics.Add((New-Diagnostic -Severity Error -Rule $script:RuleIds['ChannelWorker'] -Path $occurrence.Path -Line $occurrence.FirstSpawn -Message "Polling Channel workers must use channels/stream_runtime.rs instead of copying thread::spawn + thread::sleep."))
        }
        elseif (-not $occurrence.OwnerCancellation) {
            $diagnostics.Add((New-Diagnostic -Severity Error -Rule $script:RuleIds['ChannelWorker'] -Path $occurrence.Path -Line $occurrence.FirstSpawn -Message "Channel worker uses thread::spawn + thread::sleep without an owner-bound stop token and cleanup path."))
        }
    }

    $replayViolations = @(Find-ReplayBudgetViolations $rustFiles)
    foreach ($diagnostic in $replayViolations) { $diagnostics.Add($diagnostic) }


    $ordered = @($diagnostics | Sort-Object Severity, Rule, Path, Line)
    foreach ($diagnostic in $ordered) {
        $location = if ($diagnostic.Line -gt 0) { "$($diagnostic.Path):$($diagnostic.Line)" } else { $diagnostic.Path }
        Write-Output ("RUNTIME-SAFETY [{0}] [{1}] {2} - {3}" -f $diagnostic.Severity.ToUpperInvariant(), $diagnostic.Rule, $location, $diagnostic.Message)
    }

    $errors = @($ordered | Where-Object Severity -eq "Error")
    if ($errors.Count -gt 0) {
        Write-Error "Runtime safety policy failed with $($errors.Count) blocking diagnostic(s)."
        exit 1
    }

    Write-Output "Runtime safety policy passed."
}
finally {
    Pop-Location
}
