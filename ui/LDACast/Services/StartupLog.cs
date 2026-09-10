using System.IO;

namespace LDACast.Services;

/// <summary>
/// Best-effort trace of the WinUI startup sequence to %TEMP%, used to diagnose
/// launches that fail before any window is up.
/// </summary>
public static class StartupLog
{
    private static readonly string Path_ =
        Path.Combine(Path.GetTempPath(), "ldacwinui-startup.log");

    public static void Write(string what)
    {
        try
        {
            File.AppendAllText(Path_, $"{DateTime.Now:HH:mm:ss} {what}\n");
        }
        catch
        {
            // Diagnostics must never be the reason the app fails to start, and
            // there is nowhere left to report a failure of the log itself.
        }
    }
}
