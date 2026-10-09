# Checks the application manifest of a built Windows program (bundle-check.yml, for #48):
#
#   - exactly one manifest is embedded (build.rs makes the linker embed one and tauri-build skip
#     its own; two would be two RT_MANIFEST resources),
#   - it depends on Common Controls v6, which the dialogs and the tray need,
#   - it asks for no execution level above asInvoker (so no UAC prompt when it is started).
#
# Usage: check-exe-manifest.ps1 <path to the .exe>

param([Parameter(Mandatory = $true)][string]$Exe)

$ErrorActionPreference = 'Stop'

# Reads every RT_MANIFEST resource (type 24) of a program, in every language, without running it.
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class ExeManifests
{
    delegate bool EnumNames(IntPtr module, IntPtr type, IntPtr name, IntPtr param);
    delegate bool EnumLangs(IntPtr module, IntPtr type, IntPtr name, ushort lang, IntPtr param);

    [DllImport("kernel32", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr LoadLibraryEx(string file, IntPtr zero, uint flags);
    [DllImport("kernel32", SetLastError = true)]
    static extern bool FreeLibrary(IntPtr module);
    [DllImport("kernel32", SetLastError = true)]
    static extern bool EnumResourceNames(IntPtr module, IntPtr type, EnumNames callback, IntPtr param);
    [DllImport("kernel32", SetLastError = true)]
    static extern bool EnumResourceLanguages(IntPtr module, IntPtr type, IntPtr name, EnumLangs callback, IntPtr param);
    [DllImport("kernel32", SetLastError = true)]
    static extern IntPtr FindResourceEx(IntPtr module, IntPtr type, IntPtr name, ushort lang);
    [DllImport("kernel32", SetLastError = true)]
    static extern IntPtr LoadResource(IntPtr module, IntPtr resource);
    [DllImport("kernel32")]
    static extern IntPtr LockResource(IntPtr data);
    [DllImport("kernel32")]
    static extern uint SizeofResource(IntPtr module, IntPtr resource);

    const uint LOAD_LIBRARY_AS_DATAFILE = 0x2;
    static readonly IntPtr RT_MANIFEST = new IntPtr(24);

    public static List<string> Read(string path)
    {
        List<string> found = new List<string>();
        IntPtr module = LoadLibraryEx(path, IntPtr.Zero, LOAD_LIBRARY_AS_DATAFILE);
        if (module == IntPtr.Zero)
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        try
        {
            EnumLangs langs = null;
            EnumNames names = delegate(IntPtr m, IntPtr type, IntPtr name, IntPtr param)
            {
                langs = delegate(IntPtr m2, IntPtr type2, IntPtr name2, ushort lang, IntPtr param2)
                {
                    IntPtr resource = FindResourceEx(m2, type2, name2, lang);
                    IntPtr data = LockResource(LoadResource(m2, resource));
                    byte[] bytes = new byte[SizeofResource(m2, resource)];
                    Marshal.Copy(data, bytes, 0, bytes.Length);
                    found.Add(Encoding.UTF8.GetString(bytes).TrimStart('﻿'));
                    return true;
                };
                return EnumResourceLanguages(m, type, name, langs, IntPtr.Zero);
            };
            // No manifest at all makes this fail with ERROR_RESOURCE_TYPE_NOT_FOUND: no manifests.
            EnumResourceNames(module, RT_MANIFEST, names, IntPtr.Zero);
            GC.KeepAlive(names);
        }
        finally
        {
            FreeLibrary(module);
        }
        return found;
    }
}
'@

$path = (Resolve-Path -LiteralPath $Exe).Path
$manifests = @([ExeManifests]::Read($path))
Write-Host "$path has $($manifests.Count) embedded manifest(s)."
if ($manifests.Count -ne 1) {
    throw "Expected exactly one embedded manifest, found $($manifests.Count)."
}

$text = $manifests[0]
Write-Host $text
$xml = [xml]$text

$controls = @($xml.SelectNodes("//*[local-name()='dependentAssembly']/*[local-name()='assemblyIdentity'][@name='Microsoft.Windows.Common-Controls' and starts-with(@version, '6.')]"))
if ($controls.Count -ne 1) {
    throw 'The manifest has no dependency on Microsoft.Windows.Common-Controls version 6.'
}

$levels = @($xml.SelectNodes("//*[local-name()='requestedExecutionLevel']"))
foreach ($level in $levels) {
    if ($level.level -ne 'asInvoker') {
        throw "The manifest asks for execution level '$($level.level)', above asInvoker."
    }
}

Write-Host "OK: one manifest, Common Controls v6, $($levels.Count) requestedExecutionLevel element(s), none above asInvoker."
