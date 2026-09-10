using LDACast.Models;
using LDACast.Services;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace LDACast.Pages;

public sealed partial class StreamPage : Page
{
    private Settings _settings = new();
    private ulong _lastOver;
    private string? _pendingAddr;

    public StreamPage()
    {
        InitializeComponent();
        StreamSession.Instance.HealthUpdated += OnHealth;
        StreamSession.Instance.Log += OnLog;
        StreamSession.Instance.Exited += OnExited;
        StreamSession.Instance.DetailUpdated += OnDetail;
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        _settings = Backend.LoadSettings();
        DevicePicker.ItemsSource = _settings.Devices;
        DevicePicker.DisplayMemberPath = "Name";
        DevicePicker.SelectedIndex = Math.Clamp(_settings.Selected, 0, _settings.Devices.Count - 1);
        CapturePicker.ItemsSource = Backend.ListCaptureEndpoints().Select(e => e.Name).ToList();
        CapturePicker.Text = _settings.Capture;
        FallbackBox.IsChecked = _settings.AutoFallback;
        _pendingAddr = e.Parameter as string;
        RefreshButtons();
        if (_pendingAddr is { Length: > 0 } addr)
        {
            _pendingAddr = null;
            var dev = _settings.Devices.FirstOrDefault(d => d.Addr == addr);
            if (dev is not null)
            {
                DevicePicker.SelectedItem = dev;
                _settings.Selected = Math.Max(0, _settings.Devices.IndexOf(dev));
                Backend.SaveSettings(_settings);
                _lastOver = 0;
                StreamSession.Instance.Start(dev, _settings.Capture, _settings.AutoFallback);
                RefreshButtons();
            }
        }
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        Backend.SaveSettings(_settings);
    }

    private void RefreshButtons()
    {
        var running = StreamSession.Instance.Running;
        StartBtn.IsEnabled = !running;
        StopBtn.IsEnabled = running;
    }

    private DeviceEntry? Current =>
        DevicePicker.SelectedItem as DeviceEntry
        ?? (_settings.Devices.Count > 0 ? _settings.Devices[Math.Clamp(_settings.Selected, 0, _settings.Devices.Count - 1)] : null);

    private void CapturePicker_Changed(object sender, RoutedEventArgs e)
    {
        _settings.Capture = CapturePicker.Text ?? "";
        Backend.SaveSettings(_settings);
    }

    private void FallbackBox_Changed(object sender, RoutedEventArgs e)
    {
        _settings.AutoFallback = FallbackBox.IsChecked == true;
        Backend.SaveSettings(_settings);
    }

    private void Start_Click(object sender, RoutedEventArgs e)
    {
        var dev = Current;
        if (dev is null) return;
        _settings.Selected = Math.Max(0, _settings.Devices.IndexOf(dev));
        Backend.SaveSettings(_settings);
        _lastOver = 0;
        HintText.Visibility = Visibility.Collapsed;
        StreamSession.Instance.Start(dev, _settings.Capture, _settings.AutoFallback);
        RefreshButtons();
    }

    private void Stop_Click(object sender, RoutedEventArgs e)
    {
        StreamSession.Instance.Stop();
        AddEvent("stopped");
        RefreshButtons();
    }

    /// <summary>AltA2DP's "Load safe parameters": most compatible rung, no ABR.</summary>
    private void Safe_Click(object sender, RoutedEventArgs e)
    {
        var dev = Current;
        if (dev is null) return;
        dev.Quality = "mq";
        dev.Abr = false;
        Backend.SaveSettings(_settings);
        AddEvent($"safe parameters loaded for {dev.Name} (mq, ABR off) - applies on next Start");
    }

    private void Sound_Click(object sender, RoutedEventArgs e)
    {
        try
        {
            System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo("control", "mmsys.cpl,,0") { UseShellExecute = true });
        }
        catch { }
    }

    private void OnDetail()
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            SinkText.Text = StreamSession.Instance.SinkCaps;
            ConfigText.Text = StreamSession.Instance.Negotiated;
            var cap = StreamSession.Instance.CaptureFmt;
            CaptureText.Text = cap.StartsWith("capture: ") ? cap.Substring(9) : cap;
        });
    }

    private void OnHealth(Health h)
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            StateText.Text = h.State;
            WireText.Text = $"{h.Wire} kbps";
            RtText.Text = $"{h.Rt:F2}x";
            BufText.Text = $"{h.BufferedMs} ms";
            CodecText.Text = h.Bitrate > 0 ? $"LDAC {h.Bitrate}/{h.Eqmid}" : "-";
            PacketsText.Text = $"{h.Packets}";
            OverText.Text = $"{h.Over}";
            UnderText.Text = $"{h.Under}";
            QualityText.Text = StreamSession.Instance.CurrentQuality;
            EncoderText.Text = h.Bitrate > 0 ? $"{h.Bitrate} kbps (eqmid {h.Eqmid})" : "-";
            FramesText.Text = $"{h.Fps}/s, {h.Bpf} B";
            ReconnectsText.Text = $"{h.Reconnects}";
            SinkText.Text = StreamSession.Instance.SinkCaps;
            var cfg = StreamSession.Instance.Negotiated;
            ConfigText.Text = cfg;
            var cap = StreamSession.Instance.CaptureFmt;
            CaptureText.Text = cap.StartsWith("capture: ") ? cap.Substring(9) : cap;
            if (h.Over > _lastOver && _lastOver > 0)
            {
                HintText.Text = "overruns rising: link can't hold this bitrate — drop a rung (hq > sq > mq)";
                HintText.Visibility = Visibility.Visible;
            }
            _lastOver = h.Over;
        });
    }

    private void OnLog(string line)
    {
        DispatcherQueue.TryEnqueue(() => AddEvent(line));
    }

    private void OnExited()
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            AddEvent("ldacsrc exited");
            RefreshButtons();
        });
    }

    private void AddEvent(string line)
    {
        EventList.Items.Add(line);
        while (EventList.Items.Count > 300)
            EventList.Items.RemoveAt(0);
        EventList.ScrollIntoView(EventList.Items[^1]);
    }
}
