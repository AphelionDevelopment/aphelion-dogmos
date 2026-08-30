[CmdletBinding()]
param(
	[int]$Iterations = 20000,
	[int]$Repetitions = 3
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
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null

Push-Location $repositoryRoot
try {
	& cargo "+$rustToolchain" build -p dogmos-server --bin dogmosd --target $serverTarget --release --locked --offline
	if ($LASTEXITCODE -ne 0) { throw "x64 dogmosd build failed with exit code $LASTEXITCODE" }
	$env:DOGMOSD_PATH = Join-Path $repositoryRoot "target\$serverTarget\release\dogmosd.exe"
	$env:DOGMOS_IPC_ITERATIONS = $Iterations
	$successfulStatusRecords = @()
	$successfulCsvPaths = @()
	for ($run = 1; $run -le $Repetitions; $run++) {
		$outputPath = Join-Path $outputDirectory "ipc-round-trip-$run.csv"
		$memoryPath = Join-Path $outputDirectory "ipc-process-memory-$run.csv"
		$statusPath = Join-Path $outputDirectory "ipc-round-trip-$run.status.json"
		Remove-Item -LiteralPath $outputPath, $memoryPath, $statusPath -Force -ErrorAction SilentlyContinue
		'role,process_id,private_bytes,virtual_bytes,working_set_bytes' | Set-Content -LiteralPath $memoryPath
		& cargo "+$rustToolchain" bench -p dogmos-perf --bench ipc_round_trip --features ipc-benchmark --target $shimTarget --locked --offline 2>&1 |
			Tee-Object -FilePath $outputPath |
			ForEach-Object {
				Write-Output $_
				if ($_ -match '^processes,shim_pid=(\d+),service_pid=(\d+),') {
					@(
						@{ Role = 'shim'; Id = [int]$Matches[1] },
						@{ Role = 'service'; Id = [int]$Matches[2] }
					) | ForEach-Object {
						$process = Get-Process -Id $_.Id
						"$($_.Role),$($process.Id),$($process.PrivateMemorySize64),$($process.VirtualMemorySize64),$($process.WorkingSet64)" |
							Add-Content -LiteralPath $memoryPath
					}
				}
			}
		$benchmarkExitCode = $LASTEXITCODE
		if ($benchmarkExitCode -ne 0) { throw "IPC benchmark run $run failed with exit code $benchmarkExitCode" }
		$status = [ordered]@{
			run = $run
			status = 'succeeded'
			iterations = $Iterations
			source_revision = $buildIdentity.source_revision
			feature_fingerprint = $buildIdentity.feature_fingerprint
			rust_toolchain = $rustToolchain
			shim_target = $shimTarget
			server_target = $serverTarget
			output_path = $outputPath
			memory_path = $memoryPath
			completed_utc = [DateTime]::UtcNow.ToString('o')
		}
		$status | ConvertTo-Json | Set-Content -LiteralPath $statusPath -Encoding utf8
		$successfulStatusRecords += $statusPath
		$successfulCsvPaths += $outputPath
	}
	if ($successfulStatusRecords.Count -ne $Repetitions -or $successfulCsvPaths.Count -ne $Repetitions) {
		throw "IPC benchmark produced $($successfulStatusRecords.Count) status records and $($successfulCsvPaths.Count) CSVs; expected $Repetitions of each"
	}
	foreach ($path in @($successfulStatusRecords) + @($successfulCsvPaths)) {
		if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
			throw "IPC benchmark evidence is incomplete: missing $path"
		}
	}
} finally {
	Remove-Item Env:DOGMOSD_PATH -ErrorAction SilentlyContinue
	Remove-Item Env:DOGMOS_IPC_ITERATIONS -ErrorAction SilentlyContinue
	[Environment]::SetEnvironmentVariable('DOGMOS_SOURCE_REVISION', $previousSourceRevision, 'Process')
	[Environment]::SetEnvironmentVariable('DOGMOS_FEATURE_FINGERPRINT', $previousFeatureFingerprint, 'Process')
	Pop-Location
}
