function Get-DogmosBuildIdentity {
	[CmdletBinding()]
	param(
		[Parameter(Mandatory = $true)]
		[string] $RepositoryRoot,
		[switch] $AllowDirty
	)

	$resolvedRoot = (Resolve-Path -LiteralPath $RepositoryRoot).Path
	$manifestPath = Join-Path $resolvedRoot 'dogmos-build-manifest.toml'
	if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
		throw "Dogmos build manifest is missing: $manifestPath"
	}

	$sourceRevision = (& git -C $resolvedRoot rev-parse --verify HEAD 2>&1 | Out-String).Trim()
	if ($LASTEXITCODE -ne 0 -or $sourceRevision -notmatch '^[0-9a-fA-F]{40}$') {
		throw 'Unable to resolve an exact 40-character Git source revision'
	}
	$sourceRevision = $sourceRevision.ToLowerInvariant()

	$workingTreeStatus = @(& git -C $resolvedRoot status --porcelain=v1 --untracked-files=all 2>&1)
	if ($LASTEXITCODE -ne 0) {
		throw 'Unable to inspect the Git working tree state'
	}
	$dirty = $workingTreeStatus.Count -gt 0
	if ($dirty -and -not $AllowDirty) {
		throw 'Refusing to create a production build identity from a dirty working tree'
	}

	$manifestStream = [System.IO.File]::OpenRead($manifestPath)
	$sha256 = [System.Security.Cryptography.SHA256]::Create()
	try {
		$featureFingerprint = ([System.BitConverter]::ToString(
			$sha256.ComputeHash($manifestStream)
		)).Replace('-', '').ToLowerInvariant()
	} finally {
		$sha256.Dispose()
		$manifestStream.Dispose()
	}
	return [pscustomobject]@{
		source_revision = $sourceRevision
		feature_fingerprint = $featureFingerprint
		dirty = $dirty
		manifest_path = $manifestPath
	}
}

Export-ModuleMember -Function Get-DogmosBuildIdentity
