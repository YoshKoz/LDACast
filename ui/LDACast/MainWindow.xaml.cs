using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using LDACast.Pages;

// To learn more about WinUI, the WinUI project structure,
// and more about our project templates, see: http://aka.ms/winui-project-info.

namespace LDACast;

public sealed partial class MainWindow : Window
{
    public MainWindow()
    {
        Mark("ctor-enter");
        InitializeComponent();
        Mark("init done");

        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Tall;
        AppWindow.SetIcon("Assets/AppIcon.ico");
        Mark("before-navigate");
        NavFrame.Navigate(typeof(DevicesPage));
        Mark("after-navigate");
    }

    private static void Mark(string s)
    {
        try { File.AppendAllText(Path.Combine(Path.GetTempPath(), "ldacwinui-startup.log"), $"{DateTime.Now:HH:mm:ss} {s}\n"); } catch { }
    }

    private void TitleBar_PaneToggleRequested(TitleBar sender, object args)
    {
        NavView.IsPaneOpen = !NavView.IsPaneOpen;
    }

    private void TitleBar_BackRequested(TitleBar sender, object args)
    {
        NavFrame.GoBack();
    }

    private void NavView_SelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.IsSettingsSelected)
        {
            NavFrame.Navigate(typeof(SettingsPage));
        }
        else if (args.SelectedItem is NavigationViewItem item)
        {
            switch (item.Tag)
            {
                case "devices":
                    NavFrame.Navigate(typeof(DevicesPage));
                    break;
                case "stream":
                    NavFrame.Navigate(typeof(StreamPage));
                    break;
                default:
                    throw new InvalidOperationException($"Unknown navigation item tag: {item.Tag}");
            }
        }
    }
}
