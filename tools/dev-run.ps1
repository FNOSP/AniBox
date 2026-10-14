# dev-run.ps1 -- run the dev command under two guarantees:
#   (A) closing the console window kills the whole tree (Job Object, kill-on-close)
#   (B) when the app window is closed by the user, this script returns promptly so
#       dev.cmd can finish, print the log tail, and close itself
#
# [lc-1547] Both matter:
#   (A) Windows does not kill descendants when a console window is closed with the X
#       button, so vite + cargo + anibox.exe would otherwise survive.
#   (B) `npm run tauri dev` sometimes lingers after the app exits; without a watcher the
#       cmd window just sits there until you close it by hand (the reported symptom).
#
# Exit codes:
#   0  = the app ran and was closed by the user  -> dev.cmd closes without pausing
#   1+ = the app never started / a build error   -> dev.cmd keeps the window open
#
# Usage: pwsh -NoProfile -ExecutionPolicy Bypass -File tools\dev-run.ps1 -Command "npm run tauri dev"
#
# Notes:
#  - PS 5.1-compatible syntax; this repo's pwsh is 7.x but the script must not depend on it.
#  - Pure ASCII on purpose: non-ASCII in .ps1/.cmd gets mangled by the OEM codepage.

param(
    [Parameter(Mandatory = $true)][string]$Command,
    [string]$AppProcessName = 'anibox.exe',
    [int]$AppStartTimeoutSec = 1800,   # cold Rust build: 5-15 min
    [int]$PollMs = 1000
)

$ErrorActionPreference = 'Stop'

# ---- Job Object: kill the whole tree when this process dies -------------------------
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class JobKill {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateJobObject(IntPtr a, string lpName);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetInformationJobObject(IntPtr hJob, int JobObjectInfoClass, IntPtr lpJobObjectInfo, uint cbJobObjectInfoLength);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr GetCurrentProcess();

    [StructLayout(LayoutKind.Sequential)]
    private struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
        public long PerProcessUserTimeLimit;
        public long PerJobUserTimeLimit;
        public uint LimitFlags;
        public UIntPtr MinimumWorkingSetSize;
        public UIntPtr MaximumWorkingSetSize;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass;
        public uint SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct IO_COUNTERS {
        public ulong ReadOperationCount;
        public ulong WriteOperationCount;
        public ulong OtherOperationCount;
        public ulong ReadTransferCount;
        public ulong WriteTransferCount;
        public ulong OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
        public IO_COUNTERS IoInfo;
        public UIntPtr ProcessMemoryLimit;
        public UIntPtr JobMemoryLimit;
        public UIntPtr PeakProcessMemoryUsed;
        public UIntPtr PeakJobMemoryUsed;
    }

    private const int JobObjectExtendedLimitInformation = 9;
    private const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x2000;

    public static bool CreateAndBind() {
        IntPtr job = CreateJobObject(IntPtr.Zero, null);
        if (job == IntPtr.Zero) return false;

        var ext = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
        ext.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        int len = Marshal.SizeOf(typeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION));
        IntPtr ptr = Marshal.AllocHGlobal(len);
        try {
            Marshal.StructureToPtr(ext, ptr, false);
            if (!SetInformationJobObject(job, JobObjectExtendedLimitInformation, ptr, (uint)len)) return false;
        } finally {
            Marshal.FreeHGlobal(ptr);
        }

        if (!AssignProcessToJobObject(job, GetCurrentProcess())) return false;
        // Handle deliberately never closed: it is released when this process dies,
        // which is exactly what triggers KILL_ON_JOB_CLOSE.
        return true;
    }
}
'@

if ([JobKill]::CreateAndBind()) {
    Write-Host "[dev-run] Job Object active -- closing this window will kill vite + cargo + the app"
} else {
    Write-Host "[dev-run] WARN: Job Object setup failed (already inside a job?) -- descendants may survive a window close"
}

# ---- start the command as a child so we can watch the app alongside it ---------------
$child = Start-Process -FilePath 'cmd.exe' -ArgumentList '/c', $Command -PassThru -NoNewWindow
Write-Host "[dev-run] started (pid=$($child.Id)): $Command"

$appName = [System.IO.Path]::GetFileNameWithoutExtension($AppProcessName)
$appSeen = $false
$appExitedCleanly = $false
$deadline = (Get-Date).AddSeconds($AppStartTimeoutSec)

while (-not $child.HasExited) {
    $app = Get-Process -Name $appName -ErrorAction SilentlyContinue

    if ($app) {
        if (-not $appSeen) {
            $appSeen = $true
            Write-Host "[dev-run] app detected -- closing the app window will close this console too"
        }
    } elseif ($appSeen) {
        # The user closed the app. Give `tauri dev` a few seconds to unwind on its own
        # (it normally kills vite and exits), then force whatever is left.
        Write-Host "[dev-run] app exited -- shutting the dev chain down"
        Start-Sleep -Seconds 3
        if (-not $child.HasExited) {
            Stop-Process -Id $child.Id -Force -ErrorAction SilentlyContinue
        }
        $appExitedCleanly = $true
        break
    } elseif ((Get-Date) -gt $deadline) {
        # Never came up within the budget: stop watching, let the child finish/report.
        break
    }

    Start-Sleep -Milliseconds $PollMs
}

# ---- drain the child ----------------------------------------------------------------
if (-not $child.HasExited) {
    try { $child.WaitForExit(15000) | Out-Null } catch { }
}
if (-not $child.HasExited) {
    Stop-Process -Id $child.Id -Force -ErrorAction SilentlyContinue
}

$childCode = 0
try { $childCode = [int]$child.ExitCode } catch { $childCode = 0 }

if ($appExitedCleanly) {
    Write-Host "[dev-run] clean app-exit path -- dev.cmd will close without pausing"
    exit 0
}

Write-Host "[dev-run] command exited with code $childCode (no clean app-exit detected)"
if ($childCode -eq 0) { exit 0 } else { exit $childCode }
