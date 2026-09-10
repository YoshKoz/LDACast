using LDACast.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace LDACast.Pages;

public sealed partial class SettingsPage : Page
{
    public SettingsPage()
    {
        InitializeComponent();
        PathsText.Text = $"ldacsrc: {Backend.LdacSrcExe}\nswitch script: {Backend.LdacModePs1}\nrepo: {Backend.RepoRoot}";
        Refresh();
    }

    private void Refresh()
    {
        BusyRing.IsActive = true;
        var (mode, raw) = Backend.RadioStatus();
        ModeText.Text = mode;
        DetailText.Text = raw;
        BusyRing.IsActive = false;
    }

    private void Refresh_Click(object sender, RoutedEventArgs e) => Refresh();

    private void Switch(string target)
    {
        BusyRing.IsActive = true;
        ModeText.Text = "switching…";
        Backend.SwitchRadio(target, result =>
        {
            DispatcherQueue.TryEnqueue(() =>
            {
                ModeText.Text = result.Mode;
                DetailText.Text = result.Raw;
                BusyRing.IsActive = false;
            });
        });
    }

    private void ToWindows_Click(object sender, RoutedEventArgs e) => Switch("bt");
    private void ToLdac_Click(object sender, RoutedEventArgs e) => Switch("ldac");
}
