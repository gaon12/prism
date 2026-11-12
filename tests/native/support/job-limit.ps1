# Confine one owned process (and every child it starts afterwards) to a
# low-end machine profile with a Windows Job Object, then report its peak usage.
#
#   pwsh -NoProfile -File job-limit.ps1 -ProcessId <pid> -Cpus 1 -MemoryMB 512
#
# The limits are real kernel limits, not sampling:
# - CPU affinity pins every process in the job to the first N logical CPUs.
# - Per-process committed memory above MemoryMB fails the allocation, exactly as
#   running out of memory on a small machine would.
# The script prints one JSON line "ready" after assignment. Afterwards, each
# "report" line on stdin prints peak usage, and "exit" (or EOF) ends the script.
# Closing the job handle does not kill the process; its owner remains responsible.
param(
  [Parameter(Mandatory = $true)][int]$ProcessId,
  [ValidateRange(1, 64)][int]$Cpus = 1,
  [ValidateRange(64, 65536)][int]$MemoryMB = 512
)
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class PrismJob {
  [StructLayout(LayoutKind.Sequential)]
  struct BasicLimit {
    public long PerProcessUserTimeLimit, PerJobUserTimeLimit;
    public uint LimitFlags;
    public UIntPtr MinimumWorkingSetSize, MaximumWorkingSetSize;
    public uint ActiveProcessLimit;
    public UIntPtr Affinity;
    public uint PriorityClass, SchedulingClass;
  }
  [StructLayout(LayoutKind.Sequential)]
  struct IoCounters {
    public ulong ReadOperationCount, WriteOperationCount, OtherOperationCount;
    public ulong ReadTransferCount, WriteTransferCount, OtherTransferCount;
  }
  [StructLayout(LayoutKind.Sequential)]
  struct ExtendedLimit {
    public BasicLimit Basic;
    public IoCounters Io;
    public UIntPtr ProcessMemoryLimit, JobMemoryLimit, PeakProcessMemoryUsed, PeakJobMemoryUsed;
  }
  [StructLayout(LayoutKind.Sequential)]
  struct Accounting {
    public long TotalUserTime, TotalKernelTime, ThisPeriodTotalUserTime, ThisPeriodTotalKernelTime;
    public uint TotalPageFaultCount, TotalProcesses, ActiveProcesses, TotalTerminatedProcesses;
  }

  const int ExtendedLimitInformation = 9, BasicAccountingInformation = 1;
  const uint LimitAffinity = 0x10, LimitProcessMemory = 0x100;
  const uint ProcessSetQuota = 0x0100, ProcessTerminate = 0x0001;

  [DllImport("kernel32.dll", SetLastError = true)]
  static extern IntPtr CreateJobObject(IntPtr attributes, string name);
  [DllImport("kernel32.dll", SetLastError = true)]
  static extern bool SetInformationJobObject(IntPtr job, int infoClass, ref ExtendedLimit info, int length);
  [DllImport("kernel32.dll", SetLastError = true)]
  static extern bool QueryInformationJobObject(IntPtr job, int infoClass, out ExtendedLimit info, int length, IntPtr returned);
  [DllImport("kernel32.dll", SetLastError = true)]
  static extern bool QueryInformationJobObject(IntPtr job, int infoClass, out Accounting info, int length, IntPtr returned);
  [DllImport("kernel32.dll", SetLastError = true)]
  static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
  [DllImport("kernel32.dll", SetLastError = true)]
  static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
  [DllImport("kernel32.dll", SetLastError = true)]
  static extern bool CloseHandle(IntPtr handle);

  static IntPtr job;

  public static void Confine(int pid, int cpus, long memoryBytes) {
    job = CreateJobObject(IntPtr.Zero, null);
    if (job == IntPtr.Zero) throw new System.ComponentModel.Win32Exception();
    var limit = new ExtendedLimit();
    limit.Basic.LimitFlags = LimitAffinity | LimitProcessMemory;
    limit.Basic.Affinity = new UIntPtr((1UL << cpus) - 1);
    limit.ProcessMemoryLimit = new UIntPtr((ulong)memoryBytes);
    if (!SetInformationJobObject(job, ExtendedLimitInformation, ref limit, Marshal.SizeOf(limit)))
      throw new System.ComponentModel.Win32Exception();
    var process = OpenProcess(ProcessSetQuota | ProcessTerminate, false, pid);
    if (process == IntPtr.Zero) throw new System.ComponentModel.Win32Exception();
    try {
      if (!AssignProcessToJobObject(job, process)) throw new System.ComponentModel.Win32Exception();
    } finally {
      CloseHandle(process);
    }
  }

  public static string Report() {
    ExtendedLimit limit;
    Accounting accounting;
    if (!QueryInformationJobObject(job, ExtendedLimitInformation, out limit, Marshal.SizeOf(typeof(ExtendedLimit)), IntPtr.Zero)
        || !QueryInformationJobObject(job, BasicAccountingInformation, out accounting, Marshal.SizeOf(typeof(Accounting)), IntPtr.Zero))
      throw new System.ComponentModel.Win32Exception();
    // Times are 100 ns units; report milliseconds.
    return string.Format(
      "{{\"peakProcessMemoryBytes\":{0},\"peakJobMemoryBytes\":{1},\"cpuUserMs\":{2},\"cpuKernelMs\":{3},\"totalProcesses\":{4},\"activeProcesses\":{5},\"pageFaults\":{6}}}",
      limit.PeakProcessMemoryUsed, limit.PeakJobMemoryUsed,
      accounting.TotalUserTime / 10000, accounting.TotalKernelTime / 10000,
      accounting.TotalProcesses, accounting.ActiveProcesses, accounting.TotalPageFaultCount);
  }
}
'@

$cpuCount = [Environment]::ProcessorCount
if ($Cpus -gt $cpuCount) { throw "Requested $Cpus CPUs but this machine has $cpuCount." }
[PrismJob]::Confine($ProcessId, $Cpus, [long]$MemoryMB * 1MB)
[Console]::Out.WriteLine((@{ ready = $true; cpus = $Cpus; memoryMB = $MemoryMB } | ConvertTo-Json -Compress))
[Console]::Out.Flush()
while ($null -ne ($line = [Console]::In.ReadLine())) {
  if ($line -eq 'report') {
    [Console]::Out.WriteLine([PrismJob]::Report())
    [Console]::Out.Flush()
  } elseif ($line -eq 'exit') {
    break
  }
}
