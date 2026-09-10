using System.Diagnostics;
using LDACast.Models;

namespace LDACast.Services;

public sealed class Health
{
    public string State { get; init; } = "";
    public ulong BufferedMs { get; init; }
    public double Rt { get; init; }
    public ulong Fps { get; init; }
    public ulong Bpf { get; init; }
    public ulong Packets { get; init; }
    public ulong Frames { get; init; }
    public ulong Wire { get; init; }
    public long Bitrate { get; init; }
    public long Eqmid { get; init; }
    public ulong Over { get; init; }
    public ulong Under { get; init; }
    public ulong EncErr { get; init; }
    public ulong SendErr { get; init; }
    public ulong Reconnects { get; init; }

    private static ulong NumAfter(string s, string key)
    {
        var i = s.IndexOf(key, StringComparison.Ordinal);
        if (i < 0) return 0;
        return NumHead(s[(i + key.Length)..]);
    }

    private static ulong NumHead(string s)
    {
        var w = s.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries).FirstOrDefault() ?? "0";
        w = new string(w.TakeWhile(c => char.IsDigit(c)).ToArray());
        return ulong.TryParse(w, out var n) ? n : 0;
    }

    /// <summary>Parses "[Streaming] buffered 20 ms | rt 1.00x | ..." lines. Null for anything else.</summary>
    public static Health? Parse(string line)
    {
        if (!line.StartsWith('[')) return null;
        var end = line.IndexOf(']');
        if (end < 0) return null;
        var cols = line[(end + 1)..].Split('|');
        if (cols.Length < 8) return null;
        var fpsBpf = cols[2].Split(',');
        double rt = 0;
        var rtPart = cols[1].Split("rt").Skip(1).FirstOrDefault()?.Trim().TrimEnd('x');
        double.TryParse(rtPart, System.Globalization.NumberStyles.Float,
            System.Globalization.CultureInfo.InvariantCulture, out rt);
        return new Health
        {
            State = line[1..end],
            // "buffered" is in cols[0]; reading cols[1] silently pinned this to 0
            BufferedMs = NumAfter(cols[0], "buffered"),
            Rt = rt,
            Fps = NumHead(fpsBpf.FirstOrDefault() ?? "0"),
            Bpf = fpsBpf.Length > 1 ? NumAfter(fpsBpf[1], "") : 0,
            Packets = NumAfter(cols[3], "packets"),
            Frames = Math.Max(NumAfter(cols[3].Split("packets").Skip(1).FirstOrDefault() ?? "", "frames"), NumAfter(cols[3], "frames")),
            Wire = NumAfter(cols[4], "wire"),
            Bitrate = (long)NumAfter(cols[5], "ldac"),
            Eqmid = (long)(cols[5].Split("eqmid").Skip(1).Select(NumHead).FirstOrDefault()),
            Over = NumAfter(cols[6], "over"),
            Under = Math.Max(NumAfter(cols[6].Split("over").Skip(1).FirstOrDefault() ?? "", "under"), NumAfter(cols[6], "under")),
            EncErr = NumAfter(cols[7], "encerr"),
            SendErr = Math.Max(NumAfter(cols[7].Split("encerr").Skip(1).FirstOrDefault() ?? "", "senderr"), NumAfter(cols[7], "senderr")),
            Reconnects = cols.Length > 8 ? NumAfter(cols[8], "reconnects") : 0,
        };
    }
}

/// <summary>Owns the ldacsrc child process; parses its stdout into Health/Log events.</summary>
public sealed class StreamSession
{
    public static readonly StreamSession Instance = new();

    public event Action<Health>? HealthUpdated;
    public event Action<string>? Log;
    /// <summary>Exit code: 1 capture failed, 2 unusable mix format, 4 auto Windows fallback, 5 sink cannot carry LDAC.</summary>
    public event Action<int>? Exited;

    public static string ExitReason(int code) => code switch
    {
        0 => "ldacsrc exited",
        1 => "ldacsrc exited: WASAPI loopback capture failed",
        2 => "ldacsrc exited: the playback device's format is not one LDAC can carry",
        4 => "ldacsrc exited: stream lost, radio handed back to Windows",
        5 => "ldacsrc exited: this sink cannot carry LDAC from this capture format",
        _ => $"ldacsrc exited with code {code}",
    };

    /// <summary>Last known sink capabilities / negotiated config / capture format lines.</summary>
    public string SinkCaps { get; private set; } = "-";
    public string Negotiated { get; private set; } = "-";
    public string CaptureFmt { get; private set; } = "-";
    public string CurrentQuality { get; private set; } = "-";
    public string CurrentDevice { get; private set; } = "-";
    public event Action? DetailUpdated;

    private Process? _child;
    private CancellationTokenSource? _cts;
    public bool Running => _child is { HasExited: false };

    private StreamSession() { }

    public void Start(DeviceEntry dev, string capture, bool autoFallback)
    {
        if (Running) return;
        if (!File.Exists(Backend.LdacSrcExe))
        {
            Log?.Invoke($"build missing: {Backend.LdacSrcExe}");
            return;
        }
        try
        {
            var child = new Process
            {
                StartInfo = BuildStartInfo(dev, capture, autoFallback),
                EnableRaisingEvents = true,
            };
            _cts?.Dispose();
            _cts = new CancellationTokenSource();
            var ct = _cts.Token;
            child.Exited += (_, _) => { if (!ct.IsCancellationRequested) Exited?.Invoke(child.ExitCode); };
            child.Start();
            _child = child;

            CurrentQuality = dev.Quality.ToUpperInvariant() + (dev.Abr ? "+ABR" : "");
            CurrentDevice = $"{dev.Name} ({dev.Addr})";
            SinkCaps = "-";
            Negotiated = "-";
            CaptureFmt = "-";

            var capNote = string.IsNullOrWhiteSpace(capture) ? "default device" : $"capture \"{capture.Trim()}\"";
            Log?.Invoke($"streaming to {dev.Name} ({dev.Addr}) [{dev.Quality}{(dev.Abr ? "+abr" : "")}] via {capNote}{(autoFallback ? ", auto Windows fallback on" : "")}");

            Task.Run(() => Pump(child.StandardError, ct, l => Log?.Invoke(l)), ct);
            Task.Run(() => Pump(child.StandardOutput, ct, HandleStdoutLine), ct);
        }
        catch (Exception e)
        {
            Log?.Invoke($"spawn failed: {e.Message}");
        }
    }

    private static ProcessStartInfo BuildStartInfo(DeviceEntry dev, string capture, bool autoFallback)
    {
        var args = $"--addr {dev.Addr} --quality {dev.Quality}{(dev.Abr ? " --abr" : "")}"
            + (string.IsNullOrWhiteSpace(capture) ? "" : $" --capture \"{capture.Trim()}\"")
            + (autoFallback ? " --auto-bt-fallback 30" : "");
        return new ProcessStartInfo(Backend.LdacSrcExe, args)
        {
            WorkingDirectory = Backend.RepoRoot,
            RedirectStandardOutput = true,
            // ldacsrc reports capture failures, unusable mix formats and bad
            // arguments on stderr; without this the app shows nothing at all
            // when the engine exits before it ever streams.
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
    }

    /// <summary>Forwards one redirected pipe until the child closes it.</summary>
    private static void Pump(StreamReader pipe, CancellationToken ct, Action<string> onLine)
    {
        try
        {
            string? line;
            while (!ct.IsCancellationRequested && (line = pipe.ReadLine()) != null)
            {
                var t = line.Trim();
                if (t.Length > 0) onLine(t);
            }
        }
        catch
        {
            // The pipe is torn down when the child exits, or when the session is
            // stopped and kills it, so there is nobody left to report this to.
        }
    }

    /// <summary>Health lines drive the meters; the rest are detail or plain log.</summary>
    private void HandleStdoutLine(string line)
    {
        var h = Health.Parse(line);
        if (h != null)
        {
            HealthUpdated?.Invoke(h);
            return;
        }
        if (line.StartsWith("sink accepted", StringComparison.Ordinal)
            || line.StartsWith("configuring seid", StringComparison.Ordinal)) Negotiated = line;
        else if (line.StartsWith("sink ", StringComparison.Ordinal)) SinkCaps = line;
        else if (line.StartsWith("capture:", StringComparison.Ordinal)) CaptureFmt = line;
        else
        {
            Log?.Invoke(line);
            return;
        }
        DetailUpdated?.Invoke();
        Log?.Invoke(line);
    }

    public void Stop()
    {
        _cts?.Cancel();
        try
        {
            if (_child is { HasExited: false })
                _child.Kill(entireProcessTree: true);
        }
        catch
        {
            // The child may have exited between the check and the kill, or be
            // outside our rights to signal; either way it is no longer ours.
        }
        _child?.Dispose();
        _child = null;
        _cts?.Dispose();
        _cts = null;
    }
}
