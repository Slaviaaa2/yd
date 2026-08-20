# YtdlpCLI

yt-dlp のかんたんな CLI ラッパー。Windows / Linux / macOS で動きます。

## 特徴

**yt-dlp・ffmpeg・deno は自動で用意されます。** 手動での配置は不要です。
起動時に次の順で探し、見つからなければ実行環境に合ったビルドをダウンロードして `tools/` に置きます。

1. `tools/` ディレクトリ
2. PATH 上の既存インストール（`apt install ffmpeg` などで入れたもの）
3. 自動ダウンロード

| ツール | 用途 | 取得元 |
| --- | --- | --- |
| yt-dlp | ダウンロード本体 | [yt-dlp/yt-dlp](https://github.com/yt-dlp/yt-dlp) |
| ffmpeg / ffprobe | 映像と音声の結合、音声抽出、メタデータ埋め込み | [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds)（macOS は [evermeet.cx](https://evermeet.cx/ffmpeg/)） |
| deno | yt-dlp が YouTube の JavaScript チャレンジを解くのに使用 | [denoland/deno](https://github.com/denoland/deno) |

初回のみ合計 400MB 程度のダウンロードが発生します。2 回目以降は `tools/` のものを使います。

## 使い方

```
ytdlp-cli [オプション] [URL]

  -o, --output <DIR>      保存先ディレクトリ
  -a, --audio [FORMAT]    音声のみ (既定: wav)
  -v, --video             動画としてダウンロード
  -k, --keep              同じ動画 ID の既存ファイルを削除しない
      --no-deno           JS ランタイム (deno) を用意しない
  -h, --help              ヘルプを表示
```

省略した項目は対話プロンプトで入力します。引数なしで起動すれば従来どおり全て対話で進みます。

```sh
# 対話モード
ytdlp-cli

# 動画をダウンロード
ytdlp-cli -o ~/Videos "https://www.youtube.com/watch?v=..."

# mp3 で音声のみ
ytdlp-cli -o ~/Music -a mp3 "https://www.youtube.com/watch?v=..."
```

対応する音声フォーマット: `aac` `alac` `flac` `m4a` `mp3` `opus` `vorbis` `wav`

出力ファイル名は動画なら `%(upload_date)s_%(id)s [%(resolution)s].%(ext)s`、
音声なら `%(upload_date)s_%(id)s.%(ext)s` になります。
同じ動画 ID のファイルが保存先にあると既定で削除してから取得し直します（`-k` で抑止）。

## 対応環境

| OS | アーキテクチャ | 備考 |
| --- | --- | --- |
| Windows | x64 / arm64 | |
| Linux (glibc) | x64 / arm64 | |
| Linux (glibc) | armv7 | ffmpeg・deno の公式ビルドが無いため別途インストールが必要 |
| Linux (musl / Alpine) | x64 / arm64 | 同上（`apk add ffmpeg`） |
| macOS | x64 / arm64 | ffmpeg は x86_64 ビルド（Apple Silicon では Rosetta 2 経由） |

ffmpeg が用意できない場合も、単一ファイルで済む形式（`best`）でのダウンロードは続行します。

## ビルド

```sh
dotnet build
dotnet run --project YtdlpCLI -- --help
```

.NET 10 SDK が必要です。
