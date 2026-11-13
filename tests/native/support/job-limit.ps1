# Confine one owned process (and every child it starts afterwards) to a
# low-end machine profile with a Windows Job Object, then report its usage.
#
#   pwsh -NoProfile -File job-limit.ps1 -Cpus 1 -MemoryMB 512
#
# Protocol (one JSON line per answer):
# 1. On start the job is created and its limits applied before any process is
#    known, and "prepared" is printed. Compiling the helper takes about a
#    second, so do this before launching the app.
# 2. "assign <pid>" on stdin assigns that process. Children it starts later
#    join the job automatically. The answer echoes the limits read back from
#    the kernel and the process ids in the job.
# 3. "report" prints peak memory, CPU time and the processes in the job.
# 4. "exit" (or EOF) ends the script. Closing the job handle does not kill
#    the processes; their owner remains responsible.
#
# The limits are real kernel limits, not sampling:
# - CPU affinity pins every process in the job to the first N CPUs this
#   helper itself may run on (its own allowed mask, not assumed CPU numbers).
# - Per-process AND whole-job committed memory above MemoryMB fail the
#   allocation, as running out of memory on a small machine would. The job
#   limit covers the app together with its WebView processes.
# -ProcessId still assigns immediately for compatibility (prints "ready").
param(
  [int]$ProcessId = 0,
  [ValidateRange(1, 64)][int]$Cpus = 1,
  [ValidateRange(64, 65536)][int]$MemoryMB = 512
)
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
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

  const int BasicAccountingInformation = 1, BasicProcessIdList = 3, ExtendedLimitInformation = 9;
  const uint LimitAffinity = 0x10, LimitProcessMemory = 0x100, LimitJobMemory = 0x200;
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
  static extern bool QueryInformationJobObject(IntPtr job, int infoClass, IntPtr info, int length, IntPtr returned);
  [DllImport("kernel32.dll", SetLastError = true)]
  static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
  [DllImport("kernel32.dll", SetLastError = true)]
  static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
  [DllImport("kernel32.dll", SetLastError = true)]
  static extern bool CloseHandle(IntPtr handle);
  [DllImport("kernel32.dll")]
  static extern IntPtr GetCurrentProcess();
  [DllImport("kernel32.dll", SetLastError = true)]
  static extern bool GetProcessAffinityMask(IntPtr process, out UIntPtr processMask, out UIntPtr systemMask);

  static IntPtr job;

  /// Create the job with its limits. Affinity uses the lowest `cpus` CPUs of
  /// this helper's own allowed mask, so a restricted host still gets valid CPUs.
  public static void Prepare(int cpus, long memoryBytes) {
    UIntPtr allowed, system;
    if (!GetProcessAffinityMask(GetCurrentProcess(), out allowed, out system))
      throw new System.ComponentModel.Win32Exception();
    ulong mask = 0, remaining = allowed.ToUInt64();
    for (int chosen = 0; chosen < cpus; chosen++) {
      if (remaining == 0) throw new InvalidOperationException("Not enough allowed CPUs for this profile.");
      ulong lowest = remaining & (~remaining + 1);
      mask |= lowest;
      remaining &= ~lowest;
    }
    job = CreateJobObject(IntPtr.Zero, null);
    if (job == IntPtr.Zero) throw new System.ComponentModel.Win32Exception();
    var limit = new ExtendedLimit();
    limit.Basic.LimitFlags = LimitAffinity | LimitProcessMemory | LimitJobMemory;
    limit.Basic.Affinity = new UIntPtr(mask);
    limit.ProcessMemoryLimit = new UIntPtr((ulong)memoryBytes);
    limit.JobMemoryLimit = new UIntPtr((ulong)memoryBytes);
    if (!SetInformationJobObject(job, ExtendedLimitInformation, ref limit, Marshal.SizeOf(limit)))
      throw new System.ComponentModel.Win32Exception();
  }

  public static void Assign(int pid) {
    var process = OpenProcess(ProcessSetQuota | ProcessTerminate, false, pid);
    if (process == IntPtr.Zero) throw new System.ComponentModel.Win32Exception();
    try {
      if (!AssignProcessToJobObject(job, process)) throw new System.ComponentModel.Win32Exception();
    } finally {
      CloseHandle(process);
    }
  }

  /// Process ids currently in the job.
  static string ProcessIds() {
    // Header: two uint counts, then one ULONG_PTR per process.
    int capacity = 256, size = 8 + capacity * IntPtr.Size;
    IntPtr buffer = Marshal.AllocHGlobal(size);
    try {
      if (!QueryInformationJobObject(job, BasicProcessIdList, buffer, size, IntPtr.Zero))
        throw new System.ComponentModel.Win32Exception();
      int listed = Marshal.ReadInt32(buffer, 4);
      var ids = new List<string>();
      for (int index = 0; index < listed; index++)
        ids.Add(Marshal.ReadIntPtr(buffer, 8 + index * IntPtr.Size).ToInt64().ToString());
      return "[" + string.Join(",", ids) + "]";
    } finally {
      Marshal.FreeHGlobal(buffer);
    }
  }

  /// Per-process name, private (committed) bytes, working set and CPU time
  /// so far (user/kernel ms), for the processes still running in the job, so
  /// the app's own CPU can be told apart from its WebView processes'.
  /// Exited processes are skipped.
  static string ProcessUsage() {
    var rows = new List<string>();
    foreach (var text in ProcessIds().Trim('[', ']').Split(new[] { ',' }, StringSplitOptions.RemoveEmptyEntries)) {
      try {
        using (var process = System.Diagnostics.Process.GetProcessById(int.Parse(text))) {
          rows.Add(string.Format("{{\"pid\":{0},\"name\":\"{1}\",\"privateBytes\":{2},\"workingSetBytes\":{3},\"cpuUserMs\":{4},\"cpuKernelMs\":{5}}}",
            text, process.ProcessName.Replace("\\", "").Replace("\"", ""), process.PrivateMemorySize64, process.WorkingSet64,
            (long)process.UserProcessorTime.TotalMilliseconds, (long)process.PrivilegedProcessorTime.TotalMilliseconds));
        }
      } catch (ArgumentException) {
      } catch (InvalidOperationException) {
      }
    }
    return "[" + string.Join(",", rows) + "]";
  }

  /// Limits as the kernel reports them back, plus the job's processes.
  public static string Limits() {
    ExtendedLimit limit;
    if (!QueryInformationJobObject(job, ExtendedLimitInformation, out limit, Marshal.SizeOf(typeof(ExtendedLimit)), IntPtr.Zero))
      throw new System.ComponentModel.Win32Exception();
    return string.Format(
      "\"limitFlags\":{0},\"affinityMask\":{1},\"processMemoryLimitBytes\":{2},\"jobMemoryLimitBytes\":{3},\"processIds\":{4}",
      limit.Basic.LimitFlags, limit.Basic.Affinity.ToUInt64(), limit.ProcessMemoryLimit.ToUInt64(),
      limit.JobMemoryLimit.ToUInt64(), ProcessIds());
  }

  public static string Report() {
    ExtendedLimit limit;
    Accounting accounting;
    if (!QueryInformationJobObject(job, ExtendedLimitInformation, out limit, Marshal.SizeOf(typeof(ExtendedLimit)), IntPtr.Zero)
        || !QueryInformationJobObject(job, BasicAccountingInformation, out accounting, Marshal.SizeOf(typeof(Accounting)), IntPtr.Zero))
      throw new System.ComponentModel.Win32Exception();
    // Times are 100 ns units; report milliseconds.
    return string.Format(
      "{{\"peakProcessMemoryBytes\":{0},\"peakJobMemoryBytes\":{1},\"cpuUserMs\":{2},\"cpuKernelMs\":{3},\"totalProcesses\":{4},\"activeProcesses\":{5},\"pageFaults\":{6},{7},\"processes\":{8}}}",
      limit.PeakProcessMemoryUsed, limit.PeakJobMemoryUsed,
      accounting.TotalUserTime / 10000, accounting.TotalKernelTime / 10000,
      accounting.TotalProcesses, accounting.ActiveProcesses, accounting.TotalPageFaultCount, Limits(),
      ProcessUsage());
  }
}
'@

function Write-Line([string]$text) {
  [Console]::Out.WriteLine($text)
  [Console]::Out.Flush()
}

[PrismJob]::Prepare($Cpus, [long]$MemoryMB * 1MB)
if ($ProcessId -gt 0) {
  [PrismJob]::Assign($ProcessId)
  Write-Line ('{"ready":true,"cpus":' + $Cpus + ',"memoryMB":' + $MemoryMB + ',' + [PrismJob]::Limits() + '}')
} else {
  Write-Line ('{"prepared":true,"cpus":' + $Cpus + ',"memoryMB":' + $MemoryMB + '}')
}
while ($null -ne ($line = [Console]::In.ReadLine())) {
  if ($line -match '^assign (\d+)$') {
    [PrismJob]::Assign([int]$Matches[1])
    Write-Line ('{"ready":true,"cpus":' + $Cpus + ',"memoryMB":' + $MemoryMB + ',' + [PrismJob]::Limits() + '}')
  } elseif ($line -eq 'report') {
    Write-Line ([PrismJob]::Report())
  } elseif ($line -eq 'exit') {
    break
  }
}
