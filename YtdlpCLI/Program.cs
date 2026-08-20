using System.Text;
using ManuHub.Ytdlp.NET;

namespace YtdlpCLI;

internal static class Program
{
    private const int ExitSuccess = 0;
    private const int ExitUsage = 1;
    private const int ExitFailure = 2;
    private const int ExitCancelled = 130;

    private static readonly string[] SupportedAudioFormats =
        ["aac", "alac", "flac", "m4a", "mp3", "opus", "vorbis", "wav"];

    private static async Task<int> Main(string[] args)
    {
        TrySetUtf8Output();

        CommandLineOptions options;
        try
        {
            options = CommandLineOptions.Parse(args);
        }
        catch (ArgumentException ex)
        {
            Console.Error.WriteLine(ex.Message);
            CommandLineOptions.PrintUsage();
            return ExitUsage;
        }

        if (options.ShowHelp)
        {
            CommandLineOptions.PrintUsage();
            return ExitSuccess;
        }

        using var cts = new CancellationTokenSource();
        Console.CancelKeyPress += (_, e) =>
        {
            e.Cancel = true; // 既定の即時終了ではなく、後片付けできるよう協調的に止める
            Console.WriteLine();
            Console.WriteLine("中断しています...");

            try
            {
                // ReSharper disable once AccessToDisposedClosure
                cts.Cancel();
            }
            catch (ObjectDisposedException)
            {
                // すでに処理を終えて終了しかけている
            }
        };

        try
        {
            return await RunAsync(options, cts.Token);
        }
        catch (OperationCanceledException)
        {
            Console.WriteLine("中断しました。");
            return ExitCancelled;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine();
            Console.Error.WriteLine("Error: " + ex.Message);
            return ExitFailure;
        }
    }

    private static async Task<int> RunAsync(CommandLineOptions options, CancellationToken ct)
    {
        var url = options.Url ?? Prompt("URL");
        if (string.IsNullOrWhiteSpace(url))
        {
            Console.Error.WriteLine("URL is required.");
            return ExitUsage;
        }

        var folder = options.OutputFolder ?? Prompt("Save at");
        if (string.IsNullOrWhiteSpace(folder))
        {
            Console.Error.WriteLine("Save path is required.");
            return ExitUsage;
        }

        folder = ExpandPath(folder);
        Directory.CreateDirectory(folder);

        var audioOnly = options.AudioOnly ?? IsYes(Prompt("Audio only? [y/N]"));
        var audioFormat = audioOnly ? NormalizeAudioFormat(options.AudioFormat) : null;

        // --- ツールの準備 -------------------------------------------------
        var toolsDirectory = Path.Combine(AppContext.BaseDirectory, "tools");

        string ytdlpPath;
        try
        {
            ytdlpPath = await ToolProvisioner.EnsureYtdlpAsync(toolsDirectory, ct);
        }
        catch (OperationCanceledException)
        {
            throw;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"yt-dlp の準備に失敗しました: {ex.Message}");
            return ExitFailure;
        }

        var ffmpegDirectory = await TryEnsureAsync(
            "ffmpeg", token => ToolProvisioner.EnsureFfmpegAsync(toolsDirectory, token), ct);

        if (ffmpegDirectory is null)
        {
            if (audioOnly)
            {
                Console.Error.WriteLine("音声抽出には ffmpeg が必要です。");
                return ExitFailure;
            }

            Console.WriteLine("ffmpeg が無いため、映像と音声の結合・メタデータ埋め込みは行いません。");
        }

        if (!options.SkipDeno)
        {
            var denoDirectory = await TryEnsureAsync(
                "deno", token => ToolProvisioner.EnsureDenoAsync(toolsDirectory, token), ct);

            // yt-dlp は PATH から deno を探すので、tools ディレクトリを PATH に載せる。
            // 引数で渡すより確実で、パスに空白が含まれていても壊れない。
            if (denoDirectory is not null)
                Platform.PrependToPath(denoDirectory);
        }

        // --- メタデータ取得 -----------------------------------------------
        var ytdlp = new Ytdlp(ytdlpPath, new ConsoleLogger());

        var metadata = await ytdlp.GetMetadataAsync(url, ct);
        ct.ThrowIfCancellationRequested();

        if (metadata is null || string.IsNullOrWhiteSpace(metadata.Id))
        {
            Console.Error.WriteLine("Failed to get metadata (video id).");
            return ExitFailure;
        }

        if (!string.IsNullOrWhiteSpace(metadata.Title))
            Console.WriteLine($"Title: {metadata.Title}");

        if (!options.KeepExisting)
            DeleteExistingDownloads(folder, metadata.Id);

        // --- 進捗表示 -------------------------------------------------------
        var progress = new ConsoleProgressBar("");
        ytdlp.OnProgressDownload += (_, e) =>
            progress.Report(e.Percent, $"{e.Speed,-12} ETA {e.ETA,-10}");

        ytdlp.OnCompleteDownload += (_, message) =>
            progress.Complete($"Finished: {message}");

        ytdlp.OnPostProcessingComplete += (_, message) =>
            progress.Complete($"Post-processing: {message}");

        // --- ダウンロード ---------------------------------------------------
        ytdlp
            .SetFormat(SelectFormat(audioOnly, ffmpegDirectory is not null))
            .SetOutputFolder(folder)
            .SetOutputTemplate(audioOnly
                ? "%(upload_date)s_%(id)s.%(ext)s"
                : "%(upload_date)s_%(id)s [%(resolution)s].%(ext)s");

        if (ffmpegDirectory is not null)
        {
            ytdlp.SetFFmpegLocation(ffmpegDirectory)
                 .EmbedMetadata();

            if (audioOnly)
                ytdlp.ExtractAudio(audioFormat!);
            else
                ytdlp.EmbedThumbnail();
        }

        try
        {
            await ytdlp.ExecuteAsync(url, ct);
        }
        finally
        {
            progress.Complete();
        }

        return ExitSuccess;
    }

    /// <summary>無くても処理を続けられるツールを用意する。失敗したら <c>null</c>。</summary>
    private static async Task<string?> TryEnsureAsync(
        string name,
        Func<CancellationToken, Task<string?>> ensure,
        CancellationToken ct)
    {
        try
        {
            return await ensure(ct);
        }
        catch (OperationCanceledException)
        {
            throw;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"{name} の準備に失敗しました: {ex.Message}");
            return null;
        }
    }

    private static string SelectFormat(bool audioOnly, bool hasFfmpeg)
    {
        if (audioOnly)
            return "bestaudio/best";

        // ffmpeg が無いと bestvideo+bestaudio の結合ができないので、単一ファイルの形式に絞る
        return hasFfmpeg ? "bestvideo+bestaudio/best" : "best";
    }

    /// <summary>同じ動画 ID を含むファイル（前回のダウンロードや .part）を消す。</summary>
    private static void DeleteExistingDownloads(string folder, string id)
    {
        string[] targets;
        try
        {
            targets = Directory.GetFiles(folder, $"*{id}*");
        }
        catch (IOException)
        {
            return;
        }
        catch (UnauthorizedAccessException)
        {
            return;
        }

        var deleted = 0;
        foreach (var file in targets)
        {
            try
            {
                File.Delete(file);
                deleted++;
            }
            catch (IOException)
            {
                // 使用中などで消せないものがあっても続行
            }
            catch (UnauthorizedAccessException)
            {
            }
        }

        if (deleted > 0)
            Console.WriteLine($"既存ファイルを {deleted} 件削除しました。");
    }

    private static string Prompt(string label)
    {
        Console.Write($"{label}: ");
        return Console.ReadLine()?.Trim() ?? string.Empty;
    }

    /// <summary>先頭の <c>~</c> をホームディレクトリに展開し、絶対パスへ正規化する。</summary>
    private static string ExpandPath(string path)
    {
        path = path.Trim();

        // シェルを介さずに起動された場合、~ は展開されないままここに届く
        if (path == "~" || path.StartsWith("~/") || path.StartsWith(@"~\"))
        {
            var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
            if (!string.IsNullOrEmpty(home))
                path = path.Length == 1 ? home : Path.Combine(home, path[2..]);
        }

        // %VAR% は Windows の書式なので、Unix のパスに含まれる % を壊さないよう Windows 限定にする
        if (OperatingSystem.IsWindows())
            path = Environment.ExpandEnvironmentVariables(path);

        return Path.GetFullPath(path);
    }

    private static bool IsYes(string? input)
    {
        if (string.IsNullOrWhiteSpace(input))
            return false;

        var value = input.Trim();
        return value is "はい" or "1"
            || value.Equals("y", StringComparison.OrdinalIgnoreCase)
            || value.Equals("yes", StringComparison.OrdinalIgnoreCase)
            || value.Equals("true", StringComparison.OrdinalIgnoreCase);
    }

    /// <summary>音声フォーマットを検証する。未指定なら対話で聞き、そこでも空なら wav。</summary>
    private static string NormalizeAudioFormat(string? requested)
    {
        if (requested is not null)
        {
            if (TryParseAudioFormat(requested, out var format))
                return format;

            WarnUnsupportedAudioFormat();
        }

        while (true)
        {
            var input = Prompt("Audio extension [wav]");
            if (string.IsNullOrWhiteSpace(input))
                return "wav";

            if (TryParseAudioFormat(input, out var format))
                return format;

            WarnUnsupportedAudioFormat();
        }
    }

    private static bool TryParseAudioFormat(string input, out string format)
    {
        format = input.Trim().TrimStart('.').ToLowerInvariant();
        return SupportedAudioFormats.Contains(format);
    }

    private static void WarnUnsupportedAudioFormat() =>
        Console.WriteLine("Unsupported audio extension. Use: " + string.Join(", ", SupportedAudioFormats));

    private static void TrySetUtf8Output()
    {
        try
        {
            Console.OutputEncoding = Encoding.UTF8;
        }
        catch (IOException)
        {
            // 出力がリダイレクトされている等でエンコーディングを変更できない場合は既定のまま
        }
    }
}
