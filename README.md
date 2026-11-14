# yd

URL を貼るだけで最短でダウンロードできる yt-dlp フロントエンド。
ワンショットの CLI と、Codex CLI 風のインライン対話モードを持つ単一バイナリです。

```
╭───────────────────────────────────────────────╮
│ >_ yd (v1.0.0)                                │
│                                               │
│ format:   mp3 · 320k             /f to change │
│ save to:  ~/Music  ★ music       /o to change │
╰───────────────────────────────────────────────╯
› https://www.youtube.com/watch?v=jNQXAC9IVRw
• Me at the zoo
  └ jawed · 0:19 · Youtube
✔ Saved ~/Music/jawed - Me at the zoo.mp3
  └ 524.7 KiB · mp3 · 320k · 2s
```

## インストール

```sh
cargo install --path .
```

yt-dlp・ffmpeg・deno は自動で用意されます（`~/.local/share/yd/tools` → PATH → 自動ダウンロードの順）。
PATH 上の yt-dlp は 90 日より古ければ使わず、新しいものを取得します。`yd update` で更新できます。

## 使い方

```sh
yd                         # 対話モード（URL を貼って Enter。DL 中も次の URL を貼ればキューに入る）
yd URL                     # 前回の形式・保存先ですぐ DL
yd -f mp3:320 -o music URL # 形式と保存先（お気に入り名 or パス）を指定
yd -p URL                  # 形式と保存先をピッカーで選ぶ
yd URL1 URL2 ...           # 複数をまとめて
yd URL -- --limit-rate 2M  # -- 以降は yt-dlp にそのまま渡す
```

| オプション | 内容 |
| --- | --- |
| `-f, --format <SPEC>` | 形式（下表）。省略時は前回の形式 |
| `-o, --out <DEST>` | お気に入り名またはディレクトリ。省略時は前回の保存先 |
| `-p, --pick` | 形式・保存先をピッカーで選ぶ |
| `-t, --template <T>` | 出力ファイル名テンプレート |
| `--force` | 既存ファイルを上書き（既定はスキップ） |
| `--playlist` | `v=` と `list=` 両方ある URL でプレイリスト全体を DL |
| `--cookies <BROWSER>` | ブラウザの Cookie を使う（`firefox`, `chrome` など） |
| `--no-deno` | deno を用意しない |

### 形式

`<形式>[:<品質>]` で指定します。

| 種類 | 形式 | 品質 |
| --- | --- | --- |
| 音声 | `mp3` `m4a` `aac` `opus` `ogg` `flac` `wav` `alac` `aiff` | kbps（`320` `256` `192` `128`）、既定 `best` |
| 動画 | `mp4` `mkv` `webm` `mov` `avi` `gif` | 高さ（`2160`/`4k` `1440`/`2k` `1080` `720` `480` `360`）、既定 `best` |
| 無変換 | `original` | — |

例: `mp3:320`、`flac`、`mp4:1080`、`mkv:4k`、`gif:360`

### 保存先（お気に入り・最近）

```sh
yd fav add music ~/Music   # お気に入り登録（パス省略でカレント）
yd fav ls                  # お気に入りと最近使った保存先（10 件）
yd fav rm music
```

`yd fav ls` と `/fav` の一覧には番号が付きます（`f:0` = 1 番目のお気に入り、`r:0` = 最近使った順の 1 番目。0 始まり）。保存先を受け取る所ならどこでも `f:N` / `r:N` が使えます。範囲外はエラーです。

```sh
yd -o f:0 URL              # 1 番目のお気に入りへ
yd -o r:1 URL              # 2 番目に最近使った保存先へ
```

お気に入り名に `:` は使えません。対話モードでは `/o` でピッカー（`f:0 ★ お気に入り`、`r:0 · 最近`、`+ パス入力`・Tab 補完）を開けます。

### 対話モードのコマンド

| コマンド | 内容 |
| --- | --- |
| `/f [spec]` | 形式を変更（引数なしでピッカー） |
| `/o [fav\|path]` | 保存先を変更（引数なしでピッカー） |
| `/fav <name>` / `/fav rm <name>` / `/fav` | 今の保存先をお気に入り登録 / 削除 / 一覧 |
| `/open` | 保存先をファイルマネージャで開く |
| `/help`, `/q` | ヘルプ、終了 |

#### 1 行だけの上書き

URL と一緒に修飾子を書くと、その行の URL にだけ適用されます。現在の形式・保存先と設定ファイルの前回値は変わりません。順序は自由です。

```
https://youtu.be/xxxx /f ogg /o f:0
/f mp4:720 /o ~/clips URL1 URL2
URL /force
```

| 修飾子 | 内容 |
| --- | --- |
| `/f <spec>`（`/format`） | この行だけ形式を変える |
| `/o <dest>`（`/out`） | この行だけ保存先を変える（お気に入り名・`f:N`・`r:N`・パス） |
| `/force` | 既存ファイルを上書きして再 DL |

URL を含まない行の `/f ogg` などは、これまでどおり設定の変更です。不正な指定があると何もキューに入れずエラーにします。上書きした項目は情報行に `ogg · best → ~/Music` のように表示されます。DL に成功した保存先は「最近」には記録されます。

`Ctrl+C` は DL 中ならキャンセル（キューも破棄）、待機中なら終了します。

## ファイル名

既定は `アップロード者 - タイトル.拡張子`。プレイリストは `<プレイリスト名>/` に入ります。
同名ファイルがあればスキップします（`--force` で上書き）。
変更は `-t` か設定ファイルの `template` で（yt-dlp の出力テンプレート書式）。

## 設定

`~/.config/yd/config.toml`（`yd config` で場所と中身を表示）。お気に入り、最近の保存先、前回の形式・保存先、テンプレートを保存します。

## 開発

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

`YD_CONFIG_DIR` / `YD_TOOLS_DIR` で設定とツールの置き場所を差し替えられます。
