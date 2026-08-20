namespace YtdlpCLI;

/// <summary>コマンドライン引数。指定されなかった項目は対話プロンプトで補う。</summary>
internal sealed class CommandLineOptions
{
    public string? Url { get; private set; }
    public string? OutputFolder { get; private set; }
    public bool? AudioOnly { get; private set; }
    public string? AudioFormat { get; private set; }

    /// <summary>同じ動画 ID の既存ファイルを削除しない。</summary>
    public bool KeepExisting { get; private set; }

    /// <summary>deno (JS ランタイム) を用意しない。</summary>
    public bool SkipDeno { get; private set; }

    /// <summary>ヘルプを表示して終了する。</summary>
    public bool ShowHelp { get; private set; }

    public static CommandLineOptions Parse(string[] args)
    {
        var options = new CommandLineOptions();

        for (var i = 0; i < args.Length; i++)
        {
            var arg = args[i];
            switch (arg)
            {
                case "-h" or "--help":
                    options.ShowHelp = true;
                    return options;

                case "-o" or "--output":
                    options.OutputFolder = RequireValue(args, ref i, arg);
                    break;

                case "-a" or "--audio":
                    options.AudioOnly = true;
                    options.AudioFormat = TryTakeValue(args, ref i);
                    break;

                case "-v" or "--video":
                    options.AudioOnly = false;
                    break;

                case "-k" or "--keep":
                    options.KeepExisting = true;
                    break;

                case "--no-deno":
                    options.SkipDeno = true;
                    break;

                default:
                    if (arg.StartsWith('-'))
                        throw new ArgumentException($"不明なオプション: {arg}");

                    if (options.Url is not null)
                        throw new ArgumentException($"URL が複数指定されています: {arg}");

                    options.Url = arg;
                    break;
            }
        }

        return options;
    }

    public static void PrintUsage()
    {
        Console.WriteLine("""
            使い方: ytdlp-cli [オプション] [URL]

              -o, --output <DIR>      保存先ディレクトリ
              -a, --audio [FORMAT]    音声のみ (既定: wav)
              -v, --video             動画としてダウンロード
              -k, --keep              同じ動画 ID の既存ファイルを削除しない
                  --no-deno           JS ランタイム (deno) を用意しない
              -h, --help              このヘルプを表示

            省略した項目は対話プロンプトで入力します。
            yt-dlp / ffmpeg / deno は PATH 上のものを使い、無ければ tools/ に自動ダウンロードします。
            deno は yt-dlp が YouTube の JavaScript チャレンジを解くのに必要です
            (約 40MB / 初回のみ)。YouTube を使わないなら --no-deno で省略できます。
            """);
    }

    private static string RequireValue(string[] args, ref int index, string optionName)
    {
        if (index + 1 >= args.Length)
            throw new ArgumentException($"{optionName} には値が必要です。");

        return args[++index];
    }

    /// <summary>次の引数がオプションでなければ値として取り込む（省略可能な値用）。</summary>
    private static string? TryTakeValue(string[] args, ref int index)
    {
        if (index + 1 >= args.Length || args[index + 1].StartsWith('-'))
            return null;

        return args[++index];
    }
}
