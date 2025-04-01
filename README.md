# Tool Use With XML

XMLフォーマットのテキストストリームを解析し、ツール呼び出しイベントのストリームに変換する実験的な実装です。

## 概要

このプロジェクトは、LLMの出力するXMLフォーマットのテキストを解析し、ツール呼び出しイベントに変換するための実験的な実装（PoC）です。

## 現在の機能

- XMLテキストの1文字ずつのストリーミング処理
- ツール呼び出しの開始・終了の検出
- パラメータの収集と構造化
- イベントの生成と配信

## 必要要件

- Rust（最新の安定版を推奨）
- Cargo

## 実行方法

### ビルドと実行

```bash
# ビルド
cargo build --bin stream_to_stream

# 実行
cargo run --bin stream_to_stream
```

### テストの実行

```bash
# すべてのテストを実行
cargo test --package tool_use_with_xml --bin stream_to_stream

# テスト出力を表示
cargo test --package tool_use_with_xml --bin stream_to_stream -- tests --show-output
```

## 開発状況

現在は実験的な実装段階で、以下の機能を検証しています：

- ストリーミング処理の基本実装
- XMLパースの動作検証
- イベント生成の仕組みの確認

### コード品質チェック

```bash
# フォーマットチェック
cargo fmt --all -- --check

# 静的解析
cargo clippy -- -D warnings
```

## CI/CD

GitHub Actionsで基本的な動作確認を自動化しています：

- コードフォーマットチェック
- 静的解析
- ユニットテスト
- ビルド

### CI/CDステータス

[![Rust CI](https://github.com/takanorifukuyama/tool_use_with_xml/actions/workflows/ci.yml/badge.svg)](https://github.com/takanorifukuyama/tool_use_with_xml/actions/workflows/ci.yml)

## 実装例

```rust
use futures::StreamExt;

let input = r#"<get_weather>
  <location>Tokyo</location>
  <date>tomorrow</date>
</get_weather>"#;

let input_stream = Box::pin(futures::stream::iter(input.chars().map(|c| c.to_string())));
let mut stream = stream_to_stream(input_stream)?;

while let Some(event) = stream.next().await {
    match event {
        ToolCallEvent::ToolStart { id, name } => println!("ツール開始: {} (ID: {})", name, id),
        ToolCallEvent::Parameter { id, arguments } => println!("パラメータ (ID: {}): {:?}", id, arguments),
        ToolCallEvent::ToolEnd { id } => println!("ツール終了 (ID: {})", id),
        ToolCallEvent::Text(text) => print!("{}", text),
        ToolCallEvent::Error(err) => eprintln!("エラー: {}", err),
    }
}
```

## ライセンス

MIT License 