// Minimal PE32+ import reader. No SDK/tool executable needed on the test machine.
// Includes delay imports; rejects malformed tables and every non-AMD64 image.
using System;
using System.Collections.Generic;
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
