using ManuHub.Ytdlp.NET;

namespace YtdlpCLI;

/// <summary>Ytdlp.NET のログのうち、Error / Warning だけを標準エラーへ出す。</summary>
internal sealed class ConsoleLogger : ILogger
{
    public void Log(LogType logType, string message)
    {
        if (logType is not (LogType.Error or LogType.Warning))
            return;

        // yt-dlp 自身の出力は既に "ERROR:" / "WARNING:" で始まるので、二重にラベルを付けない
        var alreadyLabelled =
            message.StartsWith("ERROR:", StringComparison.OrdinalIgnoreCase) ||
            message.StartsWith("WARNING:", StringComparison.OrdinalIgnoreCase);

        Console.Error.WriteLine();
        Console.Error.WriteLine(alreadyLabelled ? message : $"[{logType}] {message}");
    }
}
