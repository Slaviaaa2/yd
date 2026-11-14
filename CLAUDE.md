# yd — fast yt-dlp front-end

URL を貼るだけで最短でダウンロードできる CLI / インライン TUI。旧 C# 版 (master ブランチ) を Rust で全面書き直し中 (branch `rust-rewrite`)。

## 技術と構成
- Rust 2024 / clap / ratatui (Viewport::Inline + insert_before) + crossterm / serde + toml / ureq
- yt-dlp はライブラリを使わずプロセス直呼び。進捗は `--progress-template` の JSON を行単位でパース
- `src/tools/` 外部ツール (yt-dlp / ffmpeg / deno) の検出と自動取得。公開 IF は固定
- `src/config.rs` 設定 `~/.config/yd/config.toml`（favorites, recent(10件), last format/dest, template）
- `src/format.rs` 形式指定 (`mp3`, `mp3:320`, `mp4:1080`, `original` …) → yt-dlp 引数
- `src/ytdlp.rs` yt-dlp 起動とイベント (Info / Progress / Done / Error)
- `src/ui/` Codex CLI 風の描画（スタイル、ピッカー、進捗行）。`src/repl.rs` 対話モード

## コマンド
- `cargo build` / `cargo test` / `cargo clippy -- -D warnings`
- `cargo run -- <URL>` / `cargo run` (REPL)

## 決定事項
- コマンド名 `yd`。引数なし = インライン REPL、`yd URL` = 前回の形式・保存先で即 DL
- 既定ファイル名 `Uploader - Title.ext`（`%(playlist_title|.)s/%(uploader,channel|Unknown)s - %(title)s.%(ext)s`。`&{}/` 形式は `/` が `⧸` に化けるので不可）。プレイリストは `<playlist名>/` 配下。`v=` と `list=` 両方ある URL は既定で単体（`--playlist` で全体）
- 既存ファイルは削除しない。yt-dlp の既定どおりスキップ、`--force` で上書き
- 見た目は Codex CLI 基調: 角丸枠のヘッダ、`›` プロンプト、`•` 見出し + `└` 詳細、dim グレー中心、アクセントは cyan、成功 ✔ green / 失敗 ✘ red。絵文字は使わない
- UI テキストは日本語ではなく英語（短く）。README は日本語
- 外部ツールは tools dir (`~/.local/share/yd/tools`) → PATH → 自動 DL。PATH の yt-dlp は 90 日より古ければ使わない

## やらないこと
- フルスクリーン TUI、GUI、Ytdlp.NET 等ラッパーライブラリ

## テスト時の注意
- 設定は `YD_CONFIG_DIR`、ツールは `YD_TOOLS_DIR` で一時ディレクトリへ逃がす（実ユーザーの設定を汚さない）
- 実 DL テストは短い動画 https://www.youtube.com/watch?v=jNQXAC9IVRw で `/tmp/claude-1000/` 配下へ
- TUI の確認は pty 経由（python pty / script）。GUI ウィンドウは起動しない
