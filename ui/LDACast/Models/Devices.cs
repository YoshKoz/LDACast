using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Text.Json.Serialization;

namespace LDACast.Models;

public sealed class DeviceEntry : INotifyPropertyChanged
{
    private string _quality = "sq";
    private bool _abr;

    public string Name { get; set; } = "";
    public string Addr { get; set; } = "";

    public string Quality
    {
        get => _quality;
        set { if (_quality != value) { _quality = value; OnPropertyChanged(); } }
    }

    public bool Abr
    {
        get => _abr;
        set { if (_abr != value) { _abr = value; OnPropertyChanged(); } }
    }

    public event PropertyChangedEventHandler? PropertyChanged;
    private void OnPropertyChanged([CallerMemberName] string? p = null)
        => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(p));
}

public sealed class Settings
{
    [JsonPropertyName("devices")]
    public List<DeviceEntry> Devices { get; set; } = new();

    [JsonPropertyName("selected")]
    public int Selected { get; set; }

    /// <summary>Render endpoint name fragment to capture (e.g. a virtual cable).
    /// Blank means the Windows default playback device.</summary>
    [JsonPropertyName("capture")]
    public string Capture { get; set; } = "";

    /// <summary>Hand the radio back to Windows after a lost stream (30 s).</summary>
    [JsonPropertyName("auto_fallback")]
    public bool AutoFallback { get; set; }
}
