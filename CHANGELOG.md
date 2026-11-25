# Changelog

OpenKeito の変更履歴。Conventional Commits 1.0.0 のコミットをもとに書き、バージョンは SemVer 2.0.0 に従う。
バージョンの正は `Cargo.toml` の `[workspace.package]` の `version`。手順は `AGENTS.md` の「Versions and releases」を参照。

## 0.12.0 (2026-11-25)

M10 段 1（形式 `"0.8"` の数式制御ブロック `expression` の先行公開）: 任意の代数式・初等関数式を文字列で記述できる制御ブロック `expression`（1 入力・複数入力、`variables` 別名マップ、位置変数 `u0, u1`）と、外部クレートを増やさない純 Rust の式パーサ・事前コンパイル済み評価器、式内位置・`hint` 付き診断 `E243`、ブラウザ UI の図記号・フォーム編集、自己完結例 `examples/expression_control.json` を追加。モデル形式の版を `0.8` に上げ、`0.1`〜`0.7` の既存モデルはそのまま読め、v0.11.0 の例 22 件の実行結果 CSV（53 ファイル）は 0.11.0 とバイト単位で同じ。

### Features

* **model, core:** モデル形式 `0.8` を追加: 数式制御ブロック `expression`（`inputs`、`variables` 別名マップ、位置変数 `u0, u1`、定数 `pi, e`、右結合 `^`、14 関数 `sin`/`cos`/`tan`/`atan`/`atan2`/`abs`/`sqrt`/`exp`/`ln`/`sign`/`min`/`max`/`hypot`/`pow`）、外部クレートを使わない再帰下降パーサと事前コンパイル済み評価器（`interpolated` 用 `preview` 対応）、構文誤り・未定義変数/関数・引数数誤りの式内位置・`hint` 付き診断 `E243`、直達ループ `E171`、非有限値停止 `E251`、旧形式 `"0.7"` 以下での使用拒否 `E102` を追加
* **examples, web:** `examples/expression_control.json` を追加（`web/examples.json`・`README.md` 登録）し、`web/src/{symbols,inspector,inspector_panel}.js` に `CONTROL.expression` 記号（カタログ 58 種）と `variables`（`additionalProperties`）入力・`E243` 診断紐付け、`web/tests/expression.test.mjs` を追加

### Documentation & Release

* **docs:** `docs/model.md`（形式 `0.8`、`expression` の構文・演算子優先順位・関数一覧・`E243`）・`docs/numerical-method.md`・`README.md`・`web/README.md`・`AGENTS.md`・`CITATION.cff` を更新
* **chore:** 公開リポジトリ向けの URL・`.zenodo.json`・DOI バッジ・Windows CI 設定を更新（`release/public-prep`）

## 0.11.0 (2026-11-24)

M9 (形式 `"0.7"` の制御ブロック拡張・部品単位定常除外・WECC 汎用 IBR 例と検証材料): 1 次元折れ線テーブルブロック `piecewise_linear`、`lag`・`pi`・`limiter`・`integrator` の上下限への信号名指定、`pi` のアンチワインドアップ方式 `"anti_windup"`（`"conditional"` | `"state"`）、部品単位の定常初期化除外 `"initialization": "exclude"` と接続・直列要素の診断（`E183`・`W185`）、自己完結の WECC 汎用 IBR 例（`examples/wecc_ibr.json`）、および公開用の論文図表・検証テストを追加。モデル形式の版を `0.7` に上げ、`0.1`〜`0.6` の既存モデルはそのまま読め、既存の例 21 件の実行結果 CSV（50 ファイル）は 0.10.0 とバイト単位で同じ。

### Features

* **model, core:** モデル形式 `0.7` を追加: 区分線形補間（範囲外端値保持）の `piecewise_linear`（誤った配列・非単調は `E242`、非有限は `E251`）、`integrator`・`lag`・`pi`・`limiter` の上限・下限に定数または信号名を取れる `LimitValue`（`min_signal`/`max_signal` 相当、制限信号のループは `E171`、`wrap: true` 併用は `E132`）と非ワインドアップ処理、`t = 0` での初期状態の `[min, max]` クランプを追加
* **model, core:** `pi` ブロックに `"anti_windup"`（既定 `"conditional"` の条件付き積分、または IEEE Std 421.5 型の状態制限積分 `"state"`）を追加
* **model, core:** 全 26 種の回路部品に `"initialization": "exclude"`（既定 `"include"`）を追加。除外部品は定常フェーザ解から外し、時間領域のゼロ初期状態（`t = 0+`）から開始する。残余回路の全除外・接地欠落・電圧未保持は `E183` で拒否し、直列・多端子部品への誤適用には警告 `W185` を出す
* **examples, web:** `REPCA1` / `REECA1` / `REGCA1` 構成を `piecewise_linear` と可変上下限で組んだ自己完結モデル `examples/wecc_ibr.json` を追加し、`web/examples.json`・`web/src/{symbols,inspector,inspector_panel}.js` に `CONTROL.piecewise_linear` 記号と形式 `0.7` 入力欄を追加

### Documentation & Paper

* **paper:** `paper/figures/controls_v07.py`（`piecewise_linear`、動的上下限での 2 次収束、`pi` の `"anti_windup"` 比較、`"initialization": "exclude"` の解析解・明示ブロック構成との照合）および `paper/figures/wecc_ibr.py`（`examples/wecc_ibr.json` の定常・事故応答・復帰図）と `python/tests/test_paper_figures.py` の検査を追加
* **docs:** `docs/model.md`（形式 `0.7`、`piecewise_linear`、信号上下限、`anti_windup`、`"initialization": "exclude"`、`E183`・`W185`・`E242`）、`docs/numerical-method.md`（非ワインドアップ積分と 2 方式の差、定常除外の行列処理）、`docs/python.md`・`README.md`・`paper/README.md`・`CITATION.cff` を更新

## 0.10.0 (2026-11-24)

M8 (#26 実用 GUI 拡充と #41 公開準備の下ごしらえ): 回路図と連動する構造化編集・ドラッグ配置と配置サイドカー、電圧・電流の大きさ色分けと三相詳細展開、一括実行結果の遅延読込・重ね波形比較ビュー、キーボード・クロスブラウザ改善、および公開マニフェスト検査・サードパーティライセンス一覧・`--remap-path-prefix` ビルド設定を追加。モデル形式の版は `0.6` のまま、`0.1`〜`0.6` の既存モデルはそのまま読め、既存の例 21 件の結果（50 CSV ファイル）は 0.9.0 とバイト単位で同じ。

### Features

* **web, py:** 配置サイドカー形式 `openkeito-layout` `0.1`（`web/src/layout_sidecar.js`、`python/openkeito/layout.py` の座標・pandapower からの生成、自動配置への統合）と、図・JSON 往復編集、Undo/Redo、全 56 種のカタログ入力補助、`validate` / `E001` 診断マッピング、3 px しきい値のドラッグ移動、サイドカー JSON の入出力と `localStorage` 保存を備えた構造化編集パネル（`web/src/inspector.js`・`web/src/inspector_panel.js`）を追加
* **web:** 三相母線の空間ベクトル大きさ・単相の直前 1 周期最大値による電圧（基準ピークの 0.45・0.9・1.1 倍）および電流（最大の 0.15・0.5・0.85 倍）の大きさ色分け（`web/src/overlay.js`、`render.js` の `data-heat`）と上限 400 部品の三相詳細展開（`web/src/layout.js` の `expandThreePhase`）を追加
* **web:** `openkeito batch` の出力フォルダを読み込み、初読時は `batch-summary.json`・`batch-metrics.csv` のみを展開して個別ケース `result.csv` を選択時に遅延読込する一括実行ビュー・フィルタ/ソート・複数ケース重ね描き比較を追加（`web/src/batch.js`・`web/src/batch_view.js`）
* **cli, py:** `openkeito batch` と Python `run_batch` が各有効ケースの出力フォルダ（`<dir>/<case>/model.json`）にモデルの写しを書き出すように変更（書込失敗時は警告 `W300`、既存の出力 CSV と並列ビット一致は不変）

### Bug Fixes

* **web:** 部品・ブロックへの `focusable="true"` 付与と `:focus-visible` 輪郭、`.diagram-view` からの Enter / 矢印キー移動、`setPointerCapture` や狭い画面幅（390 px）での折り返し・タッチ操作などキーボード・クロスブラウザの挙動を改善（`web/src/render.js`・`web/src/diagram.js`・`web/src/chart.js`・`web/style.css`）

### Build & Documentation

* 公開マニフェスト（`scripts/public_manifest.json`・`scripts/check_public_manifest.py`、公開切り出し時の `public_redactions` 除外ルールと `--strict` / `--export-dir` 対応）、Rust 外部クレート 133 件の `THIRD-PARTY-LICENSES` 生成・検証（`scripts/generate_licenses.py`）、CI（`.github/workflows/ci.yml`）の `--remap-path-prefix` と厳格マニフェスト検証、技術宿題メモ（`scripts/README.md`）を追加
* 公開向け整理として `docs/performance.md` から外部解析ソフトとの比較節を削除し、`docs/numerical-method.md`・`crates/openkeito-core/tests/steady_state.rs` の 2S-DIRK 誤差記述から固有製品名・ローカル比較表現を削除、`README.md`・`web/README.md`・`docs/model.md`・`docs/python.md`・`AGENTS.md`・`plan.md`・`CITATION.cff` を更新

## 0.9.0 (2026-11-23)

M7: 読み込み誤り (`E001`) の一括返却、1 本の推移での停止機再並列と変圧器タップ切替、YNyn6 変圧器の零相符号反転、および Linux での MCP 出力リンク拒否メッセージ修正。モデル形式の版を `0.6` に上げ、`0.1`〜`0.5` の既存モデルはそのまま読め、既存の例 21 件の結果は変わらない。

### Features

* **model:** `read.rs` を追加し、JSON をいったん `serde_json::Value` に読み、構文誤りを除いた重複キーと各セクション・要素（`simulation`・`components[i]`・`controls[i]`・`outputs[i]`・`frequency_scan`）の `E001` を一括で返すように変更。`Diagnostic` に `path`（`line X column Y`）に加えて `json_path` と `at <json_path>` を追加
* **model, core:** モデル形式 `0.6` を追加: `transformer`・`three_phase_transformer`・`three_phase_ideal_transformer` に `tap_changes`、`coupled_rl` に `impedance_changes`、`three_phase_ideal_transformer` に `zero_sequence_inverted` を追加。高圧側タップ変更時に線形・飽和磁化枝の鎖交磁束/ターンを連続に保ち、時刻ごとのステップ切り替えを `events.csv` に `step` として記録
* **py (powsybl):** 両側が既知の `Y_GROUNDED` で時計数 6 (YNyn6) の 2 巻線・3 巻線変圧器に `zero_sequence_inverted: true` を設定して零相の符号を反転し、零相の仮定警告を外した（結線不明または時計数 0・6 以外の警告は維持）
* **py (transition):** 1 本でつなぐ推移で、変圧器タップ位置・直列インピーダンスの時刻指定切替（`tap_changes`・`impedance_changes`）に対応。また停止して遮断器を開いた同期機の再並列を、停止中の `abc_to_dq0` による系統母線電圧・位相追従と投入後への無衝撃切替で実現（`governor` または `exciter` が無い機の再並列は `E700` と案内付き `ConversionError` で拒否）

### Bug Fixes

* **mcp:** `query_results`（`python/openkeito/mcp_server.py`）が `out` 配下の結果ファイルのリンクを検証する際に、解決用のフルパスではなく利用者が渡した引数の名前（`out`）を拒否メッセージに表示するように修正（Linux で `test_links_inside_an_output_directory_are_refused` が文字列差で落ちる問題を解消）

### Documentation

* `docs/model.md`（形式 `0.6`、`E001` 一括返却・`json_path`・重複キー、`E700`）、`docs/python.md`（YNyn6、再並列・タップ切替）、`docs/numerical-method.md`、`README.md`、`plan.md`、`CITATION.cff` を更新

## 0.8.0 (2026-11-23)

M6 (回路の図): モデルの接続を機械可読に出す `topology` (CLI・WebAssembly・Python・MCP) と、ブラウザのページで回路図・制御の
ブロック線図を描いて JSON・診断・実行結果と連動させる表示。モデル形式の版は `0.5` のまま。既存の例 21 件の `result.csv`・
`events.csv`・`phasors.csv` は、0.7.0 とバイト単位で同じ。

### Features

* **model, core, cli:** モデルの接続 (部品と端子・ノード・母線・制御ブロック・信号・診断の対象) を JSON で返す
  `openkeito topology <model> [--out <dir>]` を追加
* **wasm, py, mcp:** WebAssembly の `topology(text)`、Python の `openkeito.topology(model)`、MCP サーバーの
  `get_topology` を追加
* **web:** モデルの回路図 (単相は回路図、三相は単線結線図、IEC 60617 の記号 56 種) と制御のブロック線図を描く
  `Diagram` タブ、図 ↔ JSON の相互選択と診断の印、実行結果の重ね描き (波形のカーソルと連動する値、イベント時刻に合わせた
  スイッチ・ダイオード・故障の状態、部品から波形への列追加)、長い名前の折り返しと全体の概観を追加

### Documentation

* `docs/model.md` (Topology と `result.columns` の `kind`・`target`)、`docs/python.md`、`docs/mcp.md`、`web/README.md`、
  `crates/openkeito-wasm/README.md`、`README.md`、`AGENTS.md`、`CONTRIBUTING.md` を更新

## 0.7.0 (2026-11-23)

M4・M5 のレビューで見つかった不具合の修正 (#33〜#37)、リファクタリング (#20)、および推移の計算時間の見積もりと途中の整定不足の
警告。モデル形式の版は `0.5` のまま。既存の例 21 件の `result.csv` と `events.csv` は、0.6.0 とバイト単位で同じ。

結果や出力の形が変わるもの:

- `interpolated` で、分布定数線路 (`bergeron_line`) とダイオード (または格子点に寄せられたゲート時刻) を同時に持つモデルの、
  格子点での動作に続く刻みの解 (#33)。
- 開いた理想スイッチや動作していない故障などの理想枝の電流がつながるノードを、大きいインダクタンス ($L > 10^7 h$) や小さい
  コンダクタンス ($G < 10^{-13}\text{ S}$) だけで支えたモデルが、$t = 0^+$ の E200 や定常解の E203 で止まらずに完了する (#34)。
  既に完了していたモデルの結果と条件数の見積もりはビット単位で変わらない。
- `bus_voltage` と `node_voltage` で同じ名前の列ができるモデルが E155 で、極端な素子値で段の係数が有限でなくなるモデルが
  E130 で、重複キーを持つインラインの一括実行モデルが E001 で拒まれる (#35)。
- 失敗した `openkeito batch` の JSON 文書のキーが `model` から `batch` (および `"command": "batch"`) になり、`openkeito run`・
  `scan` に一括実行や計画値のファイルを渡したときの診断が E001 から E300 (wasm は E600) になった (#35)。
- `--force` で不正なモデルを `run`・`scan` に渡したとき、出力フォルダの古い結果ファイル (`summary.json`・`result.csv` など) を
  消す (#35)。
- Python の推移の計算で、正弦波の電源が残らない系統 (`ext_grid` と負荷・枝だけ) が 0 からではなく潮流の定常状態から始まる。
  PowSyBl で初めの時刻に電圧の無い母線があとの時刻に生きる推移、電圧階級が混ざった系統で枝を開閉する推移、PowSyBl で対地
  アドミタンスを持つ枝を開閉する推移のモデルが直った (#36)。
- PowSyBl・pandapower の変換で、有効電力 (またはコンダクタンス) が負で無効電力が 0 でない負荷・対地アドミタンスは、無効電力の
  分を定インピーダンスとして残し、負の有効電力の分だけを電流源にする (IEEE 118 の母線 VL87 の波形の角度差が 2.4e-2° から
  4.0e-3° の 2 次精度に改善) (#36)。
- `TransitionRun.time_rows` の各行に `settled` が加わり、途中の時刻の整定が足りないときに警告が出る (#36)。

### Features

* **py:** 推移の計算の長さと実時間の見積もり (`openkeito.transition.estimate`、`Transition.estimate`、MCP `run_transitions` の
  `estimate_only`・`max_wall_s`) と、途中の時刻の整定不足の警告 (`settled`) (#36)
* **py:** `openkeito.run` にスレッド数の指定 `threads` を追加 (#35)

### Bug Fixes

* **core:** `interpolated` で、格子点での動作のあとの試しの解に `floor(t/h)` から作った刻みの終わりを渡していたため、格子点の
  1〜8 % で分布定数線路に長さ 0 の区間を渡して第 1 段の履歴位置がずれていたのを修正。制御ループで NaN が出たときに即座に抜けて
  E251 を返し、$t = 0^+$ の段を `Stage::is_initial` で判定する (#33)
* **core:** 密・疎の LU の特異判定に列の尺度を入れ、理想枝の電流の ±1 と同じ行にある小さいコンダクタンスが特異と誤判定されるのを
  修正 (#34)
* **model:** `bus_voltage` と `node_voltage` の列名の重複を E155 で検出し、インラインの一括実行モデルを `RawValue` で保持して
  重複キーを E001 で検出し、非有限の段係数になる極端な素子値を E130 で検出し、診断の連続空白 (11 か所) と `untagged` 型・E001 の
  hint を改善 (#35)
* **cli:** 標準出力のパイプが閉じたときに `BrokenPipe` でパニックしないようにし、`--force` で検証に失敗したときも古い結果ファイルを
  消し、`run`・`scan` に一括実行・計画値ファイルを渡したときに E300 (wasm は E600) を返し、失敗した `batch` の JSON キーを `batch` に
  そろえた (#35)
* **py:** 正弦波の電源が残らない推移の 0 A 周波数基準、PowSyBl の後から生きる母線の名前付け替えの衝突、開閉枝の端の 1 W 抵抗の
  定格電圧、開閉枝の対地アドミタンスの位置、負の有効電力と無効電力の分離、CGMES 境界線路の `b0ch`・`g0ch` と `urn:uuid:` の id、
  `machine_parameters` の `generator/<id>` 指定と未知キーの検査を修正 (#36)
* **mcp:** pandapower JSON の `_class` の検査、CGMES XML の DTD 拒否、`run_time_slices` の変換警告の返却、`jobs` の範囲検査を修正 (#36)
* **web:** `build.mjs --out` の削除保護と別ドライブ対応、Worker 読み込み失敗時の表示、実行中の例の切り替え、壊れた URL ハッシュと
  `hashchange`、`formatValue(null)`・ほぼ一定の列の目盛り・空白 CSV セル・日本語を含む行の `locate` を修正し、`decimate` の単体テストを
  追加 (#37)

### Performance Improvements

* **core:** 出力の行を 256 行ずつためて書き出し、時計の計測を 256 行に 1 組に削減 (降圧チョッパで 166 → 142 ns/刻み、整流器で
  157 → 132 ns/刻み) (#20)

### Refactoring・Documentation・CI

* **core:** 分割判定とソルバの重複を整理し、残っていた 6 つのテストファイルの実行補助を `tests/common` に統合 (#20)
* **model, cli, py, web, paper:** `validate.rs` (W240・Union-Find)・カタログの `name`、CLI の失敗文書の外枠、`transition.py` の
  組み立てと `make_transitions`、`web/src` と `paper/` の重複を整理 (#20, #36, #37)
* **docs, ci:** `docs/numerical-method.md`・`docs/model.md`・`docs/python.md`・`docs/mcp.md`・`paper/README.md`・`crates/openkeito-cli/README.md`
  を更新し、CI の Python ジョブに `paper` と `permissions: contents: read` を追加 (#37)

## 0.6.0 (2026-11-22)

M5: WebAssembly のモジュールとブラウザのページ、PSS と 3 時刻以上の推移、MCP サーバーのルート、M4 から残った宿題、
公開に向けた文書とメタデータ、論文の図表を再現するスクリプト。モデル形式の版は `0.5` のまま。

結果や出力の形が変わるもの:

- 不正なモデルの文書 (`validate`・`run`・`scan`・`batch`、wasm、Python、MCP) で、`errors` はエラーだけになり、`warnings` が
  必ずある。これまでは警告が `errors` に混ざり、`warnings` のキーが無かった。Python の `OpenKeitoError.diagnostics` は
  エラーだけになり、`warnings` が増えた。
- 読み込みの誤り (E001) の `path` が誤った名前・値そのものの位置になり、`message` から「at line N column M」が外れ、`hint` が
  実際の値から作られる。
- これまで検証を通っていたモデルの一部が E139 で拒まれる (実行が使う段の係数の全部でピボットを見る。多くは、実行すれば
  E204 で止まっていたモデル)。`frequency_scan` とダイオード・制御スイッチのあるモデルは、`validate`・`run` でも W192 が出る。
- `interpolated` で、同じ時刻に開いて閉じたダイオードの 2 件が `events.csv` に残らない (`result.csv` は変わらない)。
- 推移の計算 (`transition`・`run_transitions`) は、既定で同期機に PSS を付け、定格の無い発電機の定格を全部の時刻から決める。
  `pss=False` で PSS は付かない。時間断面の計算は変わらない。
- PowSyBl の変換で、時計数が 0 でない変圧器があると警告が 1 つ増える。CGMES 3.0 の同期機の `r0`・`x0`・`r2`・`x2` を
  Ω として読む。
- 既存の例の `result.csv` と `events.csv` は、0.5.0 とバイト単位で同じ。

### ⚠ BREAKING CHANGES

* **mcp:** `openkeito-mcp` は、ルート (既定は起動時の作業フォルダ) の外のパスを断る。相対パスはルートからの相対になった。
  外のファイルを使うには、`--root <フォルダ>` か環境変数 `OPENKEITO_MCP_ROOT` でルートを替えるか、`--allow-any-path` を
  付けて起動する (740e221)
* **cli:** 不正なモデルの文書の `errors` から警告が外れ、`warnings` に入る (4a0c1c5)
* **cli:** ライブラリのコマンドと `batch` は feature `native` (既定で有効) に移った。`default-features = false` で使うと
  無くなる (851d84e)

### Features

* **wasm:** クレート `openkeito-wasm` (wasm32-unknown-unknown、単一スレッド。`validate`・`run`・`scan`・`schema`・
  `components` を JSON の文字列で受けて返す。一括実行のファイルは E600) と、Node でネイティブと比べるテスト (b5bef61)
* **web:** ブラウザでソルバを動かすページ `web/` (例の実行と波形、モデルの編集と診断、周波数スキャン。Cloudflare Pages の
  手順つき) (c3a797c)
* **cli:** ファイルを使わない実行 `run_texts`・`scan_texts` (851d84e)
* **py:** PowSyBl の変換で、変圧器の時計数をモデルで回せる (`phase_angle_clocks="rotate"`。既定は `ignore`)。短絡データの
  零相の励磁リアクタンス `xm` を読む (f4ed264、b3f02a9)
* **py:** 推移の計算の同期機に PSS (IEEE Std 421.5 の PSS1A 形。`openkeito.transition.PSS`、`add_machine_controls(pss=...)`)。
  有効電力が定格の 10 % に満たない機には付けず、警告に出す。`measure(end_time_s=...)` (a5b8667)
* **py:** 計画値の 3 時刻以上を 1 本の推移の計算でつなぐ (`build_sequence`、`transition(times=[...])`、
  `run_transitions(chain=True)`、`measure(time=...)`、`TransitionRun.time_rows`、`switch_delay_s`、区間ごとの長さのリスト)
  (ce766b9)
* **mcp:** `--root`・`--allow-any-path`・`--version`、`run_transitions` の `chain`・`times` (740e221、d900dd0)
* 例 `examples/transition_three_times.json` (78afdbd)

### Bug Fixes

* **model:** スキャンでダイオードをオフとみなす警告 (W192) を検証で出す (1d5843b)
* **model:** 直列インピーダンスの条件 (E139) を、実行が使う段の係数の全部で確かめる (3fb50f0)
* **model:** 読み込みの誤り (E001) の hint と位置を、実際の値から作る (8d4fef1)
* **core:** 同じ時刻に開いて閉じたダイオードをイベントに残さない (4130b1f)
* **py:** CGMES 3.0 の同期機の `r0`・`x0`・`r2`・`x2` を pu として読んでいた (b3f02a9)
* **py:** 推移の計算で、定格の無い発電機の定格を全部の時刻から決める。これまでは初めの時刻の出力から決めていて、
  出力が増える機が調速機の弁の上限に当たっていた (b1d1cac)
* **py:** 推移の計算で、負荷の有効電力だけを計画したときなどに、入り切りする部分がインダクタンスだけになり、実行が
  t = 0 の E200 で止まることがあった。そのような部分に 1 W を持たせる (38e71d9)

### Performance Improvements

* **core:** 小さい回路の 1 刻みから、分割の確認と毎刻みの確保を除く (降圧チョッパで 215 → 168 ns/刻み) (174b758)

### Refactoring・Documentation・Build

* **core:** 計時を `clock::Stopwatch` にまとめる (wasm で差し替えられる) (3a2fa28)
* **core:** テストの補助関数を `tests/common` にまとめる (d4b0343)
* README を整え、モデル・Python・MCP の説明を `docs/model.md`・`docs/python.md`・`docs/mcp.md` に分けた。`CONTRIBUTING.md`、
  `SECURITY.md`、`CITATION.cff`、issue と PR のテンプレートを足した (dbc2439、fd99144)
* 文献の書誌を出版社の記録で確かめて直した (23260ad)
* `paper/`: 図表を再現するスクリプトと材料の整理。任意の extra `paper` (84428b7、0a3838b)
* crates.io・PyPI 向けのメタデータ (`rust-version` 1.85 を宣言)。CI で依存のライセンスを `cargo deny` で確かめる
  (f360221、df667af)
* **ci:** ワークフローは手で始めたときだけ動く。マージの基準は手元の Windows の検査 (4ce107a)

## 0.5.0 (2026-11-22)

M4: 分布定数線路、非線形素子、制御ブロック、同期機、変換器の平均値モデル、刻みの中のスイッチング時刻、推移の計算、
線路での回路分割と並列化。モデル形式の版は `0.5` になった (0.1〜0.4 のモデルはそのまま読む)。

結果が変わるもの:

- `coupled_rl` (と、それに展開される部品) で零相と正相の比が大きいモデルの、t = 0+ の行 (比 1e6 で母線電圧が 1e-4
  ずれていた)。
- `bergeron_line` とスイッチ動作のあるモデルは新しい部品なので、該当する既存のモデルは無い。既存の例の `result.csv` と
  `events.csv` は、0.4.1 とバイト単位で同じ。
- これまで検証を通っていたモデルの一部が拒まれる: 理想素子のループ (E142)、電圧を支えるものの無いノード (E143)。
  どちらも、実行すれば E200 で止まっていたモデル。測定が選んだ端子・相の列が無いこと (E173) は `validate` で出る。
- `summary.json` の `solver` と `simulation` にキーが増えた (反復の回数、刻みの中の切り替えの回数、部分の数、スレッド数)。
- `openkeito-core` の `Element` (pub な trait) が変わった (下の BREAKING CHANGES)。モデルの形式、CLI、Python の API に
  破壊的な変更は無い。

### ⚠ BREAKING CHANGES

* **core:** `Element::prepare(&[f64])` は `prepare(&Stages)` に、`apply_gate` は `read_signals` になった。素子は `Send` で
  あること。素子が受け取る解と右辺は、自分の部分回路のスライスになった (3d159e1、c5846df)

### Features

* **model:** モデル形式 0.5。部品と制御ブロックを族ごとのモジュールに分け、部品を足すときに触る所を族のファイルと
  一覧の 1 行にした (13f32af、7f143fb、6c38637)
* **core:** 段ごとの動作点の反復 (収束しなければ E205)、素子に渡す刻み幅と段の表、信号の出入りの口 (3d159e1)
* **core:** 分布定数線路 `bergeron_line`。損失は R/4・R/2・R/4 に集中、多相はモード変換 (平衡な線路は Clarke 変換、
  行列で与えた線路は L·C の固有ベクトル)。フェーザ解・定常初期化・周波数スキャンに対応。E230・E231、W230・W231
  (15c4527、84d26c8)
* **core:** 区分線形の `nonlinear_resistor` (表と、避雷器のべき乗の式)、`nonlinear_inductor`、`transformer` と
  `three_phase_transformer` の `saturation`。E260〜E264、W260・W261 (294c510)
* **core:** 制御ブロック 21 種 (伝達関数、積分器、1 次遅れ、進み遅れ、むだ時間、リミッタ、変化率リミッタ、不感帯、
  積・商・関数、選択、t = 0 の値の保持、時間関数、abc/dq0 変換、PLL、三相の測定)、状態を通る閉ループ、三相の部品と
  素子の信号の測定、制御電源 4 種。E240〜E244、E250・E251、W240 (d8a9561)
* **core:** `simulation.switching` = `interpolated`: 切り替え時刻で刻みを 2 つに分けて解く。既定は `grid` のまま。
  W270 (33727c6、7ff5478)
* **core:** ダイオードと制御スイッチの抵抗が既定のまま、遮断中の電圧が 10 kV を超えると W271。抵抗の決め方を README と
  カタログに書いた (12cfef5、66c3dfb)
* **core:** 同期機 `synchronous_machine` (dq 軸のモデル 2.2 と 2.1、運動方程式、界磁電圧と機械トルクの信号、潮流からの
  初期化)。E280〜E282、W280 (23981ff、a37a529、a3a0b40)
* **model:** 系統連系インバータの平均値モデル `grid_following_converter` (PLL、dq の電流制御、電流と電圧の制限) と、
  制御ブロック `dq_limiter`・`pi_dq`。E290、W290〜W293 (ecf924f、88a017d)
* **core:** `bergeron_line` で回路を部分に分け、部分ごとの行列で解く。`simulation.partitioning` (`auto`・`every_line`・
  `off`) と `simulation.threads` (既定 1)。逐次と並列の結果はビット単位で同じ。E301、W301 (b816cde、c5846df、2f24cf6)
* **model:** 理想素子のループ (E142) と、電圧を支えるものの無いノード (E143) を検証で見つける。スイッチや故障の時刻で
  後から起きる場合は W142・W143 (6a41180)
* **cli:** `openkeito run --threads N`。`summary.json` に、動作点の反復・制御ループの反復・刻みの中の切り替えの回数、
  部分の数とスレッド数 (67b45ed、a25f59a、b330868、2f24cf6)
* **py:** 変換器で、発電機を同期機に (`generators="machine"`)、sgen・蓄電池を変換器に (`inverter_model="converter"`)
  選べる (889a47c、3f793cd)
* **py:** PowSyBl の変換で、短絡データ (x″、零相、結線、接地) を、IIDM の拡張、CGMES の EQ の SC の属性、引数
  `short_circuit_data` から読む (68ca01b)
* **py:** 計画値の変化を追う推移の計算 `openkeito.transition` と、`pandapower`・`powsybl` の `transition`・
  `run_transitions` (404b5de)
* **mcp:** 道具 `run_transitions`。CGMES のファイルの短絡の属性を MCP の道具でも読む (46564c9、a785234)

### Bug Fixes

* **core:** 零相と正相の比が大きい結合 RL で、履歴項の行列の積が桁を落とし、t = 0+ の電圧がずれていた (45977b8)
* **model:** 測定が選んだ端子・相の列が無いことを、`run` でなく `validate` で知らせる (f93c521)

### Performance

* **core:** `bergeron_line` の履歴の読み出しを刻みごとに 1 回にした。輪状の基準系統で `pi_line` の 0.87〜1.02 倍
  (5425711)
* **core:** 族ごとのモジュール分けで遅くなった分を、小さな補助関数の `#[inline]` で戻した。分割しない回路の 1 段を、
  部分の手間なしで解く (1e17f39、1e70a91)

### Documentation

* `docs/performance.md` の数字を、コアを固定して取り直した (速いコアと遅いコアが混在する機械で、以前の値は 8〜25 %
  長かった)。線路の分割と並列、M4 の部品、推移の計算の時間を足した
* `docs/numerical-method.md` に、M4 の部品と方法、比べて捨てた方式の表、文献を足した
* plan.md に M4 の設計・結果・決定事項を書き、M4 を済、M5 を次にした

## 0.4.1 (2026-11-22)

M0〜M3 のレビュー (issue #8〜#23) で見つかった不具合の修正。モデル形式の版は `0.4` のまま。

結果が変わるもの:

- `step` 波形を使うモデル。ステップの時刻の行は `before` の値になり、`events` に `step` が載る。
- `end_time_s / time_step_s` が丸めで整数をわずかに下回っていたモデル (60 s / 2.5 µs など) は、刻みが 1 つ増える。
- これまで検証を通っていたモデルの一部が拒まれる (E113、E123、E125、E139、E155、E194、ケース名の E402)。

### Bug Fixes

* **model:** 検証の抜けをふさいだ。`fault` の `phases: []` と `ground: true` で `validate` が panic していた。
  故障の相のノードの `gnd`・重複 (E123)、列名が重複するモデル (E125・E155・E194)、解けないほど離れた零相と正相
  (E139)、1e12 を超える刻みの数 (E113) を拒み、出力が 1e8 行を超えると警告する (W114)。刻みの数は整数から 1e-6
  以内なら丸める。`pi_line` だけが接地につながる系統を E140 にしていた。ケース名は、大文字と小文字だけの違い、
  末尾の点、Windows のデバイス名、バッチ自身のファイル名を拒む (E402)。バッチの inline モデルは JSON のまま持ち、
  モデルでもパスでもない値は E403。E001 の文面を文書の種類に合わせた (48a4dc4)
* **core:** `step` 波形の電源が、指定の時刻より 0.29 刻み (2S-DIRK)・0.5 刻み (台形則) 早く効いていた。スイッチと
  同じく、`time_s` 以後の最初の格子点の出力のあとで値を変える。結合 RL の行列が丸めで特異になると panic していた
  (E204)。電流零点を待ったまま終わったスイッチと故障を警告にする (W221)。失敗した実行でも、そこまでのイベントと
  回数を返す。格子点と時刻を比べる許容を、長い実行でも足りるようにした (fee03cd)
* **cli:** `--force` は、実行の前に前の結果のファイルを消す。失敗した実行の summary にイベントとソルバの回数を
  書く。バッチの inline モデルの誤りは、そのケースの診断になる。Python の `run_batch` でもケース名を検査し
  (`out` の外に書けた)、既にあるバッチの結果は `force` なしでは拒む。`validate` がバッチファイルを見分け、
  ケースのモデルも検証する。run と scan、バッチの要約の書き出しの重複をまとめた (89179b0)
* **py:** pandapower のタップは、潮流が当てる場合 (`tap_changer_type` が Ratio・Symmetrical・Ideal で `tap_side`
  が hv・lv) だけ当てる (母線電圧が 4.8e-2 pu ずれていた)。片端が開いた枝、休止中の母線につながる線路を残す。
  `g_us_per_km` を入れる。開いた側がデルタの変圧器を、無負荷の電力で表す。`schedule_from_opf` (AC) が無効電力も
  計画値に入れる。計画した電力は潮流に入る値 (`scaling` で割る)。外乱の名前の重複、存在しない母線の故障、
  収束しない時刻、NaN の計画値を、分かるエラーにする。潮流の設定を `power_flow_options` で渡せる (1431a0d)
* **mcp:** pickle ファイルを読まない。pandapower の JSON が、pandapower 自身の書かないモジュールの物体を名指し
  していたら拒む。既存のファイルは `force` なしでは上書きしない。使い方の誤りを、理由の付いた ToolError にする。
  `query_results` は `max_rows` 行・20 000 個の値・500 列までを返し、列の多いモデルの summary は列名だけを返す
  (1f6041d)

### Documentation

* README・AGENTS・numerical-method・plan に、上の動作と、モデル形式の版 (0.4)、M3 の状況を書いた (357eeca)

### Build・CI

* Python の lint (pyflakes) と、Python 3.10 のジョブを加えた (2ba09bd)

## 0.4.0 (2026-11-21)

Python・潮流からの初期化・計画値・MCP (計画の M3)。Python パッケージと一括実行、pandapower と PowSyBl (CGMES) の
潮流から始める変換、計画値ファイルと時間断面の並列実行、MCP サーバーを加えた。モデル形式の版を `0.4` に上げた
(`three_phase_load`・`three_phase_current_source`・`three_phase_ideal_transformer`)。`0.3` 以前のモデルはそのまま
読める。新しいファイル形式は一括実行 (`openkeito-batch` 0.1) と計画値 (`openkeito-schedule` 0.1)。

### Features

* **cli:** 実行をライブラリ (`openkeito_cli`) にし、`openkeito batch` (ケースの並列実行、`batch-summary.json`・
  `batch-metrics.csv`) と、summary の列ごとの統計 (`result.statistics`) を加えた。新しい診断は E400〜E402 (46c1a33)
* **py:** Python パッケージ `openkeito` (PyO3 の abi3 拡張 `openkeito._native` と純 Python の包み)。`run`・`scan`・
  `validate`・`schema`・`components`・`run_batch`、numpy の列、`OpenKeitoError` の診断 (59777bb)
* **core:** 三相の負荷 (定インピーダンス) と三相の電流源を追加し、形式を 0.4 にした (cbf46f7)
* **model:** 計画値ファイルの形式と検証を追加。新しい診断は E500〜E507 (76e0556)
* **cli:** `openkeito validate` が計画値ファイルを検証し、`openkeito schema --schedule` を加えた (565269a)
* **py:** pandapower からの変換 (潮流解を定常値にする)、計画値 (`Schedule`)、OPF からの計画値
  (`schedule_from_opf`)、時間断面 (`run_time_slices`) を追加 (754e631)
* **core:** 三相の理想変圧器 (対称な移相器) `three_phase_ideal_transformer` を追加 (451b136)
* **py:** PowSyBl (CGMES) からの変換と、IIDM の計画値・時間断面を追加。既定の潮流は計画値を守り、最大の発電機を
  スラックにし、残る不つり合いを母線の発電機に持たせる (68cb473)
* **mcp:** MCP サーバー `openkeito-mcp` (9 つの道具) を追加 (102f5fa)

### Refactoring

* **py:** 変換器の共通部分 (対応表、潮流との比較、時間断面) を `openkeito.convert` に分けた (463660c)

### Build・CI

* Python パッケージをビルドして pytest を走らせるジョブを 3 OS に加え (e7ac0d0)、pandapower (e23bd27)、pypowsybl と
  MCP SDK (0e2c7b8) を入れた

## 0.3.0 (2026-11-21)

三相 (計画の M2)。三相の部品と故障、フェーザ解による定常初期化、周波数スキャン、疎行列の LU を加えた。
モデル形式の版を `0.3` に上げた。`0.1`・`0.2` のモデルはそのまま読める。古い版と書いて `0.3` で加えたもの
(三相の部品、`bus_voltage` の出力、`simulation.initialization`、`simulation.linear_solver`、`frequency_scan`)
を使うと E102 になり、ヒントに必要な版と各版の追加が出る。

### Features

* **core:** 三相の部品と故障を追加。三相電源 (内部インピーダンス付き)、結合 RL、π 形線路 (区間数)、単相変圧器、
  三相変圧器 (単相器 3 台のバンク。Yy0/6・Yd1/5/7/11・Dy1/5/7/11・Dd0/6)、故障 (相・地絡・故障抵抗・相ごとの電流零点での除去)。
  母線 `b` の相はノード `b.a`・`b.b`・`b.c`。新しい診断は E124、E133〜E138、E153、E154、E173。故障電流を対称座標法と、
  変圧器を等価回路と、π 形線路を分布定数の解と比べるテストを含む (f593aa0)
* **core:** 定常初期化 (`simulation.initialization`: `"steady_state"`) と周波数スキャン (`frequency_scan`) を追加。
  密行列の LU を実数と複素数で共通にした。新しい診断は E180〜E182、W183、W184、E190〜W193、E203、W220 (5f09b9d)
* **cli:** `openkeito scan` (`scan.csv`) と、定常初期化の summary (`initialization`) と `phasors.csv` を追加 (0ef50c6)
* **core:** 疎行列の LU (faer。列の並べ替えは 1 回、分解の前に行の尺度合わせ) を加え、`simulation.linear_solver`
  (`auto`・`dense`・`sparse`) で選べるようにした。`auto` は未知数 16 以上で疎行列。M1・M1.5 の回路は密行列のまま (2621bcd)
* **cli:** summary の `solver` に、使った線形ソルバと条件数の見積もりの最大を出す (e61819e)

### Bug Fixes

* **core:** t = 0+ の解で電源を ε (1e-6 刻み) の時刻に取る。電源に直接つながるコンデンサの電流が t = 0+ で 0 になり、
  台形則ではその誤った導関数が減衰せずに残っていた (M1 から)。M1・M1.5 の例で変わるのは最初の行だけ (bc0b828)
* **core:** `scan.csv` の相互インピーダンスの列名を `Zmag(n;m)` にした。コンマで見出しの欄が余分に分かれていた (ab4d8f5)

### Build・CI

* テストのビルドを opt-level 1 にした (f652f49)
* openkeito-core が wasm32-unknown-unknown でビルドできることを CI で確かめる (dea6f03)

## 0.2.0 (2026-11-21)

パワエレの試作 (計画の M1.5)。ダイオード、制御信号で動くスイッチ、最小限の制御ブロックを加え、降圧チョッパと
単相ダイオード整流器を解析解と比べた。モデル形式の版を `0.2` に上げた。`0.1` のモデルはそのまま読める。`0.1` と書いて
`0.2` で加えたもの (diode、controlled_switch、controls、信号の出力、output_interval_steps) を使うと E102 になる。

### Features

* **core:** ダイオード・制御スイッチ・制御ブロックを追加。ダイオードと制御スイッチは導通時・遮断時の抵抗を持つ 2 値抵抗
  (既定 1 mΩ・1 MΩ)。状態が変わった格子点で全ダイオードの状態を確定し、決まらなければ E202 にする。制御ブロックは
  constant、gain、sum、triangle_wave、comparator、pi、measurement で、出力は次の刻みに効く。新しい診断は E102、E132、
  E170〜E172、E202。降圧チョッパ (連続・不連続導通、PI 制御) と単相ブリッジ整流器 (抵抗負荷・コンデンサ入力) を
  厳密解と教科書の式に比べるテストを含む (18aa6ab)
* **cli:** `events.csv` に全部のスイッチ動作を書き、`summary.json` に解き直しの回数と計算時間の内訳 (`timing`) を加えた。
  `summary.json` の `events` は先頭 100 件になり、`event_count` と部品ごとの件数が付く (3dd5fa5)

## 0.1.0 (2026-11-20)

最初の版 (計画の M0・M1)。線形の R・L・C、電源、理想スイッチの回路を、JSON のモデルからコマンド 1 つで解く。
モデル形式の版は `0.1`。

### Features

* **model:** JSON のモデル形式と検証を追加。単位をフィールド名に入れ、未知のフィールドと指数文字のない数値 (`1.1141-6`) はエラーにして直し方を返す。部品の一覧と JSON Schema を出せる (a8c0e14)
* **core:** 節点解析と 2S-DIRK・台形則のソルバを追加。R・L・C、電圧源・電流源、理想スイッチ (時刻指定、電流零点での開放) に対応し、解析解と比べるテストを含む (9338003)
* **cli:** `openkeito` コマンド (`validate`、`run`、`schema`、`components`) を追加。`run` は `result.csv` と `summary.json` を書く (9eac891)
