using ManuHub.Ytdlp.NET;

namespace YtdlpCLI;

internal sealed class ConsoleLogger : ILogger
{
    public void Log(LogType logType, string message)
    {
        // Error / Warning だけ出す（Info は無視）
        if (logType == LogType.Error || logType == LogType.Warning)
        {
            Console.WriteLine();
            Console.WriteLine($"[{logType}] {message}");
        }
    }
}