using System.Diagnostics;
using System.Globalization;
using System.Runtime.InteropServices;

namespace YtdlpCLI;

/// <summary>
/// yt-dlp / ffmpeg を用意する。優先順位は
/// (1) アプリの <c>tools</c> ディレクトリ → (2) PATH 上の既存インストール → (3) 自動ダウンロード。
/// </summary>
internal static class ToolProvisioner
{
    private const string BtbNBaseUrl = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/";
    private const string YtdlpBaseUrl = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/";
    private const string DenoBaseUrl = "https://github.com/denoland/deno/releases/latest/download/";

    private const string BundleDirectoryName = "yt-dlp-bundle";

    /// <summary>
    /// PATH 上の yt-dlp を使ってよい古さの上限。ディストリのパッケージは数年前で止まっていることが多く、
    /// 古い yt-dlp は YouTube 側の仕様変更で動かなくなるため、期限切れなら自前で新しいものを取得する。
    /// </summary>
    private static readonly TimeSpan MaxYtdlpAge = TimeSpan.FromDays(90);

    /// <summary>アーカイブを 1 つ落として、必要な実行ファイルだけ取り出すための指定。</summary>
    private sealed record RemoteArchive(string Url, ArchiveKind Kind, string[] Files);

    /// <summary>
    /// yt-dlp のリリースアセット。<paramref name="BundleExecutableName"/> が指定されている場合は
    /// zip の onedir 配布（実行ファイル + <c>_internal/</c>）で、丸ごと展開する必要がある。
    /// </summary>
    private sealed record YtdlpAsset(string AssetName, string? BundleExecutableName = null)
    {
        public bool IsBundle => BundleExecutableName is not null;

        public string ExecutableName => BundleExecutableName ?? AssetName;
    }

    public static async Task<string> EnsureYtdlpAsync(string toolsDirectory, CancellationToken ct)
    {
        var asset = ResolveYtdlpAsset();
        var localPath = asset.IsBundle
            ? Path.Combine(toolsDirectory, BundleDirectoryName, asset.ExecutableName)
            : Path.Combine(toolsDirectory, Platform.ExecutableName("yt-dlp"));

        if (File.Exists(localPath))
        {
            Platform.MakeExecutable(localPath);
            return localPath;
        }

        if (Platform.FindOnPath("yt-dlp") is { } onPath)
        {
            var version = await TryReadYtdlpVersionAsync(onPath, ct);
            if (version is null)
            {
                Console.WriteLine($"yt-dlp: {onPath} はバージョンを確認できないため使用しません。");
            }
            else if (DateTime.UtcNow - version.Value.Released > MaxYtdlpAge)
            {
                Console.WriteLine(
                    $"yt-dlp: {onPath} は {version.Value.Text} と古いため使用しません（新しい版を取得します）。");
            }
            else
            {
                Console.WriteLine($"yt-dlp: システムにインストール済みのものを使用します ({onPath}, {version.Value.Text})");
                return onPath;
            }
        }

        Console.WriteLine($"yt-dlp が見つかりません。{asset.AssetName} をダウンロードします ({Platform.Describe()})...");
        Directory.CreateDirectory(toolsDirectory);

        if (asset.IsBundle)
        {
            // 単体バイナリではなく、実行ファイルと _internal/ が対になった配布物。
            // ディレクトリ構成を保ったまま丸ごと展開する必要がある。
            await DownloadBundleAsync(asset, Path.Combine(toolsDirectory, BundleDirectoryName), ct);
        }
        else
        {
            await DownloadBinaryAsync(YtdlpBaseUrl + asset.AssetName, localPath, "yt-dlp", ct);
        }

        if (!File.Exists(localPath))
            throw new InvalidOperationException($"yt-dlp を {localPath} に展開できませんでした。");

        Platform.MakeExecutable(localPath);
        Console.WriteLine("yt-dlp の準備が完了しました。");
        return localPath;
    }

    /// <summary>
    /// ffmpeg を用意し、その実行ファイルが入っているディレクトリ（yt-dlp の
    /// <c>--ffmpeg-location</c> に渡す値）を返す。用意できなければ <c>null</c>。
    /// </summary>
    public static async Task<string?> EnsureFfmpegAsync(string toolsDirectory, CancellationToken ct)
    {
        var localName = Platform.ExecutableName("ffmpeg");
        var localPath = Path.Combine(toolsDirectory, localName);

        if (File.Exists(localPath))
        {
            // 手で置かれた場合に実行権限が無いことがあるので付け直しておく
            Platform.MakeExecutable(localPath);

            var ffprobePath = Path.Combine(toolsDirectory, Platform.ExecutableName("ffprobe"));
            if (File.Exists(ffprobePath))
                Platform.MakeExecutable(ffprobePath);

            return toolsDirectory;
        }

        if (Platform.FindOnPath("ffmpeg") is { } onPath &&
            Path.GetDirectoryName(Path.GetFullPath(onPath)) is { } onPathDirectory)
        {
            Console.WriteLine($"ffmpeg: システムにインストール済みのものを使用します ({onPath})");
            return onPathDirectory;
        }

        var sources = ResolveFfmpegSources();
        if (sources.Count == 0)
        {
            Console.WriteLine($"ffmpeg の自動ダウンロードに対応していない環境です ({Platform.Describe()})。");
            Console.WriteLine("  " + ManualInstallHint());
            return null;
        }

        Console.WriteLine($"ffmpeg が見つかりません。ビルド済みバイナリをダウンロードします ({Platform.Describe()})...");
        Directory.CreateDirectory(toolsDirectory);

        foreach (var source in sources)
            await DownloadArchiveAsync(source, toolsDirectory, "ffmpeg", ct);

        if (!File.Exists(localPath))
        {
            Console.WriteLine("ダウンロードしたアーカイブに ffmpeg が含まれていませんでした。");
            Console.WriteLine("  " + ManualInstallHint());
            return null;
        }

        Console.WriteLine("ffmpeg の準備が完了しました。");
        return toolsDirectory;
    }

    /// <summary>
    /// deno を用意し、その実行ファイルが入っているディレクトリを返す。用意できなければ <c>null</c>。
    /// yt-dlp は 2025 年後半以降、YouTube の JavaScript チャレンジを解くのに
    /// 外部 JS ランタイム（既定では deno）を必要とする。
    /// </summary>
    public static async Task<string?> EnsureDenoAsync(string toolsDirectory, CancellationToken ct)
    {
        var localPath = Path.Combine(toolsDirectory, Platform.ExecutableName("deno"));

        if (File.Exists(localPath))
        {
            Platform.MakeExecutable(localPath);
            return toolsDirectory;
        }

        if (Platform.FindOnPath("deno") is { } onPath &&
            Path.GetDirectoryName(Path.GetFullPath(onPath)) is { } onPathDirectory)
        {
            Console.WriteLine($"deno: システムにインストール済みのものを使用します ({onPath})");
            return onPathDirectory;
        }

        var assetName = ResolveDenoAssetName();
        if (assetName is null)
        {
            Console.WriteLine($"deno の自動ダウンロードに対応していない環境です ({Platform.Describe()})。");
            Console.WriteLine("  YouTube の一部フォーマットが取得できない場合があります: https://github.com/yt-dlp/yt-dlp/wiki/EJS");
            return null;
        }

        Console.WriteLine("deno (yt-dlp が YouTube で使う JS ランタイム) が見つかりません。ダウンロードします...");
        Directory.CreateDirectory(toolsDirectory);

        await DownloadArchiveAsync(
            new RemoteArchive(DenoBaseUrl + assetName, ArchiveKind.Zip, [Platform.ExecutableName("deno")]),
            toolsDirectory,
            "deno",
            ct);

        if (!File.Exists(localPath))
        {
            Console.WriteLine("deno を取得できませんでした。YouTube の一部フォーマットが取得できない場合があります。");
            return null;
        }

        Console.WriteLine("deno の準備が完了しました。");
        return toolsDirectory;
    }

    /// <summary>
    /// <c>yt-dlp --version</c> を実行して版を読む。yt-dlp の版は <c>YYYY.MM.DD</c> 形式なので
    /// そのままリリース日として扱える。読めなければ <c>null</c>。
    /// </summary>
    private static async Task<(string Text, DateTime Released)?> TryReadYtdlpVersionAsync(string path, CancellationToken ct)
    {
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
        timeout.CancelAfter(TimeSpan.FromSeconds(15));

        string output;
        try
        {
            using var process = Process.Start(new ProcessStartInfo(path, "--version")
            {
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true
            });

            if (process is null)
                return null;

            output = await process.StandardOutput.ReadToEndAsync(timeout.Token);
            await process.WaitForExitAsync(timeout.Token);

            if (process.ExitCode != 0)
                return null;
        }
        catch (OperationCanceledException) when (!ct.IsCancellationRequested)
        {
            return null; // 応答しないバイナリ
        }
        catch (Exception ex) when (ex is InvalidOperationException or IOException or UnauthorizedAccessException)
        {
            return null;
        }

        var text = output.Trim();

        // 開発版は 2026.08.19.123456 のように続きが付くので先頭 3 要素だけ見る
        var parts = text.Split('.');
        if (parts.Length < 3)
            return null;

        return DateTime.TryParseExact(
            string.Join('.', parts[..3]),
            "yyyy.MM.dd",
            CultureInfo.InvariantCulture,
            DateTimeStyles.AssumeUniversal | DateTimeStyles.AdjustToUniversal,
            out var released)
            ? (text, released)
            : null;
    }

    private static async Task DownloadBinaryAsync(string url, string destinationPath, string label, CancellationToken ct)
    {
        var temporaryPath = destinationPath + $".{Guid.NewGuid():N}.download";
        try
        {
            await Downloader.DownloadFileAsync(url, temporaryPath, $"  {label}", ct);
            File.Move(temporaryPath, destinationPath, overwrite: true);
        }
        finally
        {
            TryDelete(temporaryPath);
        }
    }

    private static async Task DownloadBundleAsync(YtdlpAsset asset, string bundleDirectory, CancellationToken ct)
    {
        var temporaryPath = bundleDirectory + $".{Guid.NewGuid():N}.download";
        try
        {
            await Downloader.DownloadFileAsync(YtdlpBaseUrl + asset.AssetName, temporaryPath, "  yt-dlp", ct);
            Downloader.ExtractAll(temporaryPath, bundleDirectory, ct);
        }
        finally
        {
            TryDelete(temporaryPath);
        }
    }

    private static async Task DownloadArchiveAsync(RemoteArchive archive, string toolsDirectory, string label, CancellationToken ct)
    {
        var temporaryPath = Path.Combine(toolsDirectory, $"{label}.{Guid.NewGuid():N}.download");
        try
        {
            await Downloader.DownloadFileAsync(archive.Url, temporaryPath, $"  {label}", ct);

            var extracted = Downloader.ExtractFiles(temporaryPath, archive.Kind, archive.Files, toolsDirectory, ct);
            var missing = archive.Files.Except(extracted, StringComparer.OrdinalIgnoreCase).ToArray();
            if (missing.Length > 0)
                Console.WriteLine($"  警告: アーカイブ内に {string.Join(", ", missing)} が見つかりませんでした。");
        }
        finally
        {
            TryDelete(temporaryPath);
        }
    }

    private static YtdlpAsset ResolveYtdlpAsset()
    {
        if (Platform.IsWindows)
        {
            return Platform.Architecture switch
            {
                Architecture.X64 => new YtdlpAsset("yt-dlp.exe"),
                Architecture.Arm64 => new YtdlpAsset("yt-dlp_arm64.exe"),
                Architecture.X86 => new YtdlpAsset("yt-dlp_x86.exe"),
                _ => throw UnsupportedYtdlp()
            };
        }

        if (Platform.IsMacOS)
            return new YtdlpAsset("yt-dlp_macos");

        if (Platform.IsLinux)
        {
            return (Platform.Architecture, Platform.IsMuslLinux) switch
            {
                (Architecture.X64, false) => new YtdlpAsset("yt-dlp_linux"),
                (Architecture.Arm64, false) => new YtdlpAsset("yt-dlp_linux_aarch64"),
                (Architecture.X64, true) => new YtdlpAsset("yt-dlp_musllinux"),
                (Architecture.Arm64, true) => new YtdlpAsset("yt-dlp_musllinux_aarch64"),
                // 32bit ARM は zip の onedir 配布のみ（Raspberry Pi OS 32bit など）
                (Architecture.Arm, false) => new YtdlpAsset("yt-dlp_linux_armv7l.zip", "yt-dlp_linux_armv7l"),
                _ => throw UnsupportedYtdlp()
            };
        }

        throw UnsupportedYtdlp();
    }

    private static PlatformNotSupportedException UnsupportedYtdlp() =>
        new($"yt-dlp: 未対応の環境です ({Platform.Describe()})");

    /// <summary>この環境向けの deno アセット名。ビルドが提供されていなければ <c>null</c>。</summary>
    private static string? ResolveDenoAssetName()
    {
        // deno は musl / 32bit ARM 向けの公式ビルドを配布していない
        if (Platform.IsMuslLinux)
            return null;

        var target = Platform.Architecture switch
        {
            Architecture.X64 => "x86_64",
            Architecture.Arm64 => "aarch64",
            _ => null
        };

        if (target is null)
            return null;

        if (Platform.IsWindows)
            return $"deno-{target}-pc-windows-msvc.zip";

        if (Platform.IsMacOS)
            return $"deno-{target}-apple-darwin.zip";

        if (Platform.IsLinux)
            return $"deno-{target}-unknown-linux-gnu.zip";

        return null;
    }

    private static IReadOnlyList<RemoteArchive> ResolveFfmpegSources()
    {
        string[] windowsFiles = ["ffmpeg.exe", "ffprobe.exe"];
        string[] unixFiles = ["ffmpeg", "ffprobe"];

        if (Platform.IsWindows)
        {
            var slug = Platform.Architecture switch
            {
                Architecture.X64 => "win64",
                Architecture.Arm64 => "winarm64",
                _ => null
            };

            return slug is null
                ? []
                : [new RemoteArchive($"{BtbNBaseUrl}ffmpeg-master-latest-{slug}-gpl.zip", ArchiveKind.Zip, windowsFiles)];
        }

        if (Platform.IsMacOS)
        {
            // evermeet.cx は ffmpeg / ffprobe を別々の zip で配布している（x86_64 ビルド、
            // Apple Silicon では Rosetta 2 経由で動作する）。
            return
            [
                new RemoteArchive("https://evermeet.cx/ffmpeg/getrelease/ffmpeg/zip", ArchiveKind.Zip, ["ffmpeg"]),
                new RemoteArchive("https://evermeet.cx/ffmpeg/getrelease/ffprobe/zip", ArchiveKind.Zip, ["ffprobe"])
            ];
        }

        if (Platform.IsLinux && !Platform.IsMuslLinux)
        {
            // BtbN のビルドは glibc 向けなので musl (Alpine) では使えない。
            var slug = Platform.Architecture switch
            {
                Architecture.X64 => "linux64",
                Architecture.Arm64 => "linuxarm64",
                _ => null
            };

            return slug is null
                ? []
                : [new RemoteArchive($"{BtbNBaseUrl}ffmpeg-master-latest-{slug}-gpl.tar.xz", ArchiveKind.TarXz, unixFiles)];
        }

        return [];
    }

    private static string ManualInstallHint()
    {
        if (Platform.IsMacOS)
            return "手動でインストールしてください: brew install ffmpeg";

        if (Platform.IsMuslLinux)
            return "手動でインストールしてください: apk add ffmpeg";

        if (Platform.IsLinux)
            return "手動でインストールしてください: sudo apt install ffmpeg / sudo dnf install ffmpeg";

        return "ffmpeg を手動でインストールし、PATH に追加するか tools ディレクトリに配置してください。";
    }

    private static void TryDelete(string path)
    {
        try
        {
            if (File.Exists(path))
                File.Delete(path);
        }
        catch (IOException)
        {
            // 残っても致命的ではないので無視する
        }
        catch (UnauthorizedAccessException)
        {
        }
    }
}
