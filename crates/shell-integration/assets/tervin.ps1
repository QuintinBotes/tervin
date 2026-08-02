# Tervin shell integration — PowerShell
#
# PowerShell has no preexec hook, so the command line is read from history when
# the next prompt is drawn. That means the command and its result are reported
# together at completion rather than at submission: Blocks are still exact, they
# simply appear when the command finishes.
#
# Disable with: $env:TERVIN_SHELL_INTEGRATION = '0'

if ($env:TERM_PROGRAM -ne 'Tervin') { return }
if ($env:TERVIN_SHELL_INTEGRATION -eq '0') { return }
if ($global:__TervinLoaded) { return }
$global:__TervinLoaded = $true
$global:__TervinLastHistoryId = -1

function global:__Tervin-Osc([string]$Payload) {
    $esc = [char]27
    $bel = [char]7
    [Console]::Write("$esc]$Payload$bel")
}

function global:__Tervin-B64([string]$Value) {
    if ([string]::IsNullOrEmpty($Value)) { return '' }
    [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Value))
}

function global:__Tervin-ReportCwd() {
    # Only filesystem locations are meaningful as a cwd; registry and other
    # providers are skipped rather than reported as paths.
    if ($ExecutionContext.SessionState.Path.CurrentLocation.Provider.Name -ne 'FileSystem') { return }
    $p = $ExecutionContext.SessionState.Path.CurrentLocation.ProviderPath -replace '\\', '/'
    if ($p -notmatch '^/') { $p = "/$p" }
    __Tervin-Osc "7;file://$([System.Net.Dns]::GetHostName())$p"
}

if (-not (Get-Command __Tervin-OriginalPrompt -ErrorAction SilentlyContinue)) {
    # Preserve whatever prompt the user already had.
    $existing = (Get-Command prompt -ErrorAction SilentlyContinue).ScriptBlock
    if ($existing) {
        Set-Item -Path function:global:__Tervin-OriginalPrompt -Value $existing
    } else {
        function global:__Tervin-OriginalPrompt { "PS $($ExecutionContext.SessionState.Path.CurrentLocation)> " }
    }

    function global:prompt {
        $exitCode = if ($global:LASTEXITCODE -ne $null) { $global:LASTEXITCODE } elseif ($?) { 0 } else { 1 }

        $last = Get-History -Count 1 -ErrorAction SilentlyContinue
        if ($last -and $last.Id -ne $global:__TervinLastHistoryId) {
            $global:__TervinLastHistoryId = $last.Id
            $cmd = __Tervin-B64 $last.CommandLine
            __Tervin-Osc "7373;cmd=$cmd;shell=powershell"
            __Tervin-Osc "133;C"
            __Tervin-Osc "133;D;$exitCode"
        }

        __Tervin-ReportCwd
        __Tervin-Osc "133;A"
        $rendered = __Tervin-OriginalPrompt
        __Tervin-Osc "133;B"
        return $rendered
    }
}

__Tervin-ReportCwd
__Tervin-Osc "133;A"
