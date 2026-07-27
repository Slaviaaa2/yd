namespace YtdlpCLI;

internal static class ToolDownloader
{
    private const string YtdlpDownloadUrl =
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";

    private static readonly HttpClient HttpClient = CreateHttpClient();

    public static async Task<string> EnsureYtdlpAsync(string applicationDirectory)
    {
        var toolsDirectory = Path.Combine(applicationDirectory, "tools");
        var ytdlpPath = Path.Combine(toolsDirectory, "yt-dlp.exe");

        if (File.Exists(ytdlpPath))
            return ytdlpPath;

        Directory.CreateDirectory(toolsDirectory);

        var temporaryPath = ytdlpPath + $".{Guid.NewGuid():N}.download";
        try
        {
            Console.WriteLine("yt-dlp.exe が見つからないため、最新版をダウンロードしています...");

            using var response = await HttpClient.GetAsync(
                YtdlpDownloadUrl,
                HttpCompletionOption.ResponseHeadersRead);
            response.EnsureSuccessStatusCode();

            await using (var source = await response.Content.ReadAsStreamAsync())
            await using (var destination = new FileStream(
                             temporaryPath,
                             FileMode.CreateNew,
                             FileAccess.Write,
                             FileShare.None,
                             bufferSize: 81920,
                             useAsync: true))
            {
                await source.CopyToAsync(destination);
            }

            File.Move(temporaryPath, ytdlpPath, overwrite: true);
            Console.WriteLine("yt-dlp.exe の準備が完了しました。");
            return ytdlpPath;
        }
        finally
        {
            if (File.Exists(temporaryPath))
                File.Delete(temporaryPath);
        }
    }

    private static HttpClient CreateHttpClient()
    {
        var client = new HttpClient
        {
            Timeout = TimeSpan.FromMinutes(5)
        };
        client.DefaultRequestHeaders.UserAgent.ParseAdd("YtdlpCLI/1.0");
        return client;
    }
}
