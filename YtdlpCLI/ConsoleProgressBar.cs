using System.Diagnostics;

namespace YtdlpCLI;

/// <summary>
/// 1 行を <c>\r</c> で上書きし続ける進捗バー。
/// 出力がリダイレクトされている場合（パイプ・ログ）はバーを描かず、
/// 節目のみを普通の行として出力する。
/// </summary>
internal sealed class ConsoleProgressBar
{
    private const int BarWidth = 30;

    /// <summary>端末への再描画の最小間隔。チャンクごとに描くとちらつくため間引く。</summary>
    private static readonly TimeSpan RedrawInterval = TimeSpan.FromMilliseconds(80);

    private readonly string _label;
    private readonly bool _interactive = !Console.IsOutputRedirected;
    private readonly Stopwatch _sinceLastRedraw = Stopwatch.StartNew();
    private int _lastRenderedLength;
    private int _lastLoggedStep = -1;
    private bool _dirty;

    public ConsoleProgressBar(string label) => _label = label;

    /// <summary>進捗を描画する。<paramref name="percent"/> は 0-100。</summary>
    public void Report(double percent, string? detail = null)
    {
        percent = Math.Clamp(double.IsFinite(percent) ? percent : 0, 0, 100);

        if (!_interactive)
        {
            // 10% 刻みでのみ 1 行出力する（ログが進捗で埋まらないように）
            var step = (int)(percent / 10);
            if (step <= _lastLoggedStep)
                return;

            _lastLoggedStep = step;
            Console.WriteLine(Join(_label, $"{percent,5:F1}%", detail));
            return;
        }

        // 100% だけは必ず描いて、バーが途中で止まったまま終わらないようにする
        if (percent < 100 && _sinceLastRedraw.Elapsed < RedrawInterval)
            return;

        _sinceLastRedraw.Restart();

        var filled = (int)Math.Round(percent / 100 * BarWidth);
        var bar = "[" + new string('=', filled) + new string(' ', BarWidth - filled) + "]";
        var line = Join(_label, bar, $"{percent,5:F1}%", detail);

        // 前回より短くなったときに残骸が残らないよう空白で埋める
        var padded = line.PadRight(_lastRenderedLength);
        _lastRenderedLength = line.Length;
        _dirty = true;

        Console.Write('\r');
        Console.Write(padded);
    }

    /// <summary>バーの行を閉じる。何も描いていなければ改行しない。</summary>
    public void Complete(string? finalMessage = null)
    {
        if (_dirty)
        {
            Console.WriteLine();
            _dirty = false;
            _lastRenderedLength = 0;
        }

        if (!string.IsNullOrEmpty(finalMessage))
            Console.WriteLine(finalMessage);
    }

    private static string Join(params string?[] parts) =>
        string.Join(' ', parts.Where(part => !string.IsNullOrEmpty(part)));
}
