[CmdletBinding()]
param(
	[Parameter(Mandatory)][int]$DreamDaemonPid,
	[int]$ServerPid = 0,
	[Parameter(Mandatory)][string]$OutputDirectory,
	[double]$DurationSeconds = 60,
	[int]$SampleIntervalMilliseconds = 250
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if($DreamDaemonPid -le 0) { throw '-DreamDaemonPid must be a positive exact PID.' }
if($ServerPid -lt 0) { throw '-ServerPid must be zero or a positive exact PID.' }
if($ServerPid -eq $DreamDaemonPid) { throw 'DreamDaemon and server must be different processes.' }
if($DurationSeconds -le 0) { throw '-DurationSeconds must be positive.' }
if($SampleIntervalMilliseconds -lt 25) { throw '-SampleIntervalMilliseconds must be at least 25.' }

$output = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Path $output -Force | Out-Null

if(-not ('Dogmos.ProcessMemory' -as [type])) {
	Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

namespace Dogmos {
	public static class ProcessMemory {
		[StructLayout(LayoutKind.Sequential)]
		public struct MemoryBasicInformation {
			public IntPtr BaseAddress;
			public IntPtr AllocationBase;
			public UInt32 AllocationProtect;
			public UIntPtr RegionSize;
			public UInt32 State;
			public UInt32 Protect;
			public UInt32 Type;
		}

		[DllImport("kernel32.dll", SetLastError = true)]
		public static extern IntPtr OpenProcess(UInt32 access, bool inheritHandle, Int32 processId);

		[DllImport("kernel32.dll", SetLastError = true)]
		public static extern bool CloseHandle(IntPtr handle);

		[DllImport("kernel32.dll", SetLastError = true)]
		public static extern UIntPtr VirtualQueryEx(
			IntPtr process,
			IntPtr address,
			out MemoryBasicInformation information,
			UIntPtr informationLength
		);
	}
}
'@
}

function Get-ProcessMemoryCheckpoint {
	param([Parameter(Mandatory)][int]$ExactPid)

	$processQueryInformation = [uint32]0x0400
	$handle = [Dogmos.ProcessMemory]::OpenProcess($processQueryInformation, $false, $ExactPid)
	if($handle -eq [IntPtr]::Zero) {
		throw "OpenProcess failed for exact PID $ExactPid with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())."
	}
	try {
		$information = New-Object Dogmos.ProcessMemory+MemoryBasicInformation
		$informationSize = [Runtime.InteropServices.Marshal]::SizeOf($information)
		[uint64]$address = 0
		[uint64]$committedPrivate = 0
		[uint64]$committedMapped = 0
		[uint64]$committedImage = 0
		[uint64]$reserved = 0
		[int]$regions = 0
		while($true) {
			$result = [Dogmos.ProcessMemory]::VirtualQueryEx(
				$handle,
				[System.IntPtr]::new([int64]$address),
				[ref]$information,
				[UIntPtr]::new([uint32]$informationSize)
			)
			if($result -eq [UIntPtr]::Zero) { break }
			$regionBytes = $information.RegionSize.ToUInt64()
			if($regionBytes -eq 0) { break }
			$regions++
			if($information.State -eq 0x1000) {
				switch($information.Type) {
					0x20000 { $committedPrivate += $regionBytes }
					0x40000 { $committedMapped += $regionBytes }
					0x1000000 { $committedImage += $regionBytes }
				}
			} elseif($information.State -eq 0x2000) {
				$reserved += $regionBytes
			}
			$base = [uint64]$information.BaseAddress.ToInt64()
			$next = $base + $regionBytes
			if($next -le $address -or $next -gt [uint64][int64]::MaxValue) { break }
			$address = $next
		}
		[pscustomobject]@{
			committed_private_bytes = $committedPrivate
			committed_mapped_bytes = $committedMapped
			committed_image_bytes = $committedImage
			reserved_bytes = $reserved
			region_count = $regions
		}
	} finally {
		[void][Dogmos.ProcessMemory]::CloseHandle($handle)
	}
}

function Get-ExactProcess {
	param([Parameter(Mandatory)][int]$ExactPid, [Parameter(Mandatory)][string]$Role)
	try {
		return Get-Process -Id $ExactPid -ErrorAction Stop
	} catch {
		throw "The exact $Role PID $ExactPid is not running."
	}
}

$processes = [ordered]@{
	dreamdaemon = Get-ExactProcess -ExactPid $DreamDaemonPid -Role 'DreamDaemon'
}
if($ServerPid -gt 0) {
	$processes.server = Get-ExactProcess -ExactPid $ServerPid -Role 'server'
}

$checkpoints = [ordered]@{}
foreach($entry in $processes.GetEnumerator()) {
	$checkpoints[$entry.Key] = Get-ProcessMemoryCheckpoint -ExactPid $entry.Value.Id
}

$samples = [Collections.Generic.List[object]]::new()
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
do {
	foreach($entry in $processes.GetEnumerator()) {
		$process = $entry.Value
		$process.Refresh()
		if($process.HasExited) { throw "Exact $($entry.Key) PID $($process.Id) exited during sampling." }
		$samples.Add([pscustomobject]@{
			timestamp_utc = [DateTime]::UtcNow.ToString('o')
			elapsed_milliseconds = $stopwatch.ElapsedMilliseconds
			role = $entry.Key
			pid = $process.Id
			process_name = $process.ProcessName
			private_bytes = $process.PrivateMemorySize64
			virtual_bytes = $process.VirtualMemorySize64
			working_set_bytes = $process.WorkingSet64
			peak_working_set_bytes = $process.PeakWorkingSet64
			handle_count = $process.HandleCount
			thread_count = $process.Threads.Count
			cpu_total_ticks = $process.TotalProcessorTime.Ticks
		})
	}
	if($stopwatch.Elapsed.TotalSeconds -lt $DurationSeconds) {
		Start-Sleep -Milliseconds $SampleIntervalMilliseconds
	}
} while($stopwatch.Elapsed.TotalSeconds -lt $DurationSeconds)
$stopwatch.Stop()

function Get-RoleSummary {
	param([Parameter(Mandatory)][string]$Role, [Parameter(Mandatory)][Diagnostics.Process]$Process)
	$roleSamples = @($samples | Where-Object role -eq $Role)
	$latest = $roleSamples[-1]
	[ordered]@{
		pid = $Process.Id
		process_name = $Process.ProcessName
		sample_count = $roleSamples.Count
		private_bytes_latest = $latest.private_bytes
		private_bytes_peak = ($roleSamples.private_bytes | Measure-Object -Maximum).Maximum
		private_bytes_mean = [math]::Round(($roleSamples.private_bytes | Measure-Object -Average).Average)
		virtual_bytes_latest = $latest.virtual_bytes
		virtual_bytes_peak = ($roleSamples.virtual_bytes | Measure-Object -Maximum).Maximum
		working_set_bytes_latest = $latest.working_set_bytes
		working_set_bytes_peak = ($roleSamples.working_set_bytes | Measure-Object -Maximum).Maximum
		peak_working_set_bytes = ($roleSamples.peak_working_set_bytes | Measure-Object -Maximum).Maximum
		handle_count_peak = ($roleSamples.handle_count | Measure-Object -Maximum).Maximum
		thread_count_peak = ($roleSamples.thread_count | Measure-Object -Maximum).Maximum
		cpu_total_ticks_latest = $latest.cpu_total_ticks
		virtual_query_checkpoint = $checkpoints[$Role]
	}
}

$roles = [ordered]@{}
foreach($entry in $processes.GetEnumerator()) {
	$roles[$entry.Key] = Get-RoleSummary -Role $entry.Key -Process $entry.Value
}
$summary = [ordered]@{
	schema_version = 1
	sample_interval_milliseconds = $SampleIntervalMilliseconds
	duration_seconds_requested = $DurationSeconds
	duration_seconds_actual = $stopwatch.Elapsed.TotalSeconds
	server_memory_is_separate = $true
	server_memory_is_in_dreamdaemon_total = $false
	roles = $roles
}

$samplesPath = Join-Path $output 'process-samples.csv'
$summaryPath = Join-Path $output 'process-summary.json'
$samples | Export-Csv -LiteralPath $samplesPath -NoTypeInformation -Encoding UTF8
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding UTF8

[pscustomobject]@{
	samples_path = $samplesPath
	summary_path = $summaryPath
	server_memory_is_separate = $true
	roles = $roles
} | ConvertTo-Json -Depth 8 -Compress
