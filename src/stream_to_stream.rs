//! XMLストリームをツール呼び出しイベントのストリームに変換するモジュール
//!
//! このモジュールは、XMLフォーマットのテキストストリームを解析し、
//! ツール呼び出しイベントのストリームに変換する機能を提供します。
//!
//! # 主な機能
//!
//! - XMLテキストの1文字ずつのストリーミング処理
//! - ツール呼び出しの開始・終了の検出
//! - パラメータの収集と構造化
//! - イベントの生成と配信
//!
//! # イベントの種類
//!
//! - `ToolStart`: ツール呼び出しの開始
//! - `Parameter`: ツールのパラメータ
//! - `ToolEnd`: ツール呼び出しの終了
//! - `Text`: XMLタグ以外のテキスト
//! - `Error`: エラー発生時のイベント
//!
//! # 使用例
//!
//! ```rust
//! use futures::StreamExt;
//!
//! let input = r#"<get_weather>
//!   <location>Tokyo</location>
//!   <date>tomorrow</date>
//! </get_weather>"#;
//!
//! let input_stream = Box::pin(futures::stream::iter(input.chars().map(|c| c.to_string())));
//! let mut stream = stream_to_stream(input_stream)?;
//!
//! while let Some(event) = stream.next().await {
//!     match event {
//!         ToolCallEvent::ToolStart { id, name } => println!("ツール開始: {} (ID: {})", name, id),
//!         ToolCallEvent::Parameter { id, arguments } => println!("パラメータ (ID: {}): {:?}", id, arguments),
//!         ToolCallEvent::ToolEnd { id } => println!("ツール終了 (ID: {})", id),
//!         ToolCallEvent::Text(text) => print!("{}", text),
//!         ToolCallEvent::Error(err) => eprintln!("エラー: {}", err),
//!     }
//! }
//! ```

use futures::StreamExt;
use futures::stream::BoxStream;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_stream::Stream;

type Result<T> = std::result::Result<T, ToolCallStreamError>;

/// ストリーム処理中に発生する可能性のあるエラー
#[derive(thiserror::Error, Debug)]
pub enum ToolCallStreamError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Unexpected end of file")]
    UnexpectedEof,
}

/// ストリーミングイベントを表すenum
/// XMLの解析結果を表現するために使用される
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallEvent {
    /// テキストイベント：XMLタグ以外のテキストを表す
    Text(String),
    /// ツール呼び出しの開始：<tool_name>タグの検出
    ToolStart { id: String, name: String },
    /// パラメータの受信：ツール呼び出しに含まれるパラメータ
    Parameter {
        id: String,
        arguments: serde_json::Value,
    },
    /// ツール呼び出しの終了：</tool_name>タグの検出
    ToolEnd { id: String },
    /// エラーイベント：処理中に発生したエラー
    Error(String),
}

type ToolCallStream = BoxStream<'static, ToolCallEvent>;
type ToolCallStreamResult = Result<ToolCallStream>;

/// パーサーの状態を表すenum
#[derive(Debug)]
enum ParserState {
    /// 通常状態：XMLタグ外
    Normal,
    /// タグ解析中：< と > の間
    InTag,
    /// ツールタグ内：<tool_name> と </tool_name> の間
    InToolTag,
    /// パラメータタグ内：<param_name> と </param_name> の間
    InParameterTag(String), // param_name
}

/// XMLストリームをイベントストリームに変換するための構造体
struct StreamToStream {
    /// 入力ストリーム
    input: BoxStream<'static, String>,
    /// タグ名を一時的に保存するバッファ
    tag_buffer: String,
    /// 現在のパーサー状態
    state: ParserState,
    /// 現在のツールのパラメータを保持
    current_params: serde_json::Map<String, serde_json::Value>,
    /// パラメータの値を一時的に保存するバッファ
    param_value_buffer: String,
    /// 現在処理中のツール名
    current_tool: Option<String>,
    /// 直前の文字が改行だったかどうか
    last_char_was_newline: bool,
    /// ToolEndイベントを発行する必要があるかどうか
    need_to_emit_tool_end: bool,
    /// XMLタグ内にいるかどうか
    in_xml: bool,
    /// 現在のツールのID
    current_id: Option<String>,
    /// IDカウンター
    id_counter: u64,
}

impl StreamToStream {
    /// 新しいStreamToStreamインスタンスを作成
    fn new(input: BoxStream<'static, String>) -> Self {
        Self {
            input,
            tag_buffer: String::new(),
            state: ParserState::Normal,
            current_params: serde_json::Map::new(),
            param_value_buffer: String::new(),
            current_tool: None,
            last_char_was_newline: false,
            need_to_emit_tool_end: false,
            in_xml: false,
            current_id: None,
            id_counter: 0,
        }
    }

    /// 新しいIDを生成
    fn generate_id(&mut self) -> String {
        self.id_counter += 1;
        format!("tool_{}", self.id_counter)
    }

    /// 1文字を処理し、必要に応じてイベントを生成
    ///
    /// # 改行の処理ルール
    /// - XMLタグ内の改行は無視
    /// - 連続する改行は1回だけ出力
    /// - それ以外の改行は通常通り出力
    fn process_char(&mut self, c: &str) -> Option<ToolCallEvent> {
        match &self.state {
            ParserState::Normal => {
                if c == "<" {
                    // XMLタグの開始
                    self.state = ParserState::InTag;
                    self.tag_buffer.clear();
                    self.in_xml = true;
                    None
                } else if c == "\n" {
                    if self.last_char_was_newline {
                        // 連続する改行は無視（XMLタグ内外に関わらず）
                        None
                    } else {
                        // 最初の改行は出力（XMLタグ内外に関わらず）
                        self.last_char_was_newline = true;
                        Some(ToolCallEvent::Text(c.to_string()))
                    }
                } else {
                    // 空白文字を含むすべての文字を出力
                    self.last_char_was_newline = false;
                    Some(ToolCallEvent::Text(c.to_string()))
                }
            }
            ParserState::InTag => {
                if c == ">" {
                    // タグの終了
                    let tag = std::mem::take(&mut self.tag_buffer);
                    if tag.starts_with('/') {
                        // 終了タグの処理
                        let tag_name = tag[1..].to_string();
                        if let Some(current_tool) = &self.current_tool {
                            if current_tool == &tag_name {
                                // ツールの終了
                                self.state = ParserState::Normal;
                                let id = self
                                    .current_id
                                    .clone()
                                    .unwrap_or_else(|| "unknown".to_string());
                                self.current_tool = None;
                                self.in_xml = false;
                                self.last_char_was_newline = false;
                                if !self.current_params.is_empty() {
                                    // パラメータがある場合は、まずパラメータを出力
                                    let params = std::mem::take(&mut self.current_params);
                                    self.need_to_emit_tool_end = true;
                                    Some(ToolCallEvent::Parameter {
                                        id: id.clone(),
                                        arguments: serde_json::Value::Object(params),
                                    })
                                } else {
                                    // パラメータがない場合は、直接ToolEndを出力
                                    self.current_id = None;
                                    Some(ToolCallEvent::ToolEnd { id })
                                }
                            } else {
                                // パラメータタグの終了
                                let value = std::mem::take(&mut self.param_value_buffer);
                                self.current_params.insert(
                                    tag_name,
                                    serde_json::Value::String(value.trim().to_string()),
                                );
                                self.state = ParserState::InToolTag;
                                None
                            }
                        } else {
                            // 予期しない終了タグ
                            self.state = ParserState::Normal;
                            self.in_xml = false;
                            None
                        }
                    } else if self.current_tool.is_none() {
                        // ツールの開始
                        let id = self.generate_id();
                        self.current_id = Some(id.clone());
                        self.current_tool = Some(tag.clone());
                        self.state = ParserState::InToolTag;
                        Some(ToolCallEvent::ToolStart { id, name: tag })
                    } else {
                        // パラメータタグの開始
                        self.state = ParserState::InParameterTag(tag);
                        self.param_value_buffer.clear();
                        None
                    }
                } else {
                    // タグ名の収集
                    self.tag_buffer.push_str(c);
                    None
                }
            }
            ParserState::InToolTag => {
                if c == "<" {
                    // 新しいタグの開始
                    self.state = ParserState::InTag;
                    self.tag_buffer.clear();
                    None
                } else if !c.trim().is_empty() {
                    // ツールタグ内のテキスト
                    Some(ToolCallEvent::Text(c.to_string()))
                } else {
                    None
                }
            }
            ParserState::InParameterTag(_) => {
                if c == "<" {
                    // パラメータタグの終了
                    self.state = ParserState::InTag;
                    self.tag_buffer.clear();
                    None
                } else {
                    // パラメータ値の収集
                    self.param_value_buffer.push_str(c);
                    None
                }
            }
        }
    }
}

/// Stream traitの実装
impl Stream for StreamToStream {
    type Item = ToolCallEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();

        // ToolEndイベントの遅延発行
        if this.need_to_emit_tool_end {
            this.need_to_emit_tool_end = false;
            if let Some(id) = this.current_id.take() {
                return Poll::Ready(Some(ToolCallEvent::ToolEnd { id }));
            }
        }

        // 入力ストリームからの次の文字を処理
        match this.input.poll_next_unpin(cx) {
            Poll::Ready(Some(c)) => {
                if let Some(event) = this.process_char(&c) {
                    Poll::Ready(Some(event))
                } else {
                    self.poll_next(cx)
                }
            }
            Poll::Ready(None) => {
                // 入力ストリームの終了処理
                if this.need_to_emit_tool_end {
                    this.need_to_emit_tool_end = false;
                    if let Some(id) = this.current_id.take() {
                        Poll::Ready(Some(ToolCallEvent::ToolEnd { id }))
                    } else {
                        Poll::Ready(None)
                    }
                } else if this.current_tool.is_some() {
                    if let Some(id) = this.current_id.take() {
                        this.current_tool = None;
                        Poll::Ready(Some(ToolCallEvent::ToolEnd { id }))
                    } else {
                        Poll::Ready(None)
                    }
                } else {
                    Poll::Ready(None)
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// 入力ストリームをツール呼び出しイベントのストリームに変換
fn stream_to_stream(input: BoxStream<'static, String>) -> ToolCallStreamResult {
    let stream = StreamToStream::new(input);
    Ok(Box::pin(stream))
}

#[tokio::main]
async fn main() {
    // サンプルの入力テキスト
    let input = r#"明日のニューヨークの天気を確認します。

<get_weather>
  <location>New York</location>
  <date>tomorrow</date>
  <unit>fahrenheit</unit>
</get_weather>

天気予報を取得しました。次に、ファイルに書き込みます。

<write_to_file>
<path>weather_report.txt</path>
<content>
明日のニューヨークの天気予報：
- 最高気温: 75°F
- 最低気温: 60°F
- 天候: 晴れ時々曇り
</content>
</write_to_file>

処理が完了しました。"#;

    // 入力テキストを1文字ずつのストリームに変換
    let input_stream = Box::pin(futures::stream::iter(input.chars().map(|c| c.to_string())));

    // ストリームを処理
    match stream_to_stream(input_stream) {
        Ok(mut stream) => {
            // イベントを順番に処理
            while let Some(event) = stream.next().await {
                match event {
                    ToolCallEvent::Text(text) => {
                        // テキストイベントの処理
                        print!("{}", text);
                    }
                    ToolCallEvent::ToolStart { id, name } => {
                        // ツール開始イベントの処理
                        println!("\n[ツール開始: {} (ID: {})]", name, id);
                    }
                    ToolCallEvent::Parameter { id, arguments } => {
                        // パラメータイベントの処理
                        println!(
                            "[パラメータ (ID: {}): {}]",
                            id,
                            serde_json::to_string_pretty(&arguments).unwrap()
                        );
                    }
                    ToolCallEvent::ToolEnd { id } => {
                        // ツール終了イベントの処理
                        println!("[ツール終了 (ID: {})]\n", id);
                    }
                    ToolCallEvent::Error(err) => {
                        eprintln!("エラー: {}", err);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("ストリームの作成に失敗しました: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tokio_stream::StreamExt;

    /// メインのストリーム変換テスト
    ///
    /// このテストでは以下の点を確認します：
    /// - テキストの1文字ずつの処理
    /// - XMLタグの適切な解析
    /// - パラメータの収集と出力
    /// - 改行の適切な処理
    #[tokio::test]
    async fn test_stream_to_stream() -> Result<()> {
        let input = r#"明日のニューヨークの天気ですね。承知いたしました。

<get_weather>
  <location>New York</location>
  <date>tomorrow</date>
  <unit>fahrenheit</unit>
</get_weather>

結果が取得でき次第、すぐにお知らせします。"#;
        let input_stream = Box::pin(futures::stream::iter(input.chars().map(|c| c.to_string())));

        let expected_events = vec![
            // 一文字ずつ返す
            ToolCallEvent::Text("明".into()),
            ToolCallEvent::Text("日".into()),
            ToolCallEvent::Text("の".into()),
            ToolCallEvent::Text("ニ".into()),
            ToolCallEvent::Text("ュ".into()),
            ToolCallEvent::Text("ー".into()),
            ToolCallEvent::Text("ヨ".into()),
            ToolCallEvent::Text("ー".into()),
            ToolCallEvent::Text("ク".into()),
            ToolCallEvent::Text("の".into()),
            ToolCallEvent::Text("天".into()),
            ToolCallEvent::Text("気".into()),
            ToolCallEvent::Text("で".into()),
            ToolCallEvent::Text("す".into()),
            ToolCallEvent::Text("ね".into()),
            ToolCallEvent::Text("。".into()),
            ToolCallEvent::Text("承".into()),
            ToolCallEvent::Text("知".into()),
            ToolCallEvent::Text("い".into()),
            ToolCallEvent::Text("た".into()),
            ToolCallEvent::Text("し".into()),
            ToolCallEvent::Text("ま".into()),
            ToolCallEvent::Text("し".into()),
            ToolCallEvent::Text("た".into()),
            ToolCallEvent::Text("。".into()),
            ToolCallEvent::Text("\n".into()),
            ToolCallEvent::ToolStart {
                id: "tool_1".to_string(),
                name: "get_weather".to_string(),
            },
            ToolCallEvent::Parameter {
                id: "tool_1".to_string(),
                arguments: serde_json::json!({
                    "location": "New York",
                    "date": "tomorrow",
                    "unit": "fahrenheit"
                }),
            },
            ToolCallEvent::ToolEnd {
                id: "tool_1".to_string(),
            },
            ToolCallEvent::Text("\n".into()),
            ToolCallEvent::Text("結".into()),
            ToolCallEvent::Text("果".into()),
            ToolCallEvent::Text("が".into()),
            ToolCallEvent::Text("取".into()),
            ToolCallEvent::Text("得".into()),
            ToolCallEvent::Text("で".into()),
            ToolCallEvent::Text("き".into()),
            ToolCallEvent::Text("次".into()),
            ToolCallEvent::Text("第".into()),
            ToolCallEvent::Text("、".into()),
            ToolCallEvent::Text("す".into()),
            ToolCallEvent::Text("ぐ".into()),
            ToolCallEvent::Text("に".into()),
            ToolCallEvent::Text("お".into()),
            ToolCallEvent::Text("知".into()),
            ToolCallEvent::Text("ら".into()),
            ToolCallEvent::Text("せ".into()),
            ToolCallEvent::Text("し".into()),
            ToolCallEvent::Text("ま".into()),
            ToolCallEvent::Text("す".into()),
            ToolCallEvent::Text("。".into()),
        ];
        let mut stream = stream_to_stream(input_stream)?;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        assert_eq!(events, expected_events);
        Ok(())
    }

    #[tokio::test]
    async fn test_write_to_file() -> Result<()> {
        let input = r#"Okay, I will write the following content to the file.
<write_to_file>
<path>src/main.rs</path>
<content>
fn main() {
    println!("Hello, world!");
}
</content>
</write_to_file>
Let me know if that looks correct."#;

        let input_stream = Box::pin(futures::stream::iter(input.chars().map(|c| c.to_string())));

        let expected_events = vec![
            // 最初のテキスト
            ToolCallEvent::Text("O".into()),
            ToolCallEvent::Text("k".into()),
            ToolCallEvent::Text("a".into()),
            ToolCallEvent::Text("y".into()),
            ToolCallEvent::Text(",".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("I".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("w".into()),
            ToolCallEvent::Text("i".into()),
            ToolCallEvent::Text("l".into()),
            ToolCallEvent::Text("l".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("w".into()),
            ToolCallEvent::Text("r".into()),
            ToolCallEvent::Text("i".into()),
            ToolCallEvent::Text("t".into()),
            ToolCallEvent::Text("e".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("t".into()),
            ToolCallEvent::Text("h".into()),
            ToolCallEvent::Text("e".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("f".into()),
            ToolCallEvent::Text("o".into()),
            ToolCallEvent::Text("l".into()),
            ToolCallEvent::Text("l".into()),
            ToolCallEvent::Text("o".into()),
            ToolCallEvent::Text("w".into()),
            ToolCallEvent::Text("i".into()),
            ToolCallEvent::Text("n".into()),
            ToolCallEvent::Text("g".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("c".into()),
            ToolCallEvent::Text("o".into()),
            ToolCallEvent::Text("n".into()),
            ToolCallEvent::Text("t".into()),
            ToolCallEvent::Text("e".into()),
            ToolCallEvent::Text("n".into()),
            ToolCallEvent::Text("t".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("t".into()),
            ToolCallEvent::Text("o".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("t".into()),
            ToolCallEvent::Text("h".into()),
            ToolCallEvent::Text("e".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("f".into()),
            ToolCallEvent::Text("i".into()),
            ToolCallEvent::Text("l".into()),
            ToolCallEvent::Text("e".into()),
            ToolCallEvent::Text(".".into()),
            ToolCallEvent::Text("\n".into()),
            // ツール呼び出しの開始
            ToolCallEvent::ToolStart {
                id: "tool_1".to_string(),
                name: "write_to_file".to_string(),
            },
            // パラメータ
            ToolCallEvent::Parameter {
                id: "tool_1".to_string(),
                arguments: serde_json::json!({
                    "path": "src/main.rs",
                    "content": "fn main() {\n    println!(\"Hello, world!\");\n}"
                }),
            },
            // ツール呼び出しの終了
            ToolCallEvent::ToolEnd {
                id: "tool_1".to_string(),
            },
            // 最後のテキスト
            ToolCallEvent::Text("\n".into()),
            ToolCallEvent::Text("L".into()),
            ToolCallEvent::Text("e".into()),
            ToolCallEvent::Text("t".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("m".into()),
            ToolCallEvent::Text("e".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("k".into()),
            ToolCallEvent::Text("n".into()),
            ToolCallEvent::Text("o".into()),
            ToolCallEvent::Text("w".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("i".into()),
            ToolCallEvent::Text("f".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("t".into()),
            ToolCallEvent::Text("h".into()),
            ToolCallEvent::Text("a".into()),
            ToolCallEvent::Text("t".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("l".into()),
            ToolCallEvent::Text("o".into()),
            ToolCallEvent::Text("o".into()),
            ToolCallEvent::Text("k".into()),
            ToolCallEvent::Text("s".into()),
            ToolCallEvent::Text(" ".into()),
            ToolCallEvent::Text("c".into()),
            ToolCallEvent::Text("o".into()),
            ToolCallEvent::Text("r".into()),
            ToolCallEvent::Text("r".into()),
            ToolCallEvent::Text("e".into()),
            ToolCallEvent::Text("c".into()),
            ToolCallEvent::Text("t".into()),
            ToolCallEvent::Text(".".into()),
        ];

        let mut stream = stream_to_stream(input_stream)?;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }

        assert_eq!(events, expected_events);
        Ok(())
    }
}
