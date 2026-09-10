using System.Collections.ObjectModel;
using LDACast.Models;
using LDACast.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace LDACast.Pages;

public sealed partial class DevicesPage : Page
{
    private Settings _settings = new();
    private readonly ObservableCollection<DeviceEntry> _view = new();

    public DevicesPage()
    {
        try { File.AppendAllText(Path.Combine(Path.GetTempPath(), "ldacwinui-startup.log"), $"{DateTime.Now:HH:mm:ss} devices-ctor\n"); } catch { }
        InitializeComponent();
        DeviceGrid.ItemsSource = _view;
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        _settings = Backend.LoadSettings();
        ApplyFilter();
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        Backend.SaveSettings(_settings);
    }

    private void ApplyFilter()
    {
        var q = SearchBox.Text.Trim();
        _view.Clear();
        foreach (var d in _settings.Devices)
        {
            if (q.Length == 0
                || d.Name.Contains(q, StringComparison.OrdinalIgnoreCase)
                || d.Addr.Contains(q, StringComparison.OrdinalIgnoreCase))
                _view.Add(d);
        }
    }

    private void SearchBox_TextChanged(object sender, TextChangedEventArgs e) => ApplyFilter();

    private void Add_Click(object sender, RoutedEventArgs e)
    {
        var addr = NewAddrBox.Text.Trim().ToUpperInvariant();
        var hex = new string(addr.Where(Uri.IsHexDigit).ToArray());
        if (hex.Length != 12) return;
        addr = string.Join(":", Enumerable.Range(0, 6).Select(i => hex.Substring(2 * i, 2)));
        _settings.Devices.Add(new DeviceEntry
        {
            Name = string.IsNullOrWhiteSpace(NewNameBox.Text) ? addr : NewNameBox.Text.Trim(),
            Addr = addr,
            Quality = "sq",
        });
        NewNameBox.Text = "";
        NewAddrBox.Text = "";
        Backend.SaveSettings(_settings);
        ApplyFilter();
    }

    private void Remove_Click(object sender, RoutedEventArgs e)
    {
        if (DeviceGrid.SelectedItem is DeviceEntry d && _settings.Devices.Count > 1)
        {
            _settings.Devices.Remove(d);
            _settings.Selected = 0;
            Backend.SaveSettings(_settings);
            ApplyFilter();
        }
    }

    private void QualityBox_Loaded(object sender, RoutedEventArgs e)
    {
        if (sender is ComboBox box && box.Tag is DeviceEntry d)
        {
            for (int i = 0; i < box.Items.Count; i++)
            {
                if (box.Items[i] is ComboBoxItem item && (item.Tag as string) == d.Quality)
                {
                    box.SelectedIndex = i;
                    break;
                }
            }
        }
    }

    private void QualityBox_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (sender is ComboBox box && box.Tag is DeviceEntry d
            && box.SelectedItem is ComboBoxItem item && item.Tag is string q)
        {
            d.Quality = q;
            Backend.SaveSettings(_settings);
        }
    }

    private void Stream_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as Button)?.Tag is not DeviceEntry d) return;
        _settings.Selected = Math.Max(0, _settings.Devices.IndexOf(d));
        Backend.SaveSettings(_settings);
        Frame.Navigate(typeof(StreamPage), d.Addr);
    }
}
