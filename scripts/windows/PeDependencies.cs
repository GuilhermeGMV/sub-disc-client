// Minimal PE32+ import reader. No SDK/tool executable needed on the test machine.
// Includes delay imports; rejects malformed tables and every non-AMD64 image.
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

public static class SubdiscPe {
    public static string[] Imports(string path) {
        byte[] b = File.ReadAllBytes(path);
        Func<int, ushort> u16 = p => { Bounds(b, p, 2); return BitConverter.ToUInt16(b, p); };
        Func<int, uint> u32 = p => { Bounds(b, p, 4); return BitConverter.ToUInt32(b, p); };
        int opt = OptionalHeader(b, path);
        int pe = opt - 24;
        int sections = u16(pe + 6), optSize = u16(pe + 20);
        int table = opt + optSize;
        Bounds(b, table, sections * 40);
        Func<uint, int> offset = rva => {
            if (rva < u32(opt + 60)) { Bounds(b, checked((int)rva), 1); return (int)rva; }
            for (int i = 0; i < sections; ++i) {
                int s = table + i * 40;
                uint start = u32(s + 12), size = u32(s + 16);
                if (rva >= start && (ulong)rva < (ulong)start + size) {
                    int p = checked((int)(u32(s + 20) + (ulong)(rva - start)));
                    Bounds(b, p, 1); return p;
                }
            }
            throw new InvalidDataException("Unmapped import RVA: " + path);
        };
        var result = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (int dir in new int[] { 1, 13 }) {
            uint rva = u32(opt + 112 + dir * 8), size = u32(opt + 116 + dir * 8);
            if (rva == 0) continue;
            int stride = dir == 1 ? 20 : 32;
            bool ended = false;
            for (uint at = 0; at + stride <= size; at += (uint)stride) {
                int p = offset(checked(rva + at));
                Bounds(b, p, stride);
                uint name = u32(p + (dir == 1 ? 12 : 4));
                if (name == 0) { ended = true; break; }
                if (dir == 13 && (u32(p) & 1) == 0)
                    throw new InvalidDataException("Unsupported VA delay imports: " + path);
                int n = offset(name), end = n;
                while (end < b.Length && b[end] != 0 && end - n < 260) ++end;
                if (end == b.Length || end - n == 260)
                    throw new InvalidDataException("Unterminated DLL name: " + path);
                string dll = Encoding.ASCII.GetString(b, n, end - n).ToLowerInvariant();
                if ((!dll.EndsWith(".dll") && !dll.EndsWith(".drv")) || dll.IndexOfAny(new char[] {'/', '\\', ':'}) >= 0)
                    throw new InvalidDataException("Invalid import name: " + path);
                result.Add(dll);
            }
            if (!ended) throw new InvalidDataException("Unterminated import table: " + path);
        }
        var values = new string[result.Count]; result.CopyTo(values); Array.Sort(values);
        return values;
    }
    // MajorLinkerVersion.MinorLinkerVersion: the MSVC toolset that linked the image.
    // An app-local CRT must be at least as new as every component linked against it.
    public static Version LinkerVersion(string path) {
        byte[] b = File.ReadAllBytes(path);
        int opt = OptionalHeader(b, path);
        return new Version(b[opt + 2], b[opt + 3]);
    }
    static int OptionalHeader(byte[] b, string path) {
        Bounds(b, 0, 64);
        if (BitConverter.ToUInt16(b, 0) != 0x5a4d) throw new InvalidDataException("Not PE: " + path);
        int pe = checked((int)BitConverter.ToUInt32(b, 0x3c));
        Bounds(b, pe, 24);
        if (BitConverter.ToUInt32(b, pe) != 0x4550 || BitConverter.ToUInt16(b, pe + 4) != 0x8664)
            throw new InvalidDataException("Expected Windows AMD64: " + path);
        int opt = pe + 24, optSize = BitConverter.ToUInt16(b, pe + 20);
        Bounds(b, opt, 224);
        if (BitConverter.ToUInt16(b, opt) != 0x20b || optSize < 224 || BitConverter.ToUInt32(b, opt + 108) < 14)
            throw new InvalidDataException("Invalid PE32+ optional header: " + path);
        return opt;
    }
    static void Bounds(byte[] bytes, int offset, int count) {
        if (offset < 0 || count < 0 || (long)offset + count > bytes.Length)
            throw new InvalidDataException("Truncated PE image");
    }
}

// Inherited by child processes: a missing DLL ends the child with an exit code
// (0xC0000135) instead of a modal loader dialog that would stall automation.
public static class SubdiscErrorMode {
    [DllImport("kernel32.dll")]
    public static extern uint SetErrorMode(uint mode);
    public const uint Quiet = 0x0001 | 0x0002 | 0x8000;
}

// Runs a program with stdout/stderr in a file, never a pipe: GLib-based tools
// (gst-inspect 1.28.8) were seen hanging with their output on a pipe. The
// process starts suspended inside a job object, so a timeout also ends the
// children it spawned (gst-plugin-scanner). C# 5 syntax for Windows PowerShell.
public static class SubdiscProcess {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct StartupInfo {
        public int cb;
        public string lpReserved;
        public string lpDesktop;
        public string lpTitle;
        public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public short wShowWindow, cbReserved2;
        public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct ProcessInformation {
        public IntPtr hProcess, hThread;
        public int dwProcessId, dwThreadId;
    }
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CreateProcessW(string applicationName, StringBuilder commandLine, IntPtr processAttributes,
        IntPtr threadAttributes, bool inheritHandles, uint creationFlags, IntPtr environment, string currentDirectory,
        ref StartupInfo startupInfo, out ProcessInformation processInformation);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetHandleInformation(IntPtr handle, uint mask, uint flags);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr CreateJobObjectW(IntPtr attributes, string name);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateJobObject(IntPtr job, uint exitCode);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateProcess(IntPtr process, uint exitCode);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetExitCodeProcess(IntPtr process, out uint exitCode);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CloseHandle(IntPtr handle);

    const uint CreateSuspended = 0x00000004, CreateUnicodeEnvironment = 0x00000400, CreateNoWindow = 0x08000000;
    const int StartfUseStdHandles = 0x00000100;
    const uint HandleFlagInherit = 0x00000001, WaitObject0 = 0, Infinite = 0xFFFFFFFF;

    // Returns the exit code (and the process id, e.g. to tell its log files from
    // those of its children). Throws Win32Exception if the process cannot be
    // started and TimeoutException (after ending the process tree) on timeout.
    public static int Run(string fileName, string arguments, string workingDirectory,
                          System.Collections.IDictionary environment, string logPath, int timeoutMilliseconds,
                          out int processId) {
        processId = 0;
        var names = new List<string>();
        foreach (System.Collections.DictionaryEntry entry in environment) {
            string name = (string)entry.Key;
            if (entry.Value == null || name.Length == 0 || name.IndexOf('=') >= 0 || name.IndexOf('\0') >= 0)
                throw new ArgumentException("Invalid environment variable: " + name);
            names.Add(name);
        }
        names.Sort(StringComparer.OrdinalIgnoreCase);
        var block = new StringBuilder();
        foreach (string name in names) block.Append(name).Append('=').Append((string)environment[name]).Append('\0');
        block.Append('\0');
        var commandLine = new StringBuilder("\"" + fileName + "\"" + (string.IsNullOrEmpty(arguments) ? "" : " " + arguments));
        var info = new ProcessInformation();
        IntPtr job = IntPtr.Zero;
        using (var log = new FileStream(logPath, FileMode.Create, FileAccess.Write, FileShare.ReadWrite | FileShare.Delete)) {
            IntPtr handle = log.SafeFileHandle.DangerousGetHandle();
            if (!SetHandleInformation(handle, HandleFlagInherit, HandleFlagInherit))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "SetHandleInformation");
            var startup = new StartupInfo();
            startup.cb = Marshal.SizeOf(typeof(StartupInfo));
            startup.dwFlags = StartfUseStdHandles;
            startup.hStdInput = IntPtr.Zero;
            startup.hStdOutput = handle;
            startup.hStdError = handle;
            IntPtr environmentBlock = Marshal.StringToHGlobalUni(block.ToString());
            try {
                if (!CreateProcessW(fileName, commandLine, IntPtr.Zero, IntPtr.Zero, true,
                        CreateSuspended | CreateUnicodeEnvironment | CreateNoWindow, environmentBlock,
                        workingDirectory, ref startup, out info))
                {
                    int error = Marshal.GetLastWin32Error();
                    throw new Win32Exception(error, "CreateProcess failed (" + new Win32Exception(error).Message + "): " + fileName);
                }
            } finally {
                Marshal.FreeHGlobal(environmentBlock);
            }
        }
        processId = info.dwProcessId;
        try {
            job = CreateJobObjectW(IntPtr.Zero, null);
            bool inJob = job != IntPtr.Zero && AssignProcessToJobObject(job, info.hProcess);
            if (ResumeThread(info.hThread) == Infinite) {
                int error = Marshal.GetLastWin32Error();
                TerminateProcess(info.hProcess, 1);
                throw new Win32Exception(error, "ResumeThread failed: " + fileName);
            }
            if (WaitForSingleObject(info.hProcess, (uint)timeoutMilliseconds) != WaitObject0) {
                if (inJob) TerminateJobObject(job, 1); else TerminateProcess(info.hProcess, 1);
                WaitForSingleObject(info.hProcess, 10000);
                throw new TimeoutException("Timed out after " + (timeoutMilliseconds / 1000) + "s: " + fileName + " " + arguments);
            }
            uint exitCode;
            if (!GetExitCodeProcess(info.hProcess, out exitCode))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "GetExitCodeProcess");
            return unchecked((int)exitCode);
        } finally {
            if (info.hThread != IntPtr.Zero) CloseHandle(info.hThread);
            if (info.hProcess != IntPtr.Zero) CloseHandle(info.hProcess);
            if (job != IntPtr.Zero) CloseHandle(job);
        }
    }
}
