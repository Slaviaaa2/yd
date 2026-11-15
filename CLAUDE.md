# yd — fast yt-dlp front-end

URL を貼るだけで最短でダウンロードできる CLI / 対話 TUI。Rust 製の単一バイナリ。旧 C# 版 (YtdlpCLI, v0.x) は git 履歴と v0.2.0 タグにのみ残る。

## 技術と構成
- Rust 2024 / clap / ratatui + crossterm / serde + toml / ureq / arboard
- yt-dlp はライブラリを使わずプロセス直呼び。進捗は `--progress-template` の JSON を行単位でパース
- `src/tools/` 外部ツール (yt-dlp / ffmpeg / deno) の検出と自動取得（tools dir → PATH → DL。PATH の yt-dlp は 90 日より古ければ使わない）
- `src/config.rs` 設定 `~/.config/yd/config.toml`（favorites, recent, last format/dest, `[settings]`）、履歴 `<config dir>/history`
- `src/format.rs` 形式指定 (`mp3`, `mp3:320`, `mp4:1080`, `original` …) → yt-dlp 引数
- `src/ytdlp.rs` yt-dlp 起動とイベント、`src/clipboard.rs` Ctrl+V 用の読み取り
- `src/ui/` スタイル・ピッカー・ワンショット出力・端末制御、`src/repl/` 対話モード（入力・候補・ハイライト・モーダル）

## コマンド
- `cargo build` / `cargo test` / `cargo clippy --all-targets -- -D warnings`
- `cargo run -- <URL>`（ワンショット）/ `cargo run`（対話モード）

## 決定事項
- 見た目は Codex CLI 基調（角丸枠ヘッダ、`›` プロンプト、`•` 見出し + `└` 詳細、dim 中心、アクセント cyan、✔ green / ✘ red、絵文字なし）。**Codex への直接言及はこの CLAUDE.md だけ**に留め、README・コード・リリースノート・リポジトリ説明には書かない
- UI テキストは短い英語、README とリリースノートは日本語
- `yd URL` = 前回の形式・保存先で即 DL（`-f` `-o` `-p` で変更）。引数なし = 対話モード
- 対話モード: 代替スクリーン。カード → transcript → ライブ領域 → 入力欄 → footer を上詰めの 1 本の文書として描画し、入力欄は内容の直後。溢れたら末尾追従。終了時は元の画面に戻し保存ファイルの要約だけ出す。ワンショットと `-p` は通常画面のインライン
- キー: ↑↓（= ホイール）は Codex 方式（空 or 未編集の履歴なら履歴、候補リスト表示中は候補、それ以外は履歴で途中入力は下書き復元）。Shift+↑↓ は常に候補、Ctrl+P/N は常に履歴、PgUp/PgDn スクロール、Ctrl+V クリップボード貼り付け
- 行内修飾子 `URL /f x /o y /force` はその行だけ上書き（last_* は変えず recent にだけ記録）。保存先は `f:N` / `r:N`（0 始まり）で参照し `config::resolve_dest` に一本化。お気に入り名に `:` と空白は不可
- `/fav` と `/settings` は Esc まで入力を占有するモーダル
- ファイル名 `%(playlist_title|.)s/%(uploader,channel|Unknown)s - %(title)s.%(ext)s`（`&{}/` 形式は `/` が `⧸` に化けるので不可）。`v=` と `list=` 両方ある URL は既定で単体
- 既存ファイルは削除しない（スキップ、`--force` で上書き）

## やらないこと
- マウスキャプチャ（テキスト選択を残す）、GUI、Ytdlp.NET 等ラッパーライブラリ

## テスト時の注意
- 設定は `YD_CONFIG_DIR`、ツールは `YD_TOOLS_DIR` で一時ディレクトリへ逃がす（実ユーザーの設定を汚さない）
- 実 DL テストは短い動画 https://www.youtube.com/watch?v=jNQXAC9IVRw で `/tmp/claude-1000/` 配下へ
- TUI の確認は pty 経由（python pty + pyte）。GUI ウィンドウは起動しない
- CI は Linux / macOS / Windows で clippy + test。パスを扱うテストは Windows でも通る書き方にする

## 配布
- `Cargo.toml` の version を上げてタグ `vX.Y.Z` を push → `release.yml` が 5 ターゲットをビルドし draft release に `yd-<os>-<arch>.{tar.gz,zip}`（版番号なし、`latest/download` で固定 URL）を添付 → ノートを書いて publish
- publish で `install-test.yml` が install.sh / install.ps1（pwsh・powershell）/ install.cmd を実機ランナーで検証
- インストーラ: `install.sh`（curl | bash）、`install.ps1`（irm | iex、PS 5.1 互換）、`install.cmd`（cmd 用、pwsh 優先）
