using ManuHub.Ytdlp.NET;

namespace YtdlpCLI;

internal static class Program
{
    private const int BarWidth = 30;

    private static async Task<int> Main()
    {
        Console.Write("URL: ");
        var url = Console.ReadLine()?.Trim();
        if (string.IsNullOrWhiteSpace(url))
        {
            Console.WriteLine("URL is required.");
            return 1;
        }

        Console.Write("Save at: ");
        var folder = Console.ReadLine()?.Trim();
        if (string.IsNullOrWhiteSpace(folder))
        {
            Console.WriteLine("Save path is required.");
            return 1;
        }

        if (!Directory.Exists(folder))
        {
            Directory.CreateDirectory(folder);
        }

        var baseDir    = Directory.GetCurrentDirectory();
        var ytdlpPath  = Path.Combine(baseDir, "tools", "yt-dlp.exe");
        var ffmpegPath = Path.Combine(baseDir, "tools", "ffmpeg.exe");

        var ytdlp = new Ytdlp(ytdlpPath, new ConsoleLogger());

        // まずメタデータだけ取ってIDを知る
        var meta = await ytdlp.GetMetadataAsync(url);
        if (meta is null || string.IsNullOrWhiteSpace(meta.Id))
        {
            Console.WriteLine("Failed to get metadata (video id).");
            return 2;
        }

        var id = meta.Id;

        // 既存ファイルを全部削除（同じIDを含むもの）
        foreach (var file in Directory.EnumerateFiles(folder, $"*{id}*.*"))
        {
            try
            {
                File.Delete(file);
            }
            catch
            {
                // 消せないものがあっても続行
            }
        }

        // 進捗バー
        ytdlp.OnProgressDownload += (_, e) =>
        {
            var percent = e.Percent;
            if (percent < 0) percent = 0;
            if (percent > 100) percent = 100;

            var filled = (int)(percent / 100 * BarWidth);
            var bar    = "[" + new string('=', filled) + new string(' ', BarWidth - filled) + "]";

            Console.Write(
                "\r" +
                bar +
                $" {percent,5:F1}% {e.Speed,-12} ETA {e.ETA,-10}"
            );
        };

        ytdlp.OnCompleteDownload += (_, msg) =>
        {
            Console.WriteLine();
            Console.WriteLine($"Finished: {msg}");
        };

        try
        {
            await ytdlp
                .SetFormat("bestvideo+bestaudio/best")
                .SetOutputFolder(folder)
                .SetOutputTemplate("%(upload_date)s_%(id)s [%(resolution)s].%(ext)s")
                .EmbedMetadata()
                .EmbedThumbnail()
                .SetFfmpegLocation(ffmpegPath)
                .ExecuteAsync(url);

            return 0;
        }
        catch (Exception ex)
        {
            Console.WriteLine();
            Console.WriteLine("Error: " + ex.Message);
            return 2;
        }
    }
}
