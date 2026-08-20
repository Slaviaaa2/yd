using System.Runtime.InteropServices;

namespace YtdlpCLI;

/// <summary>
/// OS / アーキテクチャの判定と、実行ファイルの探索・権限付与をまとめたヘルパー。
/// <see cref="Environment.OSVersion"/> は Linux と macOS を区別できない（どちらも
/// <c>PlatformID.Unix</c>）ため、判定はすべて <see cref="OperatingSystem"/> 経由で行う。
/// </summary>
internal static class Platform
{
    public static bool IsWindows => OperatingSystem.IsWindows();
    public static bool IsLinux => OperatingSystem.IsLinux();
    public static bool IsMacOS => OperatingSystem.IsMacOS();

    public static Architecture Architecture => RuntimeInformation.OSArchitecture;

    /// <summary>glibc ではなく musl libc の Linux か（Alpine など）。</summary>
    public static bool IsMuslLinux { get; } = DetectMusl();

    /// <summary>実行時 OS 向けの実行ファイル名（Windows なら <c>.exe</c> を付ける）。</summary>
    public static string ExecutableName(string baseName) => IsWindows ? baseName + ".exe" : baseName;

    /// <summary>人間向けの環境説明（"linux-x64" など）。</summary>
    public static string Describe()
    {
        var os = IsWindows ? "windows" : IsMacOS ? "macos" : IsMuslLinux ? "linux(musl)" : IsLinux ? "linux" : "unknown";
        return $"{os}-{Architecture.ToString().ToLowerInvariant()}";
    }

    /// <summary>PATH から実行ファイルを探す。見つからなければ <c>null</c>。</summary>
    public static string? FindOnPath(string baseName)
    {
        var candidates = IsWindows ? WindowsCandidates(baseName) : [baseName];

        foreach (var directory in EnumeratePathDirectories())
        {
            foreach (var candidate in candidates)
            {
                string full;
                try
                {
                    full = Path.Combine(directory, candidate);
                }
                catch (ArgumentException)
                {
                    continue; // PATH に不正な文字が含まれるエントリ
                }

                if (IsExecutableFile(full))
                    return full;
            }
        }

        return null;
    }

    /// <summary>
    /// このプロセスの PATH の先頭にディレクトリを追加する。
    /// 子プロセス（yt-dlp）は親の環境変数を引き継ぐので、これで yt-dlp から
    /// tools ディレクトリの deno / ffmpeg が見えるようになる。
    /// </summary>
    public static void PrependToPath(string directory)
    {
        var current = Environment.GetEnvironmentVariable("PATH") ?? string.Empty;

        var alreadyPresent = current
            .Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .Any(entry => string.Equals(entry.Trim('"'), directory, StringComparison.Ordinal));

        if (alreadyPresent)
            return;

        Environment.SetEnvironmentVariable(
            "PATH",
            current.Length == 0 ? directory : directory + Path.PathSeparator + current);
    }

    /// <summary>Unix でのみ実行権限 (0755) を付与する。Windows では何もしない。</summary>
    public static void MakeExecutable(string path)
    {
        if (OperatingSystem.IsWindows())
            return;

        const UnixFileMode mode =
            UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute |
            UnixFileMode.GroupRead | UnixFileMode.GroupExecute |
            UnixFileMode.OtherRead | UnixFileMode.OtherExecute;

        File.SetUnixFileMode(path, mode);
    }

    private static bool DetectMusl()
    {
        if (!IsLinux)
            return false;

        if (RuntimeInformation.RuntimeIdentifier.Contains("musl", StringComparison.OrdinalIgnoreCase))
            return true;

        try
        {
            // Alpine などでは /lib/ld-musl-<arch>.so.1 がローダーとして置かれている
            return Directory.Exists("/lib") && Directory.EnumerateFiles("/lib", "ld-musl-*.so.1").Any();
        }
        catch (IOException)
        {
            return false;
        }
        catch (UnauthorizedAccessException)
        {
            return false;
        }
    }

    private static bool IsExecutableFile(string path)
    {
        if (!File.Exists(path))
            return false;

        if (OperatingSystem.IsWindows())
            return true;

        try
        {
            return (File.GetUnixFileMode(path) &
                    (UnixFileMode.UserExecute | UnixFileMode.GroupExecute | UnixFileMode.OtherExecute)) != 0;
        }
        catch (IOException)
        {
            return false;
        }
        catch (UnauthorizedAccessException)
        {
            return false;
        }
    }

    private static IEnumerable<string> EnumeratePathDirectories()
    {
        var path = Environment.GetEnvironmentVariable("PATH");
        if (string.IsNullOrEmpty(path))
            yield break;

        foreach (var entry in path.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
            yield return entry.Trim('"');
    }

    private static string[] WindowsCandidates(string baseName)
    {
        var pathExt = Environment.GetEnvironmentVariable("PATHEXT") ?? ".COM;.EXE;.BAT;.CMD";
        var extensions = pathExt.Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
        return [baseName, .. extensions.Select(extension => baseName + extension)];
    }
}
