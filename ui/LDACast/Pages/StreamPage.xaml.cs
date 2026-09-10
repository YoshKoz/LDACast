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
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        _settings = Backend.LoadSettings();
        DevicePicker.ItemsSource = _settings.Devices;
        DevicePicker.DisplayMemberPath = "Name";
        DevicePicker.SelectedIndex = Math.Clamp(_settings.Selected, 0, _settings.Devices.Count - 1);
        CaptureBox.Text = _settings.Capture;
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
                StreamSession.Instance.Start(dev, _settings.Capture);
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

    private void CaptureBox_TextChanged(object sender, TextChangedEventArgs e)
    {
        _settings.Capture = CaptureBox.Text ?? "";
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
        StreamSession.Instance.Start(dev, _settings.Capture);
        RefreshButtons();
    }

    private void Stop_Click(object sender, RoutedEventArgs e)
    {
        StreamSession.Instance.Stop();
        AddEvent("stopped");
        RefreshButtons();
    }

    private void OnHealth(Health h)
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            StateText.Text = h.State;
            WireText.Text = $"{h.Wire} kbps";
            RtText.Text = $"{h.Rt:F2}x";
            BufText.Text = $"{h.BufferedMs} ms";
            CodecText.Text = h.Bitrate > 0 ? $"LDAC {h.Bitrate} kbps eqmid {h.Eqmid}" : "-";
            PacketsText.Text = $"{h.Packets}";
            OverText.Text = $"{h.Over}";
            UnderText.Text = $"{h.Under}";
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
