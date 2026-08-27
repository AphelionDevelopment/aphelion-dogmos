[CmdletBinding()]
param(
	[string] $CargoPath = "cargo",
	[string] $Target = "i686-pc-windows-msvc"
)

$ErrorActionPreference = "Stop"

$matrix = @(
	@{ Name = "no features"; NoDefaultFeatures = $true; Features = $null },
	@{ Name = "turf_processing"; NoDefaultFeatures = $true; Features = "turf_processing" },
	@{ Name = "fastmos"; NoDefaultFeatures = $true; Features = "fastmos" },
	@{ Name = "katmos"; NoDefaultFeatures = $true; Features = "katmos" },
	@{ Name = "superconductivity"; NoDefaultFeatures = $true; Features = "superconductivity" },
	@{ Name = "reaction_hooks"; NoDefaultFeatures = $true; Features = "reaction_hooks" },
	@{ Name = "aphelion_reactions"; NoDefaultFeatures = $true; Features = "aphelion_reactions" },
	@{ Name = "citadel_reactions"; NoDefaultFeatures = $true; Features = "citadel_reactions" },
	@{ Name = "yogs_reactions"; NoDefaultFeatures = $true; Features = "yogs_reactions" },
	@{ Name = "zas_hooks"; NoDefaultFeatures = $true; Features = "zas_hooks" },
	@{ Name = "default"; NoDefaultFeatures = $false; Features = $null },
	@{ Name = "default + tracy"; NoDefaultFeatures = $false; Features = "tracy" }
)

foreach ($configuration in $matrix) {
	$arguments = @(
		"check",
		"--workspace",
		"--locked",
		"--target",
		$Target,
		"--all-targets"
	)
	if ($configuration.NoDefaultFeatures) {
		$arguments += "--no-default-features"
	}
	if ($configuration.Features) {
		$arguments += "--features"
		$arguments += $configuration.Features
	}

	Write-Host "Checking Dogmos feature configuration: $($configuration.Name)"
	& $CargoPath @arguments
	$configurationExitCode = $LASTEXITCODE
	if ($configurationExitCode -ne 0) {
		[Console]::Error.WriteLine(
			"Dogmos feature configuration '$($configuration.Name)' failed with exit code $configurationExitCode."
		)
		exit $configurationExitCode
	}
}

Write-Host "All Dogmos feature configurations passed."
