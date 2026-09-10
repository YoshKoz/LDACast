using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Data;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Navigation;

// To learn more about WinUI, the WinUI project structure,
// and more about our project templates, see: http://aka.ms/winui-project-info.

namespace LDACast;

/// <summary>
/// Provides application-specific behavior to supplement the default Application class.
/// </summary>
public partial class App : Application
{
    private Window? _window;
    
    /// <summary>
    /// Initializes the singleton application object.  This is the first line of authored code
    /// executed, and as such is the logical equivalent of main() or WinMain().
    /// </summary>
    public App()
    {
        InitializeComponent();
        UnhandledException += (s, e) =>
        {
            try { File.AppendAllText(Path.Combine(Path.GetTempPath(), "ldacwinui-startup.log"), $"{DateTime.Now:HH:mm:ss} UNHANDLED {e.Exception}\n"); } catch { }
        };
    }

    /// <summary>
    /// Invoked when the application is launched.
    /// </summary>
    /// <param name="args">Details about the launch request and process.</param>
    protected override void OnLaunched(Microsoft.UI.Xaml.LaunchActivatedEventArgs args)
    {
        try { File.AppendAllText(Path.Combine(Path.GetTempPath(), "ldacwinui-startup.log"), $"{DateTime.Now:HH:mm:ss} launched\n"); } catch { }
        _window = new MainWindow();
        _window.Activate();
        try { File.AppendAllText(Path.Combine(Path.GetTempPath(), "ldacwinui-startup.log"), $"{DateTime.Now:HH:mm:ss} activated\n"); } catch { }
    }
}
