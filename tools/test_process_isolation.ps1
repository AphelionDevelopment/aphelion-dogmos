[CmdletBinding()]
param(
	[long]$DiagnosticBytes = 536870912,
	[int]$HoldMilliseconds = 1000
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
$outputPath = Join-Path $outputDirectory 'process-isolation.csv'
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

Push-Location $repositoryRoot
try {
	& cargo "+$rustToolchain" build -p dogmos-server --bin dogmosd --target $serverTarget --release --offline
	if ($LASTEXITCODE -ne 0) { throw "x64 dogmosd build failed with exit code $LASTEXITCODE" }
	& cargo "+$rustToolchain" build -p dogmos-byond --example cross_bitness_probe --target $shimTarget --release --offline
	if ($LASTEXITCODE -ne 0) { throw "i686 IPC probe build failed with exit code $LASTEXITCODE" }

	$serverPath = Join-Path $repositoryRoot "target\$serverTarget\release\dogmosd.exe"
	$probePath = Join-Path $repositoryRoot "target\$shimTarget\release\examples\cross_bitness_probe.exe"
	$baseline = @{}
	$allocated = @{}
	& $probePath $serverPath $DiagnosticBytes $HoldMilliseconds 2>&1 | ForEach-Object {
		Write-Output $_
		if ($_ -match '^isolation_(baseline|allocated),shim_pid=(\d+),service_pid=(\d+)') {
			$phase = $Matches[1]
			$shimId = [int]$Matches[2]
			$serviceId = [int]$Matches[3]
			$target = if ($phase -eq 'baseline') { $baseline } else { $allocated }
			$target.Shim = Read-ProcessSample $phase 'shim' $shimId
			$target.Service = Read-ProcessSample $phase 'service' $serviceId
		}
	}
	if ($LASTEXITCODE -ne 0) { throw "process isolation probe failed with exit code $LASTEXITCODE" }
	$shimGrowth = $allocated.Shim - $baseline.Shim
	$serviceGrowth = $allocated.Service - $baseline.Service
	if ($shimGrowth -gt 33554432) {
		throw "i686 shim private bytes grew by $shimGrowth, above the 32 MiB limit"
	}
	if ($serviceGrowth -lt ($DiagnosticBytes * 0.9)) {
		throw "x64 service private bytes grew by only $serviceGrowth for a $DiagnosticBytes-byte arena"
	}
	Write-Output "process isolation passed: shim_growth=$shimGrowth service_growth=$serviceGrowth"
} finally {
	[Environment]::SetEnvironmentVariable('DOGMOS_SOURCE_REVISION', $previousSourceRevision, 'Process')
	[Environment]::SetEnvironmentVariable('DOGMOS_FEATURE_FINGERPRINT', $previousFeatureFingerprint, 'Process')
	Pop-Location
}
