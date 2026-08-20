using System.IO.Compression;
using SharpCompress.Readers;

namespace YtdlpCLI;

internal enum ArchiveKind
{
    Zip,
    TarXz
}

/// <summary>HTTP ダウンロードと、取得したアーカイブからの実行ファイル抽出。</summary>
internal static class Downloader
{
    private static readonly HttpClient Http = CreateHttpClient();

    /// <summary>進捗を表示しながら <paramref name="url"/> を <paramref name="destinationPath"/> に保存する。</summary>
    public static async Task DownloadFileAsync(string url, string destinationPath, string label, CancellationToken ct)
    {
        using var response = await Http.GetAsync(url, HttpCompletionOption.ResponseHeadersRead, ct);
        response.EnsureSuccessStatusCode();

        var totalBytes = response.Content.Headers.ContentLength;
        var progress = new ConsoleProgressBar(label);
        var downloaded = 0L;

        await using (var source = await response.Content.ReadAsStreamAsync(ct))
        await using (var destination = new FileStream(
                         destinationPath,
                         FileMode.Create,
                         FileAccess.Write,
                         FileShare.None,
                         bufferSize: 81920,
                         useAsync: true))
        {
            var buffer = new byte[81920];
            int read;
            while ((read = await source.ReadAsync(buffer, ct)) > 0)
            {
                await destination.WriteAsync(buffer.AsMemory(0, read), ct);
                downloaded += read;

                if (totalBytes is > 0)
                    progress.Report((double)downloaded / totalBytes.Value * 100, FormatBytes(downloaded, totalBytes.Value));
                else
                    progress.Report(0, FormatBytes(downloaded, null));
            }
        }

        progress.Complete();
    }

    /// <summary>
    /// アーカイブから <paramref name="wantedFileNames"/> と（ファイル名が）一致するエントリだけを
    /// <paramref name="destinationDirectory"/> 直下に取り出す。取り出せたファイル名を返す。
    /// </summary>
    public static IReadOnlyCollection<string> ExtractFiles(
        string archivePath,
        ArchiveKind kind,
        IReadOnlyCollection<string> wantedFileNames,
        string destinationDirectory,
        CancellationToken ct)
    {
        Directory.CreateDirectory(destinationDirectory);

        var remaining = new HashSet<string>(wantedFileNames, StringComparer.OrdinalIgnoreCase);
        var extracted = new List<string>();

        void Take(string entryName, Action<Stream> copyTo)
        {
            var fileName = entryName.Replace('\\', '/').Split('/')[^1];
            if (!remaining.Remove(fileName))
                return;

            var target = Path.Combine(destinationDirectory, fileName);
            using (var file = new FileStream(target, FileMode.Create, FileAccess.Write, FileShare.None))
            {
                copyTo(file);
            }

            Platform.MakeExecutable(target);
            extracted.Add(fileName);
        }

        switch (kind)
        {
            case ArchiveKind.Zip:
            {
                using var zip = ZipFile.OpenRead(archivePath);
                foreach (var entry in zip.Entries)
                {
                    ct.ThrowIfCancellationRequested();
                    if (entry.Length == 0 && entry.FullName.EndsWith('/'))
                        continue;

                    Take(entry.FullName, destination =>
                    {
                        using var source = entry.Open();
                        source.CopyTo(destination);
                    });

                    if (remaining.Count == 0)
                        break;
                }

                break;
            }

            case ArchiveKind.TarXz:
            {
                // ReaderFactory は xz 圧縮を検出して tar として読んでくれる。
                // XZStream を TarReader に直接渡すと部分読み込みで EOF 扱いになるため使わない。
                using var file = File.OpenRead(archivePath);
                using var reader = ReaderFactory.Open(file);
                while (reader.MoveToNextEntry())
                {
                    ct.ThrowIfCancellationRequested();
                    if (reader.Entry.IsDirectory || reader.Entry.Key is not { } key)
                        continue;

                    Take(key, destination => reader.WriteEntryTo(destination));

                    if (remaining.Count == 0)
                        break;
                }

                break;
            }

            default:
                throw new ArgumentOutOfRangeException(nameof(kind), kind, null);
        }

        return extracted;
    }

    /// <summary>
    /// zip の中身をすべて destinationDirectory 以下に展開する。
    /// PyInstaller の onedir 形式のように、実行ファイルが隣接するファイル群を必要とする配布物向け。
    /// </summary>
    public static void ExtractAll(string archivePath, string destinationDirectory, CancellationToken ct)
    {
        Directory.CreateDirectory(destinationDirectory);
        var root = Path.GetFullPath(destinationDirectory);

        using var zip = ZipFile.OpenRead(archivePath);
        foreach (var entry in zip.Entries)
        {
            ct.ThrowIfCancellationRequested();

            var target = Path.GetFullPath(Path.Combine(root, entry.FullName));

            // zip slip 対策: 展開先の外に出るエントリは無視する
            if (!target.StartsWith(root + Path.DirectorySeparatorChar, StringComparison.Ordinal))
                continue;

            if (entry.Name.Length == 0)
            {
                Directory.CreateDirectory(target);
                continue;
            }

            Directory.CreateDirectory(Path.GetDirectoryName(target)!);
            entry.ExtractToFile(target, overwrite: true);
        }
    }

    private static string FormatBytes(long downloaded, long? total) =>
        total is { } t
            ? $"{downloaded / 1024.0 / 1024.0,7:F1} / {t / 1024.0 / 1024.0,-7:F1} MiB"
            : $"{downloaded / 1024.0 / 1024.0,7:F1} MiB";

    private static HttpClient CreateHttpClient()
    {
        var handler = new SocketsHttpHandler
        {
            ConnectTimeout = TimeSpan.FromSeconds(30),
            // GitHub のリリースは別ホストへリダイレクトされる
            AllowAutoRedirect = true,
            MaxAutomaticRedirections = 10
        };

        var client = new HttpClient(handler)
        {
            // ffmpeg は 100MB を超えるため、全体の制限時間ではなく
            // CancellationToken (Ctrl+C) で中断できるようにする
            Timeout = Timeout.InfiniteTimeSpan
        };
        client.DefaultRequestHeaders.UserAgent.ParseAdd("YtdlpCLI/1.1");
        return client;
    }
}
