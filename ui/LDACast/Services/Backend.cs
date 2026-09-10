using System.Diagnostics;
using System.Text;
using System.Text.Json;
using LDACast.Models;

namespace LDACast.Services;

/// <summary>
/// Paths, settings persistence, Windows-paired list and the radio switch.
/// Reuses tools/ldacmode.ps1 (which self-elevates), so this app never
/// needs to run as admin itself.
/// </summary>
public static class Backend
{
    public static readonly string RepoRoot = FindRepo();
    public static readonly string LdacSrcExe = FindLdacSrc();
    public static readonly string LdacModePs1 = Path.Combine(RepoRoot, "tools", "ldacmode.ps1");

    private static string SettingsPath =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "ldac-win", "settings.json");

    private static string FindRepo()
    {
        var dir = AppContext.BaseDirectory;
        while (dir != null)
        {
            if (File.Exists(Path.Combine(dir, "tools", "ldacmode.ps1")))
                return dir;
            dir = Path.GetDirectoryName(dir.TrimEnd(Path.DirectorySeparatorChar));
        }
        const string fallback = @"C:\Development\projects\audio-bt\ldac-win";
        if (File.Exists(Path.Combine(fallback, "tools", "ldacmode.ps1")))
            return fallback;
        return AppContext.BaseDirectory;
    }

    private static string FindLdacSrc()
    {
        var cands = new[]
        {
            Path.Combine(AppContext.BaseDirectory, "ldacsrc.exe"),
            Path.Combine(RepoRoot, "target", "release", "ldacsrc.exe"),
        };
        return cands.FirstOrDefault(File.Exists) ?? cands[1];
    }

    public static Settings LoadSettings()
    {
        Settings? s = null;
        try
        {
            s = JsonSerializer.Deserialize<Settings>(File.ReadAllText(SettingsPath));
        }
        catch { }
        if (s is null)
        {
            // migrate the egui panel's file if present
            try
            {
                var egui = Path.Combine(Path.GetDirectoryName(LdacSrcExe) ?? "", "ldacgui.json");
                if (File.Exists(egui))
                    s = JsonSerializer.Deserialize<Settings>(File.ReadAllText(egui));
            }
            catch { }
        }
        s ??= new Settings();
        // drop junk rows (blank addr) that render as empty cards
        s.Devices.RemoveAll(d => string.IsNullOrWhiteSpace(d.Addr));
        foreach (var d in s.Devices)
        {
            d.Addr = d.Addr.Trim().ToUpperInvariant();
            d.Name = string.IsNullOrWhiteSpace(d.Name) ? d.Addr : d.Name.Trim();
            if (d.Quality is not ("hq" or "sq" or "mq")) d.Quality = "sq";
        }
        if (s.Devices.Count == 0)
        {
            s.Devices.Add(new DeviceEntry { Name = "WH-1000XM3", Addr = "14:3F:A6:35:D0:AA", Quality = "sq" });
        }
        s.Selected = Math.Clamp(s.Selected, 0, s.Devices.Count - 1);
        return s;
    }

    public static void SaveSettings(Settings s)
    {
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(SettingsPath)!);
            File.WriteAllText(SettingsPath, JsonSerializer.Serialize(s, new JsonSerializerOptions { WriteIndented = true }));
        }
        catch { }
    }

    private static string? FmtAddr(string key)
    {
        if (key.Length != 12 || !key.All(Uri.IsHexDigit)) return null;
        return string.Join(":", Enumerable.Range(0, 6).Select(i => key.Substring(2 * i, 2).ToUpperInvariant()));
    }

    private static string DecodeName(string kind, string hex)
    {
        var bytes = new List<byte>();
        for (int i = 0; i + 1 < hex.Length; i += 2)
        {
            if (byte.TryParse(hex.Substring(i, 2), System.Globalization.NumberStyles.HexNumber, null, out var b))
                bytes.Add(b);
        }
        if (kind is "REG_SZ" or "REG_EXPAND_SZ")
            return Encoding.UTF8.GetString(bytes.ToArray()).Trim('\0');
        var chars = new char[bytes.Count / 2];
        Buffer.BlockCopy(bytes.ToArray(), 0, chars, 0, chars.Length * 2);
        return new string(chars).Trim('\0').Trim();
    }

    /// <summary>(addr, name) from HKLM\...\BTHPORT\Parameters\Devices. Empty when the radio is in LDAC mode.</summary>
    public static List<(string Addr, string Name)> WinPairedDevices()
    {
        var devs = new List<(string, string)>();
        try
        {
            using var p = new Process();
            p.StartInfo = new ProcessStartInfo("reg", @"query HKLM\SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters\Devices /s")
            {
                RedirectStandardOutput = true,
                UseShellExecute = false,
                CreateNoWindow = true,
            };
            p.Start();
            var text = p.StandardOutput.ReadToEnd();
            p.WaitForExit(10000);
            string? cur = null;
            foreach (var raw in text.Split('\n'))
            {
                var t = raw.Trim();
                if (t.StartsWith("HKEY_", StringComparison.Ordinal))
                {
                    cur = FmtAddr(t.Split('\\').Last());
                }
                else if (cur is not null)
                {
                    var parts = t.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);
                    if (parts.Length >= 3 && parts[0] == "Name")
                    {
                        var name = DecodeName(parts[1], string.Concat(parts.Skip(2)));
                        if (!string.IsNullOrEmpty(name))
                            devs.Add((cur, name));
                        cur = null;
                    }
                }
            }
        }
        catch { }
        return devs;
    }

    private static (string Mode, string Raw) RunLdacMode(string verb)
    {
        try
        {
            using var p = new Process();
            p.StartInfo = new ProcessStartInfo("pwsh",
                $"-NoProfile -ExecutionPolicy Bypass -File \"{LdacModePs1}\" {verb}")
            {
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true,
            };
            p.Start();
            var stdout = p.StandardOutput.ReadToEnd();
            var stderr = p.StandardError.ReadToEnd();
            p.WaitForExit(120000);
            var raw = stdout + stderr;
            var mode = raw.Split('\n')
                .Select(l => l.Trim())
                .FirstOrDefault(l => l.StartsWith("mode:"))?
                .Substring(5).Trim() ?? $"exit {p.ExitCode}";
            return (mode, raw);
        }
        catch (Exception e)
        {
            return ("error", e.Message);
        }
    }

    public static (string Mode, string Raw) RadioStatus() => RunLdacMode("status");

    /// <summary>Runs the switch on a worker thread; ldacmode.ps1 elevates itself via UAC.</summary>
    public static void SwitchRadio(string target, Action<(string Mode, string Raw)> done)
    {
        Task.Run(() => done(RunLdacMode(target)));
    }
}
