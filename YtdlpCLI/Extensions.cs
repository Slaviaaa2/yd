using System.Text;
using ManuHub.Ytdlp.NET;

namespace YtdlpCLI;

internal static class YtdlpExtensions
{
    public static Ytdlp SetFfmpegLocation(this Ytdlp ytdlp, string ffmpegPath)
    {
        if (string.IsNullOrWhiteSpace(ffmpegPath))
            throw new ArgumentException("ffmpeg path cannot be empty.", nameof(ffmpegPath));

        var field = typeof(Ytdlp).GetField(
            "_commandBuilder",
            System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance
        );

        if (field?.GetValue(ytdlp) is not StringBuilder commandBuilder)
            throw new InvalidOperationException("Failed to access command builder.");

        var handler = new StringBuilder.AppendInterpolatedStringHandler(20, 1, commandBuilder);
        handler.AppendLiteral("--ffmpeg-location ");
        handler.AppendFormatted(ffmpegPath);
        handler.AppendLiteral(" ");
        commandBuilder.Append(ref handler);

        return ytdlp;
    }
}