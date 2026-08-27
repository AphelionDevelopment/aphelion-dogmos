[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$rustToolchain = '1.98.0'
$serverTarget = 'x86_64-pc-windows-msvc'
$shimTarget = 'i686-pc-windows-msvc'

function Assert-NativeExitCode {
	param([string]$Label)
	if ($LASTEXITCODE -ne 0) {
		throw "$Label failed with exit code $LASTEXITCODE"
	}
}

function Get-PeMachine {
	param([string]$Path)
	$bytes = [System.IO.File]::ReadAllBytes($Path)
	$peOffset = [System.BitConverter]::ToInt32($bytes, 0x3c)
	return [System.BitConverter]::ToUInt16($bytes, $peOffset + 4)
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$identityModule = Join-Path $PSScriptRoot 'DogmosBuildIdentity.psm1'
Import-Module $identityModule -Force
$buildIdentity = Get-DogmosBuildIdentity -RepositoryRoot $repositoryRoot -AllowDirty
$previousSourceRevision = [Environment]::GetEnvironmentVariable('DOGMOS_SOURCE_REVISION', 'Process')
$previousFeatureFingerprint = [Environment]::GetEnvironmentVariable('DOGMOS_FEATURE_FINGERPRINT', 'Process')
$env:DOGMOS_SOURCE_REVISION = $buildIdentity.source_revision
$env:DOGMOS_FEATURE_FINGERPRINT = $buildIdentity.feature_fingerprint
Push-Location $repositoryRoot
try {
	& cargo "+$rustToolchain" build -p dogmos-server --bin dogmosd --target $serverTarget --offline
	Assert-NativeExitCode 'x64 dogmosd build'
	& cargo "+$rustToolchain" build -p dogmos-byond --example cross_bitness_probe --target $shimTarget --offline
	Assert-NativeExitCode 'i686 IPC probe build'

	$serverPath = Join-Path $repositoryRoot "target\$serverTarget\debug\dogmosd.exe"
	$probePath = Join-Path $repositoryRoot "target\$shimTarget\debug\examples\cross_bitness_probe.exe"
	if ((Get-PeMachine $serverPath) -ne 0x8664) {
		throw 'dogmosd is not an x64 PE executable'
	}
	if ((Get-PeMachine $probePath) -ne 0x014c) {
		throw 'cross-bitness probe is not an x86 PE executable'
	}

	& $probePath $serverPath
	Assert-NativeExitCode 'i686-to-x64 IPC probe'
} finally {
	[Environment]::SetEnvironmentVariable('DOGMOS_SOURCE_REVISION', $previousSourceRevision, 'Process')
	[Environment]::SetEnvironmentVariable('DOGMOS_FEATURE_FINGERPRINT', $previousFeatureFingerprint, 'Process')
	Pop-Location
}
