[CmdletBinding()]
param(
	[int]$Cycles = 10000,
	[int]$HoldMilliseconds = 500,
	[long]$ServicePlateauBytes = 8388608,
	[long]$ServiceTailGrowthBytes = 4194304
)

$ErrorActionPreference = 'Stop'
$rustToolchain = '1.98.0'
$serverTarget = 'x86_64-pc-windows-msvc'
$shimTarget = 'i686-pc-windows-msvc'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$identityModule = Join-Path $PSScriptRoot 'DogmosBuildIdentity.psm1'
Import-Module $identityModule -Force
$buildIdentity = Get-DogmosBuildIdentity -RepositoryRoot $repositoryRoot -AllowDirty
$previousSourceRevision = [Environment]::GetEnvironmentVariable('DOGMOS_SOURCE_REVISION', 'Process')
$previousFeatureFingerprint = [Environment]::GetEnvironmentVariable('DOGMOS_FEATURE_FINGERPRINT', 'Process')
$env:DOGMOS_SOURCE_REVISION = $buildIdentity.source_revision
$env:DOGMOS_FEATURE_FINGERPRINT = $buildIdentity.feature_fingerprint
$outputDirectory = Join-Path $repositoryRoot 'tmp\dogmos-perf\ipc'
$outputPath = Join-Path $outputDirectory 'callback-pressure.csv'
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
'phase,role,process_id,private_bytes,virtual_bytes,working_set_bytes' | Set-Content -LiteralPath $outputPath

function Read-ProcessSample {
	param(
		[string]$Phase,
		[string]$Role,
		[int]$ProcessId
	)
	$process = Get-Process -Id $ProcessId
	"$Phase,$Role,$ProcessId,$($process.PrivateMemorySize64),$($process.VirtualMemorySize64),$($process.WorkingSet64)" |
		Add-Content -LiteralPath $outputPath
	return $process.PrivateMemorySize64
}

if ($Cycles -lt 100) {
	throw 'Cycles must be at least 100 so the warm-up and plateau checkpoints are distinct.'
}

$samples = [ordered]@{}
Push-Location $repositoryRoot
try {
	& cargo "+$rustToolchain" build -p dogmos-server --bin dogmosd --target $serverTarget --release --offline
	if ($LASTEXITCODE -ne 0) { throw "x64 dogmosd build failed with exit code $LASTEXITCODE" }
	& cargo "+$rustToolchain" build -p dogmos-byond --example cross_bitness_probe --target $shimTarget --release --offline
	if ($LASTEXITCODE -ne 0) { throw "i686 IPC probe build failed with exit code $LASTEXITCODE" }

	$serverPath = Join-Path $repositoryRoot "target\$serverTarget\release\dogmosd.exe"
	$probePath = Join-Path $repositoryRoot "target\$shimTarget\release\examples\cross_bitness_probe.exe"
	& $probePath $serverPath 0 0 $Cycles $HoldMilliseconds 2>&1 | ForEach-Object {
		Write-Output $_
		if ($_ -match '^callback_pressure_(warmup|quarter|midpoint|three_quarter|complete),shim_pid=(\d+),service_pid=(\d+)') {
			$phase = $Matches[1]
			$shimId = [int]$Matches[2]
			$serviceId = [int]$Matches[3]
			$samples[$phase] = [pscustomobject]@{
				Shim = Read-ProcessSample $phase 'shim' $shimId
				Service = Read-ProcessSample $phase 'service' $serviceId
			}
		}
	}
	if ($LASTEXITCODE -ne 0) { throw "callback pressure probe failed with exit code $LASTEXITCODE" }

	$expectedPhases = @('warmup', 'quarter', 'midpoint', 'three_quarter', 'complete')
	foreach ($phase in $expectedPhases) {
		if (-not $samples.Contains($phase)) { throw "callback pressure probe omitted the $phase marker" }
	}
	$serviceValues = @($expectedPhases | ForEach-Object { $samples[$_].Service })
	$shimValues = @($expectedPhases | ForEach-Object { $samples[$_].Shim })
	$serviceRange = ($serviceValues | Measure-Object -Maximum).Maximum - ($serviceValues | Measure-Object -Minimum).Minimum
	$serviceTailGrowth = $samples.complete.Service - $samples.midpoint.Service
	$shimRange = ($shimValues | Measure-Object -Maximum).Maximum - ($shimValues | Measure-Object -Minimum).Minimum
	if ($serviceRange -gt $ServicePlateauBytes) {
		throw "dogmosd private bytes varied by $serviceRange after warm-up, above $ServicePlateauBytes"
	}
	if ($serviceTailGrowth -gt $ServiceTailGrowthBytes) {
		throw "dogmosd private bytes grew by $serviceTailGrowth after midpoint, above $ServiceTailGrowthBytes"
	}
	if ($shimRange -gt 33554432) {
		throw "i686 shim private bytes varied by $shimRange, above the 32 MiB ceiling"
	}
	Write-Output "callback pressure passed: cycles=$Cycles service_range=$serviceRange service_tail_growth=$serviceTailGrowth shim_range=$shimRange evidence=$outputPath"
} finally {
	[Environment]::SetEnvironmentVariable('DOGMOS_SOURCE_REVISION', $previousSourceRevision, 'Process')
	[Environment]::SetEnvironmentVariable('DOGMOS_FEATURE_FINGERPRINT', $previousFeatureFingerprint, 'Process')
	Pop-Location
}
