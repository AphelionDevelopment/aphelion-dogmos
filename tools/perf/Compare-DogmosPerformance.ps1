[CmdletBinding()]
param(
	[string]$BaselinePath,
	[string]$CurrentPath,
	[string]$BudgetPath,
	[switch]$SelfTestIdentityMismatch
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if(-not $BudgetPath) {
	$BudgetPath = Join-Path $PSScriptRoot '..\..\docs\performance\budget.toml'
}

function Compare-DogmosIdentity {
	param(
		[Parameter(Mandatory)]$Baseline,
		[Parameter(Mandatory)]$Current
	)
	$fields = @('map', 'seed', 'revision', 'features', 'byond_version', 'duration_seconds', 'scenario_sha256')
	$mismatches = @()
	foreach($field in $fields) {
		$left = $Baseline.PSObject.Properties[$field].Value | ConvertTo-Json -Compress
		$right = $Current.PSObject.Properties[$field].Value | ConvertTo-Json -Compress
		if($left -cne $right) { $mismatches += $field }
	}
	[pscustomobject]@{
		comparable = $mismatches.Count -eq 0
		mismatches = $mismatches
	}
}

if($SelfTestIdentityMismatch) {
	$baseline = [pscustomobject]@{
		map = 'MetaStation.dmm'; seed = 1; revision = 'a'; features = @('default')
		byond_version = '516.1685'; duration_seconds = 60; scenario_sha256 = ('a' * 64)
	}
	$current = [pscustomobject]@{
		map = 'IceBoxStation.dmm'; seed = 1; revision = 'a'; features = @('default')
		byond_version = '516.1685'; duration_seconds = 60; scenario_sha256 = ('b' * 64)
	}
	Compare-DogmosIdentity -Baseline $baseline -Current $current | ConvertTo-Json -Compress
	exit 0
}

if(-not $BaselinePath -or -not $CurrentPath) {
	throw '-BaselinePath and -CurrentPath are required.'
}
$baseline = Get-Content -LiteralPath (Resolve-Path -LiteralPath $BaselinePath) -Raw | ConvertFrom-Json
$current = Get-Content -LiteralPath (Resolve-Path -LiteralPath $CurrentPath) -Raw | ConvertFrom-Json
$identityResult = Compare-DogmosIdentity -Baseline $baseline.identity -Current $current.identity
if(-not $identityResult.comparable) {
	$identityResult | ConvertTo-Json -Compress
	exit 2
}

function Percent-Delta([double]$Before, [double]$After) {
	if($Before -eq 0) { return $null }
	return (($After - $Before) / $Before) * 100
}

function Get-TomlNumber {
	param([Parameter(Mandatory)][string]$Document, [Parameter(Mandatory)][string]$Key)
	$match = [regex]::Match($Document, "(?m)^\s*$([regex]::Escape($Key))\s*=\s*([-+]?[0-9]+(?:\.[0-9]+)?)\s*(?:#.*)?$")
	if(-not $match.Success) { throw "Budget '$BudgetPath' is missing numeric key '$Key'." }
	return [double]::Parse($match.Groups[1].Value, [Globalization.CultureInfo]::InvariantCulture)
}

$budgetDocument = Get-Content -LiteralPath (Resolve-Path -LiteralPath $BudgetPath) -Raw
$memoryReductionTarget = Get-TomlNumber -Document $budgetDocument -Key 'dreamdaemon_reduction_target_percent'
$p95MinimumAllowance = Get-TomlNumber -Document $budgetDocument -Key 'p95_minimum_allowance_percent'
$p99MinimumAllowance = Get-TomlNumber -Document $budgetDocument -Key 'p99_minimum_allowance_percent'
$noiseMultiplier = Get-TomlNumber -Document $budgetDocument -Key 'noise_multiplier'
$controlNoiseP95 = Get-TomlNumber -Document $budgetDocument -Key 'p95_percent'
$controlNoiseP99 = Get-TomlNumber -Document $budgetDocument -Key 'p99_percent'
$p95Allowance = [math]::Max($p95MinimumAllowance, $controlNoiseP95 * $noiseMultiplier)
$p99Allowance = [math]::Max($p99MinimumAllowance, $controlNoiseP99 * $noiseMultiplier)

$dreamdaemonDelta = Percent-Delta $baseline.summary.dreamdaemon_private_bytes $current.summary.dreamdaemon_private_bytes
$serverDelta = Percent-Delta $baseline.summary.server_private_bytes $current.summary.server_private_bytes
$p95Delta = Percent-Delta $baseline.summary.server_tick_p95_ns $current.summary.server_tick_p95_ns
$p99Delta = Percent-Delta $baseline.summary.server_tick_p99_ns $current.summary.server_tick_p99_ns
$memoryReduction = if($null -eq $dreamdaemonDelta) { $null } else { -$dreamdaemonDelta }
$memoryPassed = $null -ne $memoryReduction -and $memoryReduction -ge $memoryReductionTarget
$p95Passed = $null -ne $p95Delta -and $p95Delta -le $p95Allowance
$p99Passed = $null -ne $p99Delta -and $p99Delta -le $p99Allowance

$result = [ordered]@{
	comparable = $true
	mismatches = @()
	acceptance_passed = $memoryPassed -and $p95Passed -and $p99Passed
	dreamdaemon_private_bytes_delta_percent = $dreamdaemonDelta
	server_private_bytes_delta_percent = $serverDelta
	server_memory_is_separate = $true
	server_memory_is_in_dreamdaemon_total = $false
	server_tick_p95_delta_percent = $p95Delta
	server_tick_p99_delta_percent = $p99Delta
	gates = [ordered]@{
		dreamdaemon_private_bytes = [ordered]@{
			reduction_percent = $memoryReduction
			required_reduction_percent = $memoryReductionTarget
			passed = $memoryPassed
		}
		server_tick_p95 = [ordered]@{
			delta_percent = $p95Delta
			maximum_delta_percent = $p95Allowance
			passed = $p95Passed
		}
		server_tick_p99 = [ordered]@{
			delta_percent = $p99Delta
			maximum_delta_percent = $p99Allowance
			passed = $p99Passed
		}
	}
}
$result | ConvertTo-Json -Compress
if(-not $result.acceptance_passed) { exit 3 }
