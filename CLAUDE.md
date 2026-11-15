# yd — fast yt-dlp front-end

URL を貼るだけで最短でダウンロードできる CLI / 対話 TUI（代替スクリーン）。旧 C# 版 (master ブランチ) を Rust で全面書き直し中 (branch `rust-rewrite`)。

## 技術と構成
- Rust 2024 / clap / ratatui (REPL は代替スクリーン + Viewport::Fullscreen、`-p` のピッカーだけ短命の Viewport::Inline) + crossterm / serde + toml / ureq
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
- コマンド名 `yd`。引数なし = REPL（代替スクリーン。カード・transcript・ライブ領域・composer を 1 本の文書として上詰めで描画し、composer は内容の直後に出る。画面を超えたら末尾を表示。終了時に元の画面へ戻り、保存したファイルの要約だけ通常画面に出す）、`yd URL` = 前回の形式・保存先で即 DL
- 既定ファイル名 `Uploader - Title.ext`（`%(playlist_title|.)s/%(uploader,channel|Unknown)s - %(title)s.%(ext)s`。`&{}/` 形式は `/` が `⧸` に化けるので不可）。プレイリストは `<playlist名>/` 配下。`v=` と `list=` 両方ある URL は既定で単体（`--playlist` で全体）
- 既存ファイルは削除しない。yt-dlp の既定どおりスキップ、`--force` で上書き
- 見た目は Codex CLI 基調: 角丸枠のヘッダ、`›` プロンプト、`•` 見出し + `└` 詳細、dim グレー中心、アクセントは cyan、成功 ✔ green / 失敗 ✘ red。絵文字は使わない
- UI テキストは日本語ではなく英語（短く）。README は日本語
- 外部ツールは tools dir (`~/.local/share/yd/tools`) → PATH → 自動 DL。PATH の yt-dlp は 90 日より古ければ使わない
- 保存先は `f:N`（N 番目のお気に入り）/ `r:N`（N 番目の最近）で参照でき、`config::resolve_dest` に一本化（0 始まり、一覧の表示順）。お気に入り名に `:` は不可。REPL は `URL /f x /o y /force` の行内修飾子でその行の DL だけ上書きでき、last_* は変えず recent にだけ記録
- ワンショット出力とピッカー（`-p`）は通常画面のインライン表示のまま。REPL は PgUp/PgDn でスクロール、End で末尾追従に戻る。↑↓（= ホイール）と Ctrl+P/N は入力履歴。/fav と /settings は Esc まで入力を占有するモーダル（設定は config.toml の `[settings]`、履歴は `<config dir>/history`）。入力中はゴースト文字 + 候補リスト（Shift+↑↓ で選択、↑↓ は常に履歴、Tab/→ で確定、Enter は未完成の語なら補完・完成済みなら実行）と、修飾子/コマンドのシンタックスハイライト

## やらないこと
- マウスキャプチャ（テキスト選択を残す）、GUI、Ytdlp.NET 等ラッパーライブラリ

## テスト時の注意
- 設定は `YD_CONFIG_DIR`、ツールは `YD_TOOLS_DIR` で一時ディレクトリへ逃がす（実ユーザーの設定を汚さない）
- 実 DL テストは短い動画 https://www.youtube.com/watch?v=jNQXAC9IVRw で `/tmp/claude-1000/` 配下へ
- TUI の確認は pty 経由（python pty / script）。GUI ウィンドウは起動しない
